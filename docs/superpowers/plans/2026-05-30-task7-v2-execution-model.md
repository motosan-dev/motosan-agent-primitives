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

No pre-pass, no batch resolver, no `InterceptedSlot` zoo. Per **turn/run** (Batch 0 confirmed `ops_rx` is per-run, not per-session): a single **per-turn ops dispatcher** owns that run's `ops_rx` and lives across all the run's iterations/batches; each batch runs `run_tool_call` futures that register waiters with it. The dispatcher is a **scoped task stopped/aborted at turn terminal** — **NOT** `join!`'d with the batch (a `join!(dispatcher.run(rx), batch)` would hang, because the dispatcher loops on `rx` until close while `join!` waits for both):

```
// per turn:
let dispatcher = OpsDispatcher::spawn(ops_rx, cancel_token);   // sole ops_rx consumer for the run
... each batch: let results = run_batch(calls, &dispatcher).await;  // run_tool_call futures, order-preserving
dispatcher.shutdown();                                          // abort at turn terminal — do NOT join! it
```

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
    //    GENERIC defer protocol — ask_user AND planning AND external extensions use
    //    it, not just ask_user). On Defer, register a waiter[call.id] with the
    //    dispatcher and await ResumeDeferred (racing timeout + cancel).
    //    ⚠️ Do NOT hold the interceptor-set mutex across this await.
    match ctx.interceptors.pre_tool_use(call, &ctx).await {  // ctx exposes a dispatcher/waiter handle
        Continue(call)       => { /* execute */ }
        Defer                => { let answer = ctx.waiters.register_and_wait(call.id).await?; /* resume */ }
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

### Component 2 — per-turn ops dispatcher: the sole `ops_rx` consumer, routing all variants

The dispatcher **subsumes both** of today's op readers — `drain_ops` (try_recv at iteration boundaries) **and** `resolve_deferred_slots` (recv during deferred waits) — because `mpsc::Receiver` has a single consumer; they cannot coexist with it. It routes **every** `AgentOp` variant to one of **two destination kinds**:

```rust
match op {
    // ── turn-level (NOT per-call) ──
    Interrupt            => self.cancel_token.cancel(),          // cancel the turn
    InjectUserMessage(m) => self.turn_queue.push_message(m),     // applied at the next iteration boundary
    InjectHint(h)        => self.turn_queue.push_hint(h),        // (same)
    // ── per-call defer/resume: route THROUGH the interceptor on_op chain (the MATCHER) ──
    AskUserAnswer{..} | ApprovePlan{..} | ExtensionResume{..} => {
        // The interceptor SET owns the matching semantics (explicit call_id /
        // wildcard FIFO / pre-buffer / no-pending). on_op is SHORT — it takes the
        // interceptor-set lock briefly and NEVER awaits. The dispatcher only routes
        // its decision to the right per-call channel.
        match self.interceptors.on_op(&op) {        // dispatcher holds the interceptor-set handle
            Resume { call_id, result } => self.waiters.deliver(call_id, result),
            Buffered | NotMine         => {}         // matcher kept it / not a resume
        }
    }
}
```

- **Matching stays in the interceptors (do NOT re-implement it in the dispatcher).** Today `AskUserInterceptor::on_op` / `PlanningInterceptor::on_op` own explicit-id / wildcard-FIFO / pre-buffer / no-pending semantics. v2 keeps them there; the dispatcher just drives `on_op` and delivers its `Resume{call_id}` to that call's waiter. This is the lowest-risk path and what keeps the existing defer/ask_user/planning tests green.
- **`waiters`**: a per-turn registry `Mutex<HashMap<CallId, oneshot::Sender>>` — only the per-call wakeup channels, no matching logic.
- **Lock discipline (correctness core, no deadlock):** the **call** side (`run_tool_call` step 2) takes the interceptor-set lock, gets `Defer`, **releases the lock, THEN awaits** its waiter. The **dispatcher** side calls `on_op` holding the lock only **briefly and without awaiting**. Neither holds the interceptor-set lock across an `await`.
- **Two routing kinds:** `Interrupt`/`Inject*` are **turn-level** (cancel token / a pending message+hint queue applied at iteration boundaries), NOT per-call. `AskUserAnswer`/`ApprovePlan`/`ExtensionResume` are **per-call** resumes (matched by the interceptors, delivered by `call_id`). (Today `drain_ops`→`apply_op` does Inject*/Interrupt; `resolve_deferred_slots`→`on_op` does the defers — the dispatcher merges both.)

### Component 3 — two waits, two owners (the decoupling)

| Wait | Answer source | Channel |
|---|---|---|
| permission review | the reviewer (host-owned; loop default `DenyReviewer` = no wait) | reviewer's OWN I/O — **never** ops/dispatcher (§9a) |
| `ask_user` extension | host, via `AgentOp::AskUserAnswer` | dispatcher → that call's waiter (`AgentOp`/`ExtensionEvent::AskUser` STAY, F6) |

They share no slot/resolve code. v1's root cause (shared machinery) is structurally gone.

> Note (Batch 0): `AgentOp::AskUserAnswer` has **two meanings** today — permission approval (the `DeferredPermission` slot) AND the `ask_user` extension. Batch B **removes the permission meaning** (it becomes `reviewer.review()`); the extension meaning stays, routed by the dispatcher to the `ask_user` waiter.

### Component 4 — result ordering + streaming

**Ordering (Batch 0):** results must stay in **request/canonical order**, as today. Non-streaming uses order-preserving `join_all` + index-based reassembly (`combine_intercepted_slots`); streaming reassembles by id into `canonical_items` order. The new model keeps this: internally a `FuturesUnordered` for readiness is fine, but the externally visible results MUST be reassembled to request order before finalization (an unordered collection would be a behavior change).

**Streaming = same model:** streamed tool calls are just more `run_tool_call` futures (submitted as each complete `ToolUse` chunk arrives, still eagerly via `tokio::spawn`/`FuturesUnordered` so execution overlaps the LLM stream). Remove only the **duplicated approval/defer logic** from `streaming_executor.rs` — keep its streaming-specific chunk dedupe/ordering. No `ReviewPending` special case.

### Component 5 — cancellation

`Interrupt` op → dispatcher fires the turn `CancellationToken`; every in-call await (review, ask_user) races it; dropping the `FuturesUnordered` cancels the batch. One cancellation source instead of scattered handling.

### Component 6 — symbolic invariant

`tests/architectural_invariants.rs`: scan `engine.rs` for `fn execute_tool`'s signature + matching closing brace; assert the only `.call(` site is within that span (or require a `// sanctioned-tool-call-site` marker). No line-range widening.

### Why this is clean / maintainable

1. **One path** — every tool call (permission / ask_user / normal / streaming) goes through `run_tool_call`. A maintainer reads one function for a call's full lifecycle.
2. **One ops owner** — the dispatcher with an explicit routing table; a new op = one table arm, no pipeline change.
3. **Local waits** — each wait lives in its owning call's future; `join` gives independence structurally (not via tests).

---

## Part 2 — Implementation, in batches

> Batches run in order, each gated by its own review. Within a batch, each numbered step is a commit. **If a step shows the model is wrong, STOP and revise Part 1, re-review — do not patch around it.**

### Batch 0 — Validate the model against `ee9fec4` ✅ DONE (2026-05-30)

Result: **[2026-05-30-task7-batch0-validation.md](2026-05-30-task7-batch0-validation.md)** (file:line evidence). Verdict: the per-call model holds, but Part 1 was under-specified. The 5 corrections are now folded into Part 1 above:
1. **Dispatcher lifetime = per-turn** (ops_rx is per-run, not session) — scoped task aborted at turn terminal, NOT `join!`'d. (This corrected an earlier wrong "session-long" guess.)
2. **Dispatcher routes all 6 `AgentOp` variants** with turn-level (Interrupt/Inject*) vs per-call (defer resumes) destinations; it subsumes both `drain_ops` and `resolve_deferred_slots`.
3. **Generic defer protocol** (`ToolDecision::Defer` / `ResumeDeferred` / `ExtensionResume`) — not ask_user-only; planning uses it too.
4. **Interceptor ctx needs a waiter handle; must not hold the interceptor-set mutex while awaiting.**
5. **Order-preserving** results (join_all + reassembly), as today.

### Batch A — Structural rewrite, **ZERO behavior change** (the large, risky batch)

> Goal of Batch A: replace the two-phase pipeline with the dispatcher + `run_tool_call` model, with permission still `Allow`/`Deny` only (reviewer NOT consulted) and `ask_user` on the new waiter mechanism — and the **entire existing test suite green, behavior identical**. This isolates "the big structural rewrite" from "the new feature" (Batch B). It is the anti-v1 discipline.

- [ ] **A.1 — ops dispatcher + waiter registry.** Build `ops_dispatcher` as the single `ops_rx` consumer with the full routing table from Batch 0, and `waiters` (per-call oneshot map + unmatched-answer buffer). Not yet wired into execution. Unit-test: an `AskUserAnswer` for a registered call wakes it; for no/ wildcard call it buffers; `Interrupt` cancels. Commit.
- [ ] **A.2 — `run_tool_call` + order-preserving batch.** Introduce the per-call future (permission `Allow`/`Deny` only for now; interceptor dispatch handling the **generic `ToolDecision::Defer`** via the dispatcher waiter — covers ask_user AND planning; execute). Replace the sequential pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)` with: the **scoped per-turn dispatcher** (A.1) running alongside an order-preserving batch of `run_tool_call` futures, with results reassembled to request order and the **dispatcher aborted at turn terminal (NOT `join!`'d)**. Remove `resolve_deferred_slots` and the `InterceptedSlot` defer variants. **Acceptance: the full existing suite is green and behavior is identical** — especially every `interceptors::ask_user` / `interceptors::planning` / `ask_user_e2e` / `defer_protocol` / `contract` / `interactive_ops` / `permission_*` test passes UNCHANGED (these are the spec for the waiter mechanism). Commit.
- [ ] **A.3 — streaming onto `run_tool_call`.** Move `streaming_executor.rs` to push `run_tool_call` futures into the shared set; remove its review/defer special-casing. Existing streaming tests pass unchanged. Commit.
- [ ] **A.4 — symbolic invariant.** Replace the line-range allowlist with the `fn execute_tool`-span check. Commit.
- [ ] **Batch A review gate:** STOP and report. Confirm zero behavior change (whole suite green, ask_user semantics intact), the dispatcher is the sole `ops_rx` owner, and no `InterceptedSlot` defer / `resolve_deferred_slots` remain. Only after this passes does Batch B start.

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
