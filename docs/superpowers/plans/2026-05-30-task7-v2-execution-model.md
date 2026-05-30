# Task 7 v2 — Per-Call Execution Model (design-first)

> **For agentic workers:** this is the **design-first** artifact the botched v1 attempt lacked. **Do not write production code until the "Execution model" below is reviewed and approved.** Then implement in the ordered steps; do NOT patch incrementally on the shared pipeline (that is exactly what failed).

**Goal:** Wire the (dormant) `Reviewer` into the permission path (spec §9a) by **rebuilding** the loop's tool-execution into independent per-call futures — so permission-review, the `ask_user` extension defer, streaming, and execution no longer entangle at the batch level.

**Parent:** [2026-05-29-reviewer-approval-seam.md](2026-05-29-reviewer-approval-seam.md) Phase 2 Task 7. **Spike:** spec §9d. **Baseline:** loop@`ee9fec4` (clean Task 6, dormant Reviewer API).

---

## Why v1 failed (the lesson, so v2 doesn't repeat it)

v1 patched a **shared, batch-level** concurrency structure one blocking-point at a time. Each fix exposed another wait that was tangled into the same batch:

- permission review stopped blocking allowed tools → but made the `ask_user` extension defer wait for a sibling tool to finish;
- fixing that → changed batch spawn/cancellation semantics;
- fixing streaming review → still let a pending permission review block an extension-deferred sibling's resume.

**Root cause:** today's engine is two-phase — a slot **pre-pass** (`dispatch_tool_call_to_slot` → `consult_policy` → `InterceptedSlot`) then a **batch-level** `join!(resolve_deferred_slots, execute_tools_parallel)`. Multiple independent waits (permission review, ask_user defer, streaming chunks) are multiplexed by one batch-level resolver, so they cross-block. You cannot fix that by patching; the batch-level resolver must go.

**v1 is preserved at branch `wip/task7-v1-botched` (loop)** for reference; it is NOT to be built on.

---

## Execution model (REVIEW THIS BEFORE CODING)

### The one rule

**Each tool call is one self-contained `async` future that runs its entire lifecycle. The batch only `join_all`s them. There is no batch-level resolve phase.**

```rust
// one future per tool call — owns ALL of its own waits
async fn run_tool_call(ctx, call) -> ToolOutput {
    // 1. PERMISSION (decision + resolution)
    match policy.check(&call).await {
        Allow            => { /* fall through to 2 */ }
        Deny { reason }  => return error_output(reason),
        AskUser { prompt } => match ctx.reviewer.review(approval_request(&call, prompt)).await {
            Approve         => { /* fall through to 2 */ }
            Deny { reason } => return error_output(reason),
        }
    }
    // 2. INTERCEPTOR DISPATCH — incl. the ask_user EXTENSION wait, if any,
    //    awaited HERE inside this call's future via a per-call answer channel (see ops router)
    let call = interceptors.dispatch(call, ctx).await?;   // may await this call's own ask_user answer
    // 3. EXECUTE
    emit(ToolStarted(call.id));
    let output = execute_tool(&call, ctx).await;           // the single sanctioned `.call(` site
    emit(ToolCompleted(call.id));
    output
}

// batch: just join the independent per-call futures
let outputs = join_all(calls.into_iter().map(|c| run_tool_call(ctx.clone(), c))).await;
```

Because every wait (permission review, ask_user answer) lives **inside** the call's own future, `join_all` advances siblings whenever one suspends. No cross-call blocking is possible by construction — that is the whole point, and what v1's batch resolver violated.

### Two waits, two owners — keep them separate

| Wait | Who provides the answer | Channel |
|---|---|---|
| **permission review** (`AskUser`→reviewer) | the **reviewer** (host-owned; loop default = `DenyReviewer`, no wait) | the reviewer's OWN I/O — **NOT** engine ops (§9a) |
| **`ask_user` extension** (agent asks user mid-turn) | the host, via `AgentOp::AskUserAnswer` | the engine ops router → this call's per-call answer channel (STAYS, F6) |

The permission path **stops using** `ExtensionEvent::AskUser` / `AgentOp::AskUserAnswer`. The `ask_user` extension **keeps** them. They must not share a code path anymore.

### Ops router (replaces `resolve_deferred_slots`)

The batch-level resolver is deleted. In its place, a small router so an incoming op wakes the **specific** call waiting for it:

- A per-session `pending: Mutex<HashMap<CallId, oneshot::Sender<Answer>>>`.
- When `run_tool_call`'s interceptor step needs an `ask_user` answer, it registers a `oneshot` under `call.id` and `await`s the receiver (racing `cancellation_token`).
- One task drains `ops_rx`; on `AgentOp::AskUserAnswer { call_id, answer }` it looks up `pending[call_id]` and sends — waking exactly that call. Unmatched answers buffer (preserve today's wildcard/no-pending behavior).
- No call ever waits on another call's op.

### Streaming uses the SAME model (no `ReviewPending` special case)

The streaming-eager path (`src/streaming_executor.rs`) currently `tokio::spawn`s per chunk. v2: each streamed tool call runs the **same** `run_tool_call` future (spawned or pushed into a `FuturesUnordered`), collected as results arrive. No separate review-pending state — permission review is just step 1 of the shared future. The only streaming-specific concern is ordering of emitted chunks, not a separate approval mechanism.

### Symbolic architectural invariant (replace the line-range hack)

`tests/architectural_invariants.rs` must stop using a line-number range. Replace with a check that every `.call(` (the `is_tool_call_bypass_pattern`) in `engine.rs` lies **inside the body of `fn execute_tool`** — e.g. scan for the `fn execute_tool` signature and its matching closing brace and assert the only matched call site is within that span (or require an explicit `// sanctioned-tool-call-site` marker on the one line). No more widening a range.

---

## Implementation steps (only after the model above is approved)

> Each step is a commit. If a step reveals the model is wrong, STOP and revise the model doc — do not patch around it.

- [ ] **Step 0 — validate the model against `ee9fec4` code.** Confirm `run_tool_call` can own the ask_user-extension wait (the interceptor dispatch currently happens where?), and that `ops_rx` can be drained by a single router without losing the extension's buffering/wildcard semantics. Write findings; if the model needs adjustment, revise THIS doc and re-review before Step 1.
- [ ] **Step 1 — `approval_request(&call, prompt)` helper** in `permission_runtime` (owned `ApprovalRequest`). Commit.
- [ ] **Step 2 — introduce `run_tool_call` + the ops router**, but keep behavior identical first: ask_user extension via the per-call channel, permission still `Allow/Deny` only (reviewer not yet consulted). Get the WHOLE existing suite green on the new structure BEFORE adding reviewer review. This isolates "restructure" from "new behavior". Commit.
- [ ] **Step 3 — route `AskUser` → `reviewer.review()`** inside `run_tool_call` step 1; map Approve→continue, Deny→error. Remove the permission use of `ExtensionEvent::AskUser`/`DeferredPermission`. Commit.
- [ ] **Step 4 — streaming path** onto `run_tool_call`. Commit.
- [ ] **Step 5 — migrate permission tests** to drive a test `Reviewer` (the §9d list). Add the P4 non-blocking, P3 (shared-reviewer) via a test reviewer, event-ordering, streaming-approval tests. Commit.
- [ ] **Step 6 — symbolic invariant** + version `feat!` + CHANGELOG (F7: default now denies). Commit.

**Key sequencing insight (the anti-v1):** Step 2 lands the *structural* rewrite with **zero behavior change** and the full existing suite green. Only Step 3 adds the reviewer. That separation is what v1 skipped — it mixed restructure + new behavior + fixes in one tangled sweep.

## Hard constraints (unchanged from §9d / Task 7)

- `ExtensionEvent::AskUser`, `AgentOp::AskUserAnswer`, and ALL `interceptors::ask_user` / `ask_user_e2e` / `defer_protocol` / `contract` ask_user tests STAY and pass UNCHANGED. Only the *permission* use of that machinery is removed.
- Loop ships only `DenyReviewer`; interactive reviewer is host-owned (Phase 4 / agemo). Don't build one here.
- Don't touch primitives / subagent / agemo. P3 serialization is the reviewer impl's job (demonstrate with a test reviewer), not engine-side.

## Acceptance

- `cargo build/test --locked --all-features` green.
- ask_user extension suite passes unchanged.
- Permission tests drive a `Reviewer` (no `AgentOp::AskUserAnswer` feeding for permission).
- New: P4 non-blocking, P3 shared-reviewer, event-ordering, streaming-approval.
- `architectural_invariants.rs` is symbolic (no line range).
