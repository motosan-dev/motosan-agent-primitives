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

No pre-pass, no batch resolver, no `InterceptedSlot` zoo. Two concurrent things per turn:

```
turn = join!(
    ops_dispatcher.run(ops_rx),                    // SINGLE owner of ops_rx
    run_all_calls(),                               // FuturesUnordered<run_tool_call(call)>
)
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
    // 2. INTERCEPTORS (pre). If this tool is `ask_user`, the interceptor registers
    //    a waiter[call.id] with the dispatcher and awaits it (racing timeout + cancel).
    match ctx.interceptors.pre_tool_use(call, &ctx).await {
        Continue(call)       => { /* execute */ }
        ShortCircuit(output) => return output,     // e.g. ask_user resolves to a result here
    }
    // 3. EXECUTE
    ctx.emit(ToolStarted(call.id));
    let out = ctx.execute_tool(call).await;        // THE single sanctioned `.call(` site
    ctx.interceptors.post_tool_use(&out, &ctx).await;
    ctx.emit(ToolCompleted(call.id));
    out
}
```

Every wait (review, ask_user answer) lives **inside** the call's own future, so `FuturesUnordered` advances siblings whenever one suspends. Cross-call blocking is not expressible — structural immunity to the v1 bug class.

### Component 2 — ops dispatcher: the single `ops_rx` consumer + explicit routing table

```rust
async fn ops_dispatcher.run(ops_rx) {
    while let Some(op) = ops_rx.recv().await {
        match op {
            AskUserAnswer { call_id, answer } => self.waiters.deliver(call_id, answer), // wake THAT call; buffer if none
            Interrupt                          => self.cancel_token.cancel(),
            // … every other AgentOp variant gets an explicit arm (enumerate them all in Batch 0)
        }
    }
}
```

- `waiters: Mutex<HashMap<CallId, oneshot::Sender<Answer>>>` + a buffer for unmatched answers (replicates today's `ask_user` no-pending / wildcard semantics).
- Splits today's `resolve_deferred_slots` (which couples "drain ops" + "resolve batch slots") into **dispatcher routes ops** + **each call awaits its own waiter** — clean separation of concerns.

### Component 3 — two waits, two owners (the decoupling)

| Wait | Answer source | Channel |
|---|---|---|
| permission review | the reviewer (host-owned; loop default `DenyReviewer` = no wait) | reviewer's OWN I/O — **never** ops/dispatcher (§9a) |
| `ask_user` extension | host, via `AgentOp::AskUserAnswer` | dispatcher → that call's waiter (`AgentOp`/`ExtensionEvent::AskUser` STAY, F6) |

They share no slot/resolve code. v1's root cause (shared machinery) is structurally gone.

### Component 4 — streaming = same model

Streamed tool calls are just more `run_tool_call` futures pushed into the **same** `FuturesUnordered`. No `ReviewPending` special case in `streaming_executor.rs`. Streaming's only concern stays output-chunk ordering, not approval.

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

### Batch 0 — Validate the model against `ee9fec4` (no production code)

- [ ] **0.1** Map today's real flow precisely: where the sequential pre-pass is, what `InterceptedSlot` variants exist, who owns `ops_rx` today and **every** `AgentOp` variant + how each is handled (so the dispatcher's routing table is complete — this is the gap that sank my first draft).
- [ ] **0.2** Confirm `run_tool_call` can own the `ask_user` interceptor wait: where does interceptor dispatch happen today, and can it move inside the per-call future and await a dispatcher-delivered answer while preserving the ask_user suite's semantics (buffer / wildcard / no-pending / timeout / interrupt)?
- [ ] **0.3** Confirm the streaming-eager path can run `run_tool_call` per streamed call instead of its `tokio::spawn` special case.
- [ ] **Output:** a short validation note. If anything in Part 1 needs adjusting (e.g. an op type that can't be cleanly routed), revise Part 1 and re-review BEFORE Batch A. No code lands in Batch 0.

### Batch A — Structural rewrite, **ZERO behavior change** (the large, risky batch)

> Goal of Batch A: replace the two-phase pipeline with the dispatcher + `run_tool_call` model, with permission still `Allow`/`Deny` only (reviewer NOT consulted) and `ask_user` on the new waiter mechanism — and the **entire existing test suite green, behavior identical**. This isolates "the big structural rewrite" from "the new feature" (Batch B). It is the anti-v1 discipline.

- [ ] **A.1 — ops dispatcher + waiter registry.** Build `ops_dispatcher` as the single `ops_rx` consumer with the full routing table from Batch 0, and `waiters` (per-call oneshot map + unmatched-answer buffer). Not yet wired into execution. Unit-test: an `AskUserAnswer` for a registered call wakes it; for no/ wildcard call it buffers; `Interrupt` cancels. Commit.
- [ ] **A.2 — `run_tool_call` + `FuturesUnordered` batch.** Introduce the per-call future (permission `Allow`/`Deny` only for now; interceptor dispatch incl. `ask_user` via the dispatcher waiter; execute). Replace the pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)` with `join!(dispatcher.run, FuturesUnordered<run_tool_call>)`. Remove `resolve_deferred_slots` and the `InterceptedSlot` defer variants. **Acceptance: the full existing suite is green and behavior is identical** — especially every `interceptors::ask_user` / `ask_user_e2e` / `defer_protocol` / `contract` / `permission_*` test passes UNCHANGED. (The `ask_user` tests are the spec for the waiter mechanism — keep them green.) Commit.
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

The `ask_user` interceptor's resume mechanism changing from "batch resolver" to "per-call dispatcher waiter" (Batch A.2) — its test suite is the semantic spec; keep it green throughout. If A.2 cannot keep the ask_user suite green with the new waiter mechanism, STOP and revise Part 1 before continuing.
