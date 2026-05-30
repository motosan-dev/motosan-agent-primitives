# Task 7 v2 Batch 0 — execution-model validation note

Baseline verified: `motosan-agent-loop` on `main` at `ee9fec4 (feat(loop): add DenyReviewer + EngineBuilder::reviewer() (dormant until Task 7))`.

## Q1 — `ops_rx` scope / lifetime

`ops_rx` is **per run/turn**, not per session.

Evidence:

- `RunBuilder` owns `ops_rx: Option<mpsc::Receiver<AgentOp>>` and `Engine::run(...)` initializes it to `None` for each new run: `src/core/engine.rs:4731-4736`, `src/core/engine.rs:5331-5338`.
- `.ops(rx)` attaches a receiver to that one `RunBuilder`; calling it twice overwrites the first receiver: `src/core/engine.rs:5270-5276`.
- `RunBuilder` moves the receiver into one selected inner run path: `src/core/engine.rs:4816-4868`.
- `AgentSession` creates a fresh `(ops_tx, ops_rx)` per turn/fork/cancellable turn, passes `ops_rx` into `.ops(ops_rx)`, and returns only `ops_tx` in `TurnHandle`: `src/session/session.rs:23-25`, `src/session/session.rs:209-220`, `src/session/session.rs:260-274`, `src/session/session.rs:326-338`, `src/session/session.rs:434-452`.
- The inner run paths create a fresh internal per-run channel and forward the external receiver into it: batch path `src/core/engine.rs:1769-1799`, batch+cancel `src/core/engine.rs:2022-2047`, streaming path `src/core/engine.rs:2709-2741`.

Conclusion: a future dispatcher should be **per turn/run**, not session-long. It may live across all iterations/batches inside that run, with batches registering waiters, but it must be stopped/cancelled at turn completion. A literal `join!(dispatcher.run(rx), run_all_calls())` will hang unless the rx closes; the dispatcher lifecycle must be `select/scoped task + abort/drop on turn terminal`, not an unbounded joined future.

## Q2 — ops consumers and `AgentOp` variants

`AgentOp` variants are: `Interrupt`, `InjectUserMessage`, `InjectHint`, `AskUserAnswer`, `ApprovePlan`, `ExtensionResume` (`src/core/engine.rs:369-402`). There is no `UserInput` variant in this codebase.

Consumers/readers today:

1. Per-run forwarding tasks consume the externally supplied receiver and forward into the internal receiver: `src/core/engine.rs:1780-1790`, `src/core/engine.rs:2032-2046`, `src/core/engine.rs:2722-2733`.
2. `drain_ops` consumes immediately available ops with `try_recv()` at iteration boundaries, first routing through `dispatch_on_op`, then `apply_op`: `src/core/engine.rs:3400-3449`.
3. `resolve_deferred_slots` consumes ops with `rx.recv()` while deferred slots exist: `src/core/engine.rs:3919-3956`.

Variant handling:

- `Interrupt`: in `apply_op`, marks `ops_state.interrupted = true` (`src/core/engine.rs:3451-3455`); during deferred wait, emits `CoreEvent::Interrupted`, resolves remaining deferrals as errors, and breaks (`src/core/engine.rs:4133-4151`).
- `InjectUserMessage`: appends a user message only in `apply_op` (`src/core/engine.rs:3456-3458`). During deferred waits it falls into the `_ => other ops not handled` arm (`src/core/engine.rs:4158-4160`).
- `InjectHint`: appends `[Note: ...]` only in `apply_op` (`src/core/engine.rs:3459-3460`); likewise not handled during deferred waits.
- `AskUserAnswer`: currently has two meanings. Permission approval is handled directly by `resolve_deferred_slots` when the slot is `DeferredPermission` (`src/core/engine.rs:4009-4060`). The `ask_user` extension handles it via `on_op` and returns `ResumeDeferred` or buffers it (`src/interceptors/ask_user/interceptor.rs:199-241`). If it reaches `apply_op`, it is a no-op (`src/core/engine.rs:3462-3466`).
- `ApprovePlan`: handled by `PlanningInterceptor::on_op`, either resuming pending `exit_plan_mode` or pre-buffering approval (`src/interceptors/planning/interceptor.rs:174-204`). If it reaches `apply_op`, it is a no-op (`src/core/engine.rs:3467-3470`).
- `ExtensionResume`: direct engine path inside `resolve_deferred_slots` resolves an `InterceptedSlot::Deferred` by call id after first dispatching `on_op` for side effects (`src/core/engine.rs:4065-4095`). If it reaches `apply_op`, it warns/ignores (`src/core/engine.rs:3472-3479`). `HookCtx` exposes `ops_sender()` specifically so extensions can spawn tasks that send `ExtensionResume`: `src/core/hook_ctx.rs:123-130`.

Assessment: consolidating all op reads into one dispatcher is coherent, but only if it has explicit routes for **turn-level ops** as well as per-call waiters. `InjectUserMessage`/`InjectHint` should not be per-call waiters; they need a turn-level pending-message/hint queue applied at iteration boundaries. `Interrupt` should cancel the turn token. `AskUserAnswer`, `ApprovePlan`, and `ExtensionResume` need waiter/deferral routing. Because `mpsc::Receiver` has one consumer, these cannot remain in separate `drain_ops`/`resolve_deferred_slots` readers after the dispatcher exists.

## Q3 — current batch ↔ ops relationship / shutdown

There is no session-level op loop. Ops are read only inside the selected run's inner function.

Current batch behavior:

- Tool calls are first converted to slots by a sequential pre-pass in `execute_tools_with_policy`: `for item in items { dispatch_tool_call_to_slot(...) }` (`src/core/engine.rs:3489-3516`).
- Normal tools and deferred-slot resolution then run concurrently via `futures::join!(resolve_deferred_slots(...), execute_tools_parallel(...))`: `src/core/engine.rs:4205-4222`.
- `resolve_deferred_slots` loops until all deferred slots are resolved (`src/core/engine.rs:3859-3862`), or timeout (`src/core/engine.rs:3871-3915`), ops channel close (`src/core/engine.rs:3927-3950`), no ops channel (`src/core/engine.rs:3958-4005`), or interrupt (`src/core/engine.rs:4133-4151`). It does **not** stop because normal tool execution finished; the surrounding `join!` waits for both halves.

Correction implied: a new dispatcher must be scoped to the turn and stopped independently when the turn terminal path runs; batches should register waiters and await per-call futures, not `join!` an infinite dispatcher.

## Q4 — moving `ask_user` wait into per-call future

Today `ask_user` runs as an interceptor in `dispatch_intercept_tool_calls`, after permission allows a call:

- Permission Allow emits `ToolStarted`, then calls `dispatch_intercept_tool_calls`: `src/core/engine.rs:3562-3574`.
- `AskUserInterceptor::intercept_tool_call` detects `call.name == "ask_user"`, emits `AskUserEvent::Question`, consumes any pre-queued answer, otherwise records pending state and returns `ToolDecision::Defer`: `src/interceptors/ask_user/interceptor.rs:143-193`.
- `resolve_deferred_slots` later reads ops and dispatches `on_op`; `AskUserInterceptor::on_op` matches by explicit call id, wildcard FIFO, or buffers no-pending answers: `src/interceptors/ask_user/interceptor.rs:199-241`; generic `ResumeDeferred` fills matching `InterceptedSlot::Deferred`: `src/core/engine.rs:4097-4123`.
- Timeout calls `on_defer_timeout`, and `AskUserInterceptor` emits `AskUserEvent::Timeout`: `src/core/engine.rs:3871-3901`, `src/interceptors/ask_user/interceptor.rs:244-281`.

Can it move inside one per-call future? Yes, but not by simply awaiting inside the current shared `InterceptorSet` lock. The new per-call/interceptor context must expose a **per-turn waiter registry** (register by call id, await answer/result, support wildcard FIFO + explicit pre-buffer + timeout + cancellation), and interceptor dispatch must not hold the whole interceptor-set mutex while a call waits. Current `HookCtx` exposes only `ops_sender()` (`src/core/hook_ctx.rs:84-130`), so the new context needs additional wait/register APIs (or a dispatcher handle). This belongs in interceptor context more than `motosan_agent_tool::ToolContext`, which is only passed to the final `tool.call(...)` site.

Semantics/tests to preserve:

- Unit: defer + question event (`src/interceptors/ask_user/tests.rs:69`), explicit answer resumes (`:124`), explicit no-pending buffers (`:173`), wildcard no-pending buffers (`:201`), unrelated op passes (`:249`).
- E2E/no channel: `tests/ask_user_e2e.rs:103`, `tests/ask_user_e2e.rs:174`.
- Multi/prequeue: `tests/defer_protocol.rs:331`, `tests/defer_protocol.rs:448`.
- Contract: explicit matching (`tests/contract.rs:139`), wildcard FIFO (`:213`), interrupt during ask_user (`:311`), timeout event (`:386`), no ops channel immediate timeout (`:438`).
- Interactive concurrency: `tests/interactive_ops.rs:268`, `:339`, `:398`, `:510`, `:735`, `:878`.

## Q5 — result ordering

Non-streaming batch execution is order-preserving:

- `execute_tools_parallel` uses `join_all(items.iter().map(...))`, whose output vector is in input order: `src/core/engine.rs:4169-4181`.
- `resolve_and_execute_intercepted_slots` records `pending_indices` and writes late results back by slot index: `src/core/engine.rs:4196-4202`, `src/core/engine.rs:4237-4247`.
- `combine_intercepted_slots` reconstructs `final_pairs` indexed by original slot order and unzips in that order: `src/core/engine.rs:4387-4423`.

Streaming eager execution is also reassembled deterministically:

- `StreamingToolExecutor` starts already-authorized futures as complete streamed tool-use blocks arrive; it stores `(ToolCallItem, JoinHandle)` in a vec and `collect()` returns in submission order: `src/streaming_executor.rs:1-7`, `src/streaming_executor.rs:20-23`, `src/streaming_executor.rs:34-44`, `src/streaming_executor.rs:52-62`.
- The streaming path deduplicates streamed `ToolUse` chunks by id, dispatches permission/interceptors, and submits pending executions immediately: `src/core/engine.rs:1148-1195` and `src/core/engine.rs:3170-3195`.
- Final streaming reassembly maps combined results by id and emits results in `canonical_items` order: `src/core/engine.rs:4354-4381`.

Correction implied: use **order-preserving join/reassembly**. Internally you can use `FuturesUnordered` for readiness, but the public batch result must be restored to request/canonical order before finalization. A simple unordered collection would be a behavior change.

## Q6 — streaming

The streaming-eager path can use the same per-call `run_tool_call` future, but it must keep streaming-specific behavior:

- Streaming-specific: process text/thinking/usage/stop chunks in stream order, dedupe complete `ToolUse` chunks by id, start eligible tool futures before stream end, and later reassemble to canonical order (`src/core/engine.rs:1148-1195`, `src/core/engine.rs:3170-3195`, `src/core/engine.rs:4354-4381`).
- Approval/defer-specific logic is not intrinsic to `StreamingToolExecutor`; it currently states permission/interceptor dispatch happens before submit and the executor owns only task scheduling/ordered collection (`src/streaming_executor.rs:1-7`, `src/streaming_executor.rs:34-38`).

So the special case can be removed if `run_tool_call` is future-based and can be submitted when each complete streamed call arrives. The executor may still need `tokio::spawn` or a `FuturesUnordered` to preserve eager execution while the LLM stream continues; what should disappear is duplicated approval/defer logic, not streaming chunk ordering.

## Verdict

Part 1's high-level model holds **with required corrections**; it does not need a different architecture, but the current text is under-specified enough to be unsafe as written.

Required corrections:

1. **Dispatcher lifetime (Q1):** make it **per turn/run**, not session-long. It owns that run's internal `ops_rx`, lives across all batches/iterations of the run, and is stopped/aborted at turn terminal. Do not `join!` an infinite dispatcher with a batch; use a scoped task/select/cancellation pattern.
2. **Dispatcher routing (Q2):** include all variants, not just `AskUserAnswer`: `Interrupt` cancels; `InjectUserMessage`/`InjectHint` go to a turn-level queue; `AskUserAnswer` routes to ask_user waiters/buffers (permission use removed in Batch B); `ApprovePlan` routes to planning/defer waiter or buffer; `ExtensionResume` routes to a deferred-result waiter and preserves side-effect `on_op` dispatch.
3. **Generic defer protocol:** Part 1 should generalize waiters beyond `ask_user` to the stable `ToolDecision::Defer` / `OpDecision::ResumeDeferred` / `AgentOp::ExtensionResume` protocol, because planning and external extensions use it today.
4. **Ask_user context:** expose a dispatcher/waiter handle in the interceptor context and avoid holding the interceptor-set mutex while awaiting a user answer.
5. **Batch primitive (Q5):** use an order-preserving primitive or reassemble after `FuturesUnordered`; externally visible tool results/messages must remain in request/canonical order.
