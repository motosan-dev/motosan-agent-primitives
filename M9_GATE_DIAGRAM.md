# M9 Step 0 — Sequence Diagram Gate

Precondition check performed 2026-05-28:

- Seven repos named in the task were on `main`, equal to `origin/main`, and clean before this document was written: `motosan-agent-primitives`, `motosan-agent-tool`, `motosan-agent-loop`, `motosan-ai`, `motosan-agent-subagent`, `motosan-agent-harness`, `agemo`.
- `motosan-agent-loop` is `0.25.0`; `cargo build --all-features` passed.
- `motosan-agent-subagent` is `0.4.0`; `cargo build --all-features` passed.
- Note: the prompt says "all 8 repos" but lists seven repos. `motosan-agent-harness-finance` does not exist yet, as expected for M9 Step 0.

## §1. The scenario

User prompt:

> buy 10 AAPL if under $200

Acceptance criteria for the M9 demo trace:

1. The agent calls a quote tool for AAPL.
2. The agent compares the quote to `$200`.
3. If the quote is below `$200`, the agent requests user approval before placing the order.
4. On approval, the order tool executes the buy order.
5. Audit output records tool-call outcomes, the approval prompt, and the final result.
6. The final assistant answer is returned to the user.

## §2. The harness composition

`FinanceHarness` implements `motosan_agent_harness::Harness` and contributes the five fields currently wired by `motosan-agent-loop` 0.25.0:

| Harness field | Existing API used | Finance value for M9 demo |
|---|---|---|
| Tools | `Harness::tools() -> Vec<Arc<dyn Tool>>`; wired by `EngineBuilder::tools(...)` | Minimum needed: `finance.get_quote`, `finance.place_order`. Optional later: `finance.get_position`, `finance.backtest`. |
| System prompt | `Harness::system_prompt() -> Option<String>`; wired by `EngineBuilder::system_prompt(...)` | Finance-domain persona: obey user thresholds, quote before trading, never call `place_order` without approval. |
| Hooks | `Harness::hooks() -> Vec<Arc<dyn Hook>>`; wired by `EngineBuilder::hooks(...)` and wrapped by `HookInterceptorAdapter::new(...)` in `EngineBuilder::build()` | `AuditLogHook`, implementing `Hook::post_tool_use` and `Hook::post_tool_use_failure`; can also implement `session_start`, `stop`, `session_end` for lifecycle records. |
| Permission policy | `Harness::permission_policy() -> Option<Arc<dyn PermissionPolicy>>`; wired by `EngineBuilder::permission_policy(...)` | `FinanceApprovalPolicy`: `place_order` returns `Permission::AskUser { prompt: Some(...) }`; read-only tools return `Permission::Allow`. |
| Memory schema | `Harness::memory_schema() -> Option<MemorySchema>`; wired by `EngineBuilder::memory_schema(...)` | `None` for this demo; no persistent memory is required. |

Tool inventory and policy results:

| Tool | Required for demo? | `ToolAnnotations` | `FinanceApprovalPolicy::check(&PermissionContext)` result |
|---|---:|---|---|
| `finance.get_quote` | Yes | `ToolAnnotations { read_only: true, destructive: false, network_access: true, idempotent: true }` | `Permission::Allow` |
| `finance.place_order` | Yes | `ToolAnnotations { read_only: false, destructive: true, network_access: true, idempotent: false }` | `Permission::AskUser { prompt: Some("Approve buy 10 AAPL ...?") }` |
| `finance.get_position` | No | `ToolAnnotations { read_only: true, destructive: false, network_access: true, idempotent: true }` | `Permission::Allow` |
| `finance.backtest` | No | `ToolAnnotations { read_only: true, destructive: false, network_access: false, idempotent: true }` | `Permission::Allow` |

Harness wiring shape, matching `agemo/src/main.rs`:

```rust
let harness: Arc<dyn Harness> = Arc::new(FinanceHarness::new(...));
let mut builder = Engine::builder()
    .tools(harness.tools())
    .hooks(harness.hooks());
if let Some(prompt) = harness.system_prompt() {
    builder = builder.system_prompt(prompt);
}
if let Some(policy) = harness.permission_policy() {
    builder = builder.permission_policy(policy);
}
if let Some(schema) = harness.memory_schema() {
    builder = builder.memory_schema(schema);
}
let engine = Arc::new(builder.build());
```

All functions/types above exist today: `Harness`, `Engine::builder`, `EngineBuilder::tools`, `EngineBuilder::hooks`, `EngineBuilder::system_prompt`, `EngineBuilder::permission_policy`, `EngineBuilder::memory_schema`, and `EngineBuilder::build`.

## §3. End-to-end sequence diagram

The loop's public event stream is `motosan_agent_loop::AgentEvent::{Core, LoopInterceptor}`. A host may map those to the wire-format `motosan_agent_primitives::AgentEvent` exactly as `agemo::handle_event` does, but the core M9 execution path below names the actual loop functions that run the demo.

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant Host as Finance demo host / runner
    participant Harness as FinanceHarness: Harness
    participant Builder as EngineBuilder
    participant Engine as Arc<Engine> / RunBuilder
    participant LLM as dyn LlmClient
    participant Policy as FinanceApprovalPolicy: PermissionPolicy
    participant Hooks as InterceptorSet + HookInterceptorAdapter
    participant Audit as AuditLogHook: Hook
    participant Tools as finance Tool impls
    participant UI as Approval UI + mpsc::Sender<AgentOp>

    User->>Host: prompt text "buy 10 AAPL if under $200"
    Host->>Host: optional wire event: serde_json::to_writer(WireEvent::AgentStart { session_id })
    Host->>Harness: Harness::tools()
    Host->>Harness: Harness::hooks()
    Host->>Harness: Harness::system_prompt()
    Host->>Harness: Harness::permission_policy()
    Host->>Harness: Harness::memory_schema()
    Host->>Builder: Engine::builder().tools(...).hooks(...).system_prompt(...).permission_policy(...).memory_schema(...).build()
    Builder->>Hooks: EngineBuilder::build() wraps hooks with HookInterceptorAdapter::new(...)
    Host->>Engine: Engine::run(llm, vec![Message::user("buy 10 AAPL if under $200")]).ops(ops_rx).callback(...) or .stream()
    Engine->>Engine: RunBuilder::dispatch_callback_internal(...) or RunBuilder::dispatch_stream()
    Engine->>Engine: Engine::prepare_messages(...) prepends system prompt from EngineBuilder::system_prompt(...)
    Engine->>Hooks: Engine::dispatch_session_start(...) -> InterceptorSet::on_session_start(...)
    Hooks->>Audit: HookInterceptorAdapter::on_session_start(...) -> Hook::session_start(&SessionStartCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }

    Engine-->>Host: AgentEvent::Core(CoreEvent::IterationStarted { iteration: 1 })
    Engine->>Hooks: Engine::dispatch_before_iteration(...) -> InterceptorSet::before_iteration(...)
    Hooks->>Audit: HookInterceptorAdapter::before_iteration(...) -> Hook::user_prompt_submit(&UserPromptSubmitCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Engine->>LLM: LlmClient::chat(&messages, tools.tool_defs())
    LLM-->>Engine: ChatOutput::new(LlmResponse::ToolCalls([ToolCallItem { name: "finance.get_quote", args: {"symbol":"AAPL"} }]))

    Engine->>Engine: Engine::execute_tools_with_policy(...)
    Engine->>Engine: Engine::dispatch_tool_call_to_slot(...)
    Engine-->>Host: emit_tool_started(...) -> AgentEvent::Core(CoreEvent::ToolStarted { name: "finance.get_quote", args })
    Engine->>Policy: permission_runtime::consult_policy(...) -> PermissionPolicy::check(&PermissionContext { tool_name: "finance.get_quote", tool_input, annotations, mode: PermissionMode::AcceptEdits })
    Policy-->>Engine: Permission::Allow
    Engine->>Hooks: Engine::dispatch_intercept_tool_calls(...) -> InterceptorSet::intercept_tool_call(...)
    Hooks->>Audit: HookInterceptorAdapter::intercept_tool_call(...) -> Hook::pre_tool_use(&PreToolUseCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Hooks-->>Engine: ToolDecision::Proceed(ToolCallItem { name: "finance.get_quote", ... })
    Engine->>Tools: Engine::resolve_and_execute_intercepted_slots(...) -> Engine::execute_tools_parallel(...) -> Engine::execute_tool(...) -> Tool::call(args, &ToolContext)
    Tools-->>Engine: ToolOutput::text("AAPL: 185.00")
    Engine->>Engine: Engine::finalize_tool_call_batch(...) -> Engine::dispatch_rewrite_tool_result(...)
    Engine-->>Host: AgentEvent::Core(CoreEvent::ToolCompleted { name: "finance.get_quote", result })
    Engine->>Engine: Message::assistant_with_tool_calls(...); Message::tool_result(...)
    Engine->>Hooks: Engine::dispatch_after_tool_result(...) -> InterceptorSet::after_tool_result(...)
    Hooks->>Audit: HookInterceptorAdapter::after_tool_result(...) -> Hook::post_tool_use(&PostToolUseCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None } and audit record for quote success

    Engine-->>Host: AgentEvent::Core(CoreEvent::IterationStarted { iteration: 2 })
    Engine->>Hooks: Engine::dispatch_before_iteration(...) -> InterceptorSet::before_iteration(...)
    Hooks->>Audit: HookInterceptorAdapter::before_iteration(...) -> Hook::user_prompt_submit(&UserPromptSubmitCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Engine->>LLM: LlmClient::chat(&messages including get_quote ToolResult, tools.tool_defs())
    LLM-->>Engine: ChatOutput::new(LlmResponse::ToolCalls([ToolCallItem { id: "order-1", name: "finance.place_order", args: {"symbol":"AAPL","side":"buy","quantity":10,"max_price":200,"estimated_price":185} }]))

    Engine->>Engine: Engine::execute_tools_with_policy(...)
    Engine->>Engine: Engine::dispatch_tool_call_to_slot(...)
    Engine-->>Host: emit_tool_started(...) -> AgentEvent::Core(CoreEvent::ToolStarted { name: "finance.place_order", args })
    Engine->>Policy: permission_runtime::consult_policy(...) -> PermissionPolicy::check(&PermissionContext { tool_use_id: "order-1", tool_name: "finance.place_order", tool_input, annotations, mode: PermissionMode::AcceptEdits })
    Policy-->>Engine: Permission::AskUser { prompt: Some("Approve buy 10 AAPL @ ~$185?") }
    Engine-->>Host: AgentEvent::LoopInterceptor(ExtensionEvent::AskUser(AskUserEvent::Question { call_id: "order-1", questions: [AskUserQuestion { question, header: Some("Permission"), options: [allow, deny], multi_select: false }] }))
    Host->>UI: render AskUserEvent::Question and append approval-prompt audit record
    Engine->>Engine: deferred_calls.lock().await.insert(...); InterceptedSlot::DeferredPermission { call_id: "order-1", item }
    Engine->>Engine: Engine::resolve_and_execute_intercepted_slots(...) -> Engine::resolve_deferred_slots(...) waits on ops_rx
    UI-->>Host: user chooses "allow" / "approve"
    Host->>Engine: mpsc::Sender<AgentOp>::send(AgentOp::AskUserAnswer { call_id: Some("order-1"), answer: "approve" })
    Engine->>Engine: Engine::resolve_deferred_slots(...) receives AgentOp::AskUserAnswer
    Engine->>Engine: permission_runtime::approval_from_answer("approve") -> true
    Engine->>Hooks: Engine::dispatch_intercept_tool_calls(...) -> InterceptorSet::intercept_tool_call(...)
    Hooks->>Audit: HookInterceptorAdapter::intercept_tool_call(...) -> Hook::pre_tool_use(&PreToolUseCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Hooks-->>Engine: ToolDecision::Proceed(ToolCallItem { name: "finance.place_order", ... })
    Engine->>Tools: Engine::execute_tools_parallel(...) -> Engine::execute_tool(...) -> Tool::call(args, &ToolContext)
    Tools-->>Engine: ToolOutput::json({"status":"filled","symbol":"AAPL","side":"buy","quantity":10})
    Engine->>Engine: Engine::finalize_tool_call_batch(...) -> Engine::dispatch_rewrite_tool_result(...)
    Engine-->>Host: AgentEvent::Core(CoreEvent::ToolCompleted { name: "finance.place_order", result })
    Engine->>Engine: Message::assistant_with_tool_calls(...); Message::tool_result(...)
    Engine->>Hooks: Engine::dispatch_after_tool_result(...) -> InterceptorSet::after_tool_result(...)
    Hooks->>Audit: HookInterceptorAdapter::after_tool_result(...) -> Hook::post_tool_use(&PostToolUseCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None } and audit record for order success

    Engine-->>Host: AgentEvent::Core(CoreEvent::IterationStarted { iteration: 3 })
    Engine->>Hooks: Engine::dispatch_before_iteration(...) -> InterceptorSet::before_iteration(...)
    Hooks->>Audit: HookInterceptorAdapter::before_iteration(...) -> Hook::user_prompt_submit(&UserPromptSubmitCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Engine->>LLM: LlmClient::chat(&messages including place_order ToolResult, tools.tool_defs())
    LLM-->>Engine: ChatOutput::new(LlmResponse::Message("Bought 10 AAPL ..."))
    Engine-->>Host: AgentEvent::Core(CoreEvent::TextChunk("Bought 10 AAPL ...")) in batch mode; CoreEvent::TextDone is emitted only by chunked/streaming paths
    Engine->>Engine: Engine::handle_text_response(...) -> TextResponseOutcome::Finalize(...)
    Engine->>Engine: TurnState::into_result(...) -> AgentResult { answer, tool_calls, iterations, usage, messages }
    Engine->>Hooks: RunBuilder::dispatch_callback_internal(...) -> Engine::dispatch_on_terminal_from_meta(...) -> Engine::dispatch_on_terminal(...) -> InterceptorSet::on_terminal(...)
    Hooks->>Audit: HookInterceptorAdapter::on_terminal(...) -> Hook::stop(&StopCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Hooks->>Audit: HookInterceptorAdapter::on_terminal(...) -> Hook::session_end(&SessionEndCtx)
    Audit-->>Hooks: HookResult::Continue { updated_input: None }
    Engine-->>Host: RunBuilder::callback(...) returns AgentResult, or RunBuilder::stream() yields AgentStreamItem::Terminal(AgentTerminal { result: Ok(AgentResult), messages })
    Host->>Host: append final-answer audit record from AgentResult.answer / CoreEvent::TextChunk / CoreEvent::TextDone when present
    Host->>Host: optional wire event: serde_json::to_writer(WireEvent::AgentStop { session_id, reason })
    Host-->>User: final answer
```

Deny branch also maps cleanly with existing API:

1. `PermissionPolicy::check(...)` returns `Permission::AskUser { ... }` as above.
2. Host sends `AgentOp::AskUserAnswer { call_id: Some("order-1"), answer: "deny" }`.
3. `permission_runtime::approval_from_answer("deny") -> false`.
4. `Engine::resolve_deferred_slots(...)` resolves the slot to `ToolOutput::error("permission denied for tool 'finance.place_order' by user answer")`.
5. `Engine::finalize_tool_call_batch(...)` emits `CoreEvent::ToolCompleted { result: is_error }`.
6. `HookInterceptorAdapter::after_tool_result(...)` calls `Hook::post_tool_use_failure(&PostToolUseFailureCtx)`.
7. The LLM receives a tool-result error message and can answer that the order was not placed.

## §4. Concept gaps

No blocker gaps found for the core trade path. The existing M8.6.1 / loop 0.25.0 API surface provides:

- Harness composition through `Harness::{tools, system_prompt, hooks, permission_policy, memory_schema}` plus matching `EngineBuilder` setters.
- Tool execution through `Tool::call` and `ToolOutput`.
- Permission approval through `PermissionPolicy::check`, `Permission::AskUser`, `AskUserEvent::Question`, `AgentOp::AskUserAnswer`, and `resolve_deferred_slots`.
- Tool-result audit hook points through `Hook::post_tool_use` and `Hook::post_tool_use_failure`.
- Final-answer observation through `RunBuilder::callback(...)` events or `RunBuilder::stream()` terminal output.

Because §4 has no blockers, M9 Step 1 is not blocked by primitive/loop/harness API redesign.

## §5. Awkwardness list

These are not Step 1 blockers, but they should feed M10:

1. **Audit is split across Hook and host event handling.** `AuditLogHook` can observe tool successes/failures via `Hook::post_tool_use` and `Hook::post_tool_use_failure`, but approval prompts arrive as `AgentEvent::LoopInterceptor(ExtensionEvent::AskUser(AskUserEvent::Question { ... }))`, and final answers arrive through `CoreEvent::TextChunk` in batch mode, `CoreEvent::TextDone` in chunked/streaming paths, and/or `AgentResult`. A complete audit log therefore needs either a shared audit sink used by both the hook and the host callback, or new Hook lifecycle events for ask-user/final-answer.
2. **Two event vocabularies must be mapped.** `motosan-agent-loop` emits `motosan_agent_loop::AgentEvent::{Core, LoopInterceptor}`; `motosan-agent-primitives::AgentEvent::{AgentStart, AskUser, ToolCallStart, ToolCallEnd, ...}` is a wire format used by `agemo` after mapping in `handle_event`. Step 1 needs to be explicit about which layer the finance demo logs.
3. **`CoreEvent::ToolStarted` fires before permission approval.** In `Engine::dispatch_tool_call_to_slot`, `emit_tool_started(...)` runs before `permission_runtime::consult_policy(...)`. For `place_order`, this event means "LLM requested this tool," not "the broker order is executing." Audit/UI code must avoid treating `ToolStarted` as an irreversible side effect.
4. **Approval prompt richness depends on tool args.** `PermissionContext` carries `tool_input` and `ToolAnnotations`, but not prior tool results or arbitrary conversation state. To prompt with "@ $185", `place_order` args must include an estimated/current price, or the policy must share state with tools outside the framework.
5. **Hook session id is currently synthetic.** `HookInterceptorAdapter::session_id()` returns the literal string `"default"`, while a host like `agemo` creates its own wire `session_id`. Cross-correlating Hook audit rows with host event rows is possible but forced unless Step 1 introduces its own shared run id in the audit sink.
6. **`memory_schema` is stored but not enforced.** `EngineBuilder::memory_schema(...)` wires the field, but the current loop stores it for future enforcement. Fine for M9, but finance memory/state policy should not assume runtime enforcement yet.

## §6. Verdict

Gate PASSES — Step 1 can proceed, with the §5 awkwardness items tracked for M10.
