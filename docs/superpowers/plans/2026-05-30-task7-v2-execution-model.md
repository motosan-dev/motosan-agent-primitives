# Task 7 v2 — Per-Call Execution Model (clean rebuild, batched)

> **For agentic workers:** This is **design-first**. The architecture (Part 1) must be reviewed/approved before any code. Then implement in **batches** (Part 2): Batch 0 → A → B, in order, each its own review gate. **Do NOT patch incrementally on the old shared pipeline** — that is exactly what v1 did and it produced "green but wrong" concurrency bugs. v1 is preserved at loop branch `wip/task7-v1-botched`; do not build on it.

**Goal:** Wire the dormant `Reviewer` (spec §9a) into the permission path by **rebuilding** tool execution into uniform per-call futures, so permission-review, the `ask_user` extension, streaming, and execution can never cross-block at a batch level.

**Decision:** the **clean** rebuild (uniform per-call model), NOT a surgical hybrid. A hybrid would leave two mechanisms for "a tool call that waits" (batch resolver + per-call), which is harder to maintain than one uniform path. Maintainability over minimal-diff is the deliberate choice.

**Parent:** [2026-05-29-reviewer-approval-seam.md](2026-05-29-reviewer-approval-seam.md) Phase 2 Task 7. **Spike:** spec §9d. **Baseline:** loop@`ee9fec4` (clean Task 6, dormant Reviewer API).

---

## Part 1 — Architecture (REVIEW BEFORE CODING)

### Why v1 failed (so v2 doesn't repeat it)

Today's engine is two-phase: a sequential slot **pre-pass** (`dispatch_tool_call_to_slot` → `consult_policy` → `InterceptedSlot`) then a batch-level `join!(resolve_deferred_slots, execute_tools_parallel)`. Multiple independent waits (permission review, `ask_user` defer, streaming) are multiplexed by the one batch-level resolver, so they cross-block. v1 patched blocking points one at a time; each fix exposed another entangled wait. The batch resolver must go, not be patched.

### Target shape

No sequential slot pre-pass, no `InterceptedSlot` defer zoo. Each tool call is a `run_tool_call` future (permission/review → intercept → execute). The batch runs them concurrently with an **ops loop evolved from today's `resolve_deferred_slots`** — we do **NOT** invent a standalone `OpsDispatcher` (that abstraction fought the engine's `&mut`/state reality). The ops loop keeps `resolve_deferred_slots`'s shape — **`&self`, interior-mutable interceptor/defer state, the turn's `state`/`sink`/`ops_sender`/`ops_rx` threaded in as args** — but instead of mutating slots it **delivers resumes to per-call waiters**:

```
// concurrent, exactly like today's join!(resolve_deferred_slots, execute_tools_parallel):
let (_, results) = futures::join!(
    self.ops_loop(ops_rx, ops_state, sink, ops_sender, &waiters, cancel),  // evolved resolve loop → delivers to waiters
    run_batch(calls, &waiters),                                            // run_tool_call futures, order-preserving
);
```

`join!` is correct here because the ops loop is **bounded** (it already terminates on no-more-outstanding / channel-close / timeout / interrupt, as `resolve_deferred_slots` does) — it is NOT an unbounded task. `WaiterRegistry` (the one genuinely-new piece) is per-turn; `execute` is stateless (as `execute_tools_parallel` is today).

### Component 1 — `run_tool_call`: one self-contained future per call

```rust
async fn run_tool_call(call, ctx) -> ToolOutput {
    // 1. PERMISSION (decision + resolution)
    match ctx.policy.check(&call).await {
        Allow            => {}
        Deny { reason }  => return error_output(call.id, reason),
        AskUser { prompt } => match ctx.reviewer.review(approval_req(&call, prompt)).await {
            Approve         => {}                   // reviewer owns its I/O — does NOT touch ops
            Deny { reason } => return error_output(call.id, reason),
        }
    }
    // 2. INTERCEPTORS (pre). Any interceptor may return ToolDecision::Defer (the
    //    GENERIC defer protocol — ask_user AND planning AND external, not just ask_user).
    //    intercept_tool_call is `&mut self`, so lock the interior-mutable set BRIEFLY
    //    for the intercept call, then RELEASE the lock BEFORE awaiting the waiter:
    let decision = ctx.interceptors.lock().await.intercept_tool_call(call, ..).await?;  // lock dropped here
    match decision {
        Proceed(call)        => { /* execute */ }
        Defer { call_id }    => { let resume = ctx.waiters.register_and_wait(call_id, timeout, cancel).await?; /* resume */ }
        ShortCircuit(output) => return output,     // a defer can resolve straight to a result
    }
    // 3. EXECUTE
    ctx.emit(ToolStarted(call.id));
    let out = ctx.execute_tool(call).await;        // THE single sanctioned `.call(` site
    ctx.interceptors.post_tool_use(&out, &ctx).await;
    ctx.emit(ToolCompleted(call.id));
    out
}
```

Every wait (review, defer/resume) lives **inside** the call's own future, so the batch advances siblings whenever one suspends (order-preserving join / reassembly — see Component 4). Cross-call blocking is not expressible — structural immunity to the v1 bug class.

### Component 2 — the ops loop (evolved from `resolve_deferred_slots`)

This is **not** a new abstraction — it is today's `resolve_deferred_slots` evolved: same `&self`, same interior-mutable interceptor/defer state, the same `state`/`sink`/`ops_sender`/`ops_rx`/`permission_timeout` threaded in as args. The only change: instead of mutating `InterceptedSlot`s it **delivers resumes to a `WaiterRegistry`**, and it also subsumes the `Inject*`/`Interrupt` handling that `drain_ops` does today (single `ops_rx` consumer). Loop body per received op:

```rust
match op {
    // ── turn-level ──
    // Interrupt MIRRORS TODAY (zero behavior change): set the turn-level interrupt
    // flag applied at the iteration boundary (as `apply_op` does) AND, ONLY if there
    // are outstanding waiters, emit `Interrupted` + resolve them as errors (as today's
    // deferred-wait path). Do NOT eagerly cancel everything when there are no waiters.
    Interrupt            => { ops_state.set_interrupted(); if waiters.any_outstanding() { emit(Interrupted); waiters.error_all(Interrupted) } }
    InjectUserMessage(m) => turn_queue.push_message(m),      // applied at the next iteration boundary
    InjectHint(h)        => turn_queue.push_hint(h),         // (same)
    // ── per-call defer/resume: drive on_op (the MATCHER), deliver its resume to the waiter ──
    AskUserAnswer{..} | ApprovePlan{..} | ExtensionResume{..} => {
        // on_op is `&mut self` + needs `&mut state`, `sink`, `ops_sender`; the set is
        // interior-mutable, so lock BRIEFLY (this call only — never across a defer wait):
        match self.interceptors.lock().await.on_op(&op, state, sink, ops_sender.clone()).await {
            // EVERY OpDecision that resolves a deferred call must wake its waiter —
            // approve AND reject (planning's `on_op_resumes_with_rejection_and_feedback`
            // IS a resume). ⚠️ enumerate `decision.rs::OpDecision`; a resume left in the
            // fall-through arm hangs the call.
            Ok(d) if d.resolves_a_call() => waiters.deliver(d.call_id(), d.into_resume()),
            Ok(_ /* Pass / pure side-effect that wakes no call */) => {}
            Err(e) => self.fail_turn(e, cancel),  // Abort-policy error → terminal (see below)
        }
    }
}
// loop terminates like resolve_deferred_slots does: no outstanding waiters / channel-close / timeout / interrupt.
```

- **Real types (grounded in `ee9fec4`):** `InterceptorSet::on_op` and `LoopInterceptor::on_op` are **`&mut self`** and take `&mut AgentState` + `&mut sink` + `ops_sender`. So the set is held with **interior mutability** (a `Mutex`, as the engine already does for `self.deferred_calls`); on_op locks it. (My earlier "`Arc<InterceptorSet>` / `&self` / no-lock" was wrong — on_op is `&mut self`.)
- **Lock discipline = the v1 fix, correctly placed:** `on_op` (this loop) and `intercept_tool_call` (the per-call future) both `&mut` the set → both lock it **briefly, for that one call only**. They are **serialized** at that point — exactly as today's sequential pre-pass + single resolve loop already serialize them, so **no regression**. The concurrency win is that the **defer wait and tool execution happen OUTSIDE the lock** (the per-call future releases the lock, then awaits its waiter / executes). v1's bug was the *wait* being serialized at the batch resolver — that is what this fixes.
- **Route ALL resume variants (finding A):** any `OpDecision` that resolves a deferred call — approve, **reject**, etc. — must `deliver`. The fall-through is ONLY for decisions that wake no call. A resume in the fall-through = a hung call.
- **Matching stays in the interceptors** (explicit-id / wildcard-FIFO / pre-buffer / no-pending live in `on_op`) — keeps the existing tests green.
- **Dual registration, keyed by `call.id` (finding B):** a defer registers in **two** synced places — (1) the **interceptor's interior pending-state** (for *matching*), (2) the **`WaiterRegistry[call.id]`** (for *wakeup*). Flow: `intercept_tool_call` returns `Defer` (interceptor records pending) + the call registers its waiter; an answer op → `on_op` matches → returns the `call_id` → the loop delivers to `waiters[call_id]`. An **answer before the waiter is registered** is held by the interceptor's **pre-buffer** and matched on the next `on_op`.
- **Pending/defer ordering MUST be canonical tool-call order, not runtime registration order (A.2 finding #2).** Today's sequential pre-pass records pending defers in request order, and **wildcard-FIFO** matching ("answer with no `call_id` → oldest pending call") depends on that order. v2's per-call futures register CONCURRENTLY, so push-back/registration order is non-deterministic. To preserve the wildcard semantics: build a **canonical index map from `items` at batch start**, and order the pending-defer state by that index — NOT by which future happened to register first. Otherwise a wildcard answer matches the wrong call (a behavior change).
- **Error channel:** `on_op` `Err` only happens under `ErrorPolicy::Abort`; treat it as a **terminal turn error** (record + cancel, mirroring `AbortedByHook`), NOT log-and-continue.

### Component 3 — two waits, two owners (the decoupling)

| Wait | Answer source | Channel |
|---|---|---|
| permission review | the reviewer (host-owned; loop default `DenyReviewer` = no wait) | reviewer's OWN I/O — **never** ops/dispatcher (§9a) |
| `ask_user` extension | host, via `AgentOp::AskUserAnswer` | dispatcher → that call's waiter (`AgentOp`/`ExtensionEvent::AskUser` STAY, F6) |

They share no slot/resolve code. v1's root cause (shared machinery) is structurally gone.

> Note (Batch 0): `AgentOp::AskUserAnswer` has **two meanings** today — permission approval (the `DeferredPermission` slot) AND the `ask_user` extension. Batch B **removes the permission meaning** (it becomes `reviewer.review()`); the extension meaning stays, routed by the ops loop to the `ask_user` waiter.

### Component 4 — result ordering + streaming

**Ordering (Batch 0):** results must stay in **request/canonical order**, as today. Non-streaming uses order-preserving `join_all` + index-based reassembly (`combine_intercepted_slots`); streaming reassembles by id into `canonical_items` order. The new model keeps this: internally a `FuturesUnordered` for readiness is fine, but the externally visible results MUST be reassembled to request order before finalization (an unordered collection would be a behavior change).

**Streaming = same model:** streamed tool calls are just more `run_tool_call` futures (submitted as each complete `ToolUse` chunk arrives, still eagerly via `tokio::spawn`/`FuturesUnordered` so execution overlaps the LLM stream). Remove only the **duplicated approval/defer logic** from `streaming_executor.rs` — keep its streaming-specific chunk dedupe/ordering. No `ReviewPending` special case.

### Component 5 — cancellation

`Interrupt` op → dispatcher fires the turn `CancellationToken`; every in-call await (review, ask_user) races it; dropping the `FuturesUnordered` cancels the batch. One cancellation source instead of scattered handling.

### Component 6 — symbolic invariant

`tests/architectural_invariants.rs`: scan `engine.rs` for `fn execute_tool`'s signature + matching closing brace; assert the only `.call(` site is within that span (or require a `// sanctioned-tool-call-site` marker). No line-range widening.

### Why this is clean / maintainable

1. **One path** — every tool call (permission / ask_user / normal / streaming) goes through `run_tool_call`. A maintainer reads one function for a call's full lifecycle.
2. **One ops owner** — the ops loop (evolved `resolve_deferred_slots`) with an explicit routing table; a new op = one arm, no pipeline change.
3. **Local waits** — each wait lives in its owning call's future; `join` gives independence structurally (not via tests).

---

## Part 2 — Implementation, in batches

> Batches run in order, each gated by its own review. Within a batch, each numbered step is a commit. **If a step shows the model is wrong, STOP and revise Part 1, re-review — do not patch around it.**

### Batch 0 — Validate the model against `ee9fec4` ✅ DONE (2026-05-30)

Result: **[2026-05-30-task7-batch0-validation.md](2026-05-30-task7-batch0-validation.md)** (file:line evidence). Verdict: the per-call model holds, but Part 1 was under-specified. The 5 corrections are now folded into Part 1 above:
1. **Ops handling = an evolved `resolve_deferred_slots`, not a new standalone dispatcher** (`ops_rx` is per-run; the loop is `&self` + interior-mutable + threaded `state`/`sink`/`ops_sender`; it is bounded, so `join!(ops_loop, batch)` is correct as today). Corrected two earlier wrong guesses: "session-long dispatcher" and "`Arc<InterceptorSet>` / `&self` / no-lock" (on_op is `&mut self`).
2. **The ops loop routes all 6 `AgentOp` variants** — turn-level (Interrupt/Inject*) vs per-call (defer resumes); it subsumes `drain_ops`' `Inject*`/`Interrupt`.
3. **Generic defer protocol** (`ToolDecision::Defer` / resume variants) — not ask_user-only; planning uses it too. Route EVERY resume variant (approve AND reject).
4. **`on_op`/`intercept` are `&mut self`** on an interior-mutable set → brief lock for each call, released before the defer wait (serialized as today; wait+execute concurrent).
5. **Order-preserving** results, as today.

### Batch A — Structural rewrite, **ZERO behavior change** (the large, risky batch)

> Goal of Batch A: replace the two-phase pipeline with the `run_tool_call` futures + the evolved ops loop (from `resolve_deferred_slots`, delivering to a `WaiterRegistry`), with permission still `Allow`/`Deny` only (reviewer NOT consulted) — and the **entire existing test suite green, behavior identical**. This isolates "the big structural rewrite" from "the new feature" (Batch B). It is the anti-v1 discipline.

- [ ] **A.1 — `WaiterRegistry` (the one isolatable new piece; not wired).** Build a per-turn `WaiterRegistry`: `register_and_wait(call_id, timeout, cancel) -> Result<Resume, DeferError>` (per-call `oneshot`, racing timeout + cancel token) and `deliver(call_id, resume)` (wakes that call; an unmatched `deliver` is dropped/logged — matching lives in the interceptors, NOT here). NO `on_op`, NO interceptor handle, NO ops loop — those are A.2 (they evolve `resolve_deferred_slots`, which can't be built in isolation). Unit-test in isolation: `deliver` wakes a registered `register_and_wait`; timeout returns the timeout path; cancel returns the cancel path; deliver to an unregistered id is a no-op (no panic). NOT wired. Commit.
- [ ] **A.2 — evolve `resolve_deferred_slots` into the ops loop + `run_tool_call` (the large, irreversible batch).** Per Component 1/2: (a) turn the per-call flow into `run_tool_call` futures — permission `Allow`/`Deny` only for now (reviewer NOT consulted); `intercept_tool_call` under a **brief interior-mut lock** (released before awaiting the waiter); generic `ToolDecision::Defer` → `WaiterRegistry::register_and_wait`; execute. (b) Evolve `resolve_deferred_slots` into the **ops loop** (Component 2): keep its `&self` + interior-mut + threaded `state`/`sink`/`ops_sender`/`ops_rx` shape, subsume `drain_ops`' `Inject*`/`Interrupt`, drive `on_op` and **deliver EVERY resume variant** (approve AND reject) to the `WaiterRegistry`; `on_op` `Err` (Abort) → terminal. (c) Replace the slot pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)` with `join!(ops_loop, run_batch)`, results reassembled to request/canonical order. Remove the `InterceptedSlot` defer variants. **Acceptance: the full existing suite is green and behavior identical** — especially every `interceptors::ask_user` / `interceptors::planning` / `ask_user_e2e` / `defer_protocol` / `contract` / `interactive_ops` / `permission_*` test passes UNCHANGED (these are the spec). Because this is large + irreversible, do it as ordered sub-commits and **report at a checkpoint once the existing suite is green** before A.3/A.4.
- [ ] **A.3 — streaming onto `run_tool_call`.** Move `streaming_executor.rs` to push `run_tool_call` futures into the shared set; remove its review/defer special-casing. Existing streaming tests pass unchanged. Commit.
- [ ] **A.4 — symbolic invariant.** Replace the line-range allowlist with the `fn execute_tool`-span check. Commit.
- [ ] **Batch A review gate:** STOP and report. Confirm zero behavior change (whole suite green, ask_user/planning/defer semantics intact); `resolve_deferred_slots` is evolved into the waiter-delivering ops loop (sole `ops_rx` consumer); no `InterceptedSlot` defer variants remain. Only after this passes does Batch B start.

### Batch B — Wire the reviewer (small, the actual feature)

- [ ] **B.1 — `approval_request(&call, prompt)`** helper in `permission_runtime` (owned `ApprovalRequest`; prompt from `Permission::AskUser { prompt }`; engine cancellation_token). Commit.
- [ ] **B.2 — route `AskUser` → `reviewer.review()`** in `run_tool_call` step 1; `Approve` → continue, `Deny { reason }` → error_output. The reviewer is consulted on its OWN I/O — it does NOT register a dispatcher waiter (that's only the `ask_user` extension). Commit.
- [ ] **B.3 — migrate permission tests (R1).** Rewrite the §9d-listed permission-approval tests to drive a **test `Reviewer`** instead of feeding `AgentOp::AskUserAnswer`. Add: (a) AskUser+approve→runs; (b) AskUser+default `DenyReviewer`→blocked; (c) P4 non-blocking (Allow sibling unblocked by a pending review that never answers, then cancel); (d) P3 shared-reviewer serialization (two engines, one reviewer, recording reviewer asserts serial critical section); (e) event ordering (Approve → ToolStarted → ToolCompleted); (f) streaming approval. Commit.
- [ ] **B.4 — version `feat!` + CHANGELOG** (F7: AskUser with no reviewer now denies, was a stall; permission moved off the event/op protocol; ask_user extension unchanged). Commit.
- [ ] **Batch B review gate:** full suite green; ask_user extension suite unchanged; permission tests drive a `Reviewer`; new P3/P4/ordering/streaming tests pass. STOP — Phases 3/4 are separate.

---

## Hard constraints (apply to every batch)

- `ExtensionEvent::AskUser`, `AgentOp::AskUserAnswer`, and the entire `ask_user` extension test suite STAY and pass unchanged. Only the **permission** use of that machinery is removed (in Batch B).
- Loop ships only `DenyReviewer`; the interactive reviewer is host-owned (Phase 4 / agemo). Don't build one here.
- Don't touch primitives / subagent / agemo. P3 serialization is the reviewer impl's job (demonstrate via a test reviewer), not engine-side.

## Highest risk

Moving the **generic defer/resume** (`ToolDecision::Defer` → `ResumeDeferred`) from the batch resolver to the per-call dispatcher waiter (Batch A.2) — this covers the `ask_user` extension AND planning AND external extensions, not just ask_user. The existing defer/ask_user/planning test suites are the semantic spec (buffer / wildcard / no-pending / timeout / interrupt / pre-buffer); keep them ALL green throughout. If A.2 cannot preserve them on the new waiter mechanism, STOP and revise Part 1 before continuing.
