# motosan-agent-primitives

Core data types and abstract middleware traits for the **Motosan agent
framework**. This crate is the **contract layer**: it owns the wire-format
types and the `Hook` / `PermissionPolicy` traits, and nothing else.

## Layering

```text
   ┌────────────────────────────────────────────┐
   │ motosan-agent-harness-{finance,rental,…}   │  vertical implementations
   ├────────────────────────────────────────────┤
   │ motosan-agent-loop      (ReAct engine)     │  runs Harness + Tools
   ├────────────────────────────────────────────┤
   │ motosan-agent-harness   (Harness trait)    │  composition contract
   ├────────────────────────────────────────────┤
   │ motosan-agent-tool │ motosan-ai │ sandbox  │  capability + infra
   ├────────────────────────────────────────────┤
   │ motosan-agent-primitives  ← THIS CRATE     │  shared types + Hook + Permission
   └────────────────────────────────────────────┘
```

This crate has **minimum runtime dependencies** — no agent loop, no LLM
client, no sandbox. It depends only on:

- `serde` + `serde_json` — wire format
- `chrono` (default-features off, `serde` + `clock`) — timestamps
- `async-trait` — object-safe `dyn Hook` / `dyn PermissionPolicy`
- `uuid` — `MessageId`
- `thiserror` — error derives
- `tokio-util` — `CancellationToken` carried by every Hook context

Tokio is pulled in transitively via `tokio-util`. The original "zero runtime
dep" goal was relaxed once the Hook lifecycle gained cancellation support
(decision D5).

## What lives here

| Module      | Purpose                                                              |
|-------------|----------------------------------------------------------------------|
| `message`   | `Message`, `MessageId`, `Role`, `ContentBlock`, `ImageSource`, `DocumentSource` |
| `tool`      | **Data only** — `ToolCall`, `ToolResult`, `ToolAnnotations`. The `Tool` trait lives in `motosan-agent-tool`. |
| `permission`| `Permission`, `PermissionPolicy` trait, `PermissionMode`, `PermissionContext` |
| `hook`      | `Hook` trait, `HookResult`, `StopReason`, `ToolFailure`, nine lifecycle `*Ctx` structs |
| `event`     | `AgentEvent` (10 variants), `SubagentResult` — streaming output       |
| `memory`    | `MemorySchema`, `MemoryKey`, `MemoryKind` — schema only, no storage   |

## What does NOT live here

- The `Tool` trait, `ToolContext`, `ToolOutput`, `ToolError` — see
  `motosan-agent-tool`.
- The `Harness` trait — see the future `motosan-agent-harness` crate.
- `ChatRequest` / `ChatResponse` — provider-internal, see `motosan-ai`.
- Sandbox `Policy` — see `motosan-sandbox`.

## Hooks — important contract

Hooks **return by value**. To rewrite a tool's input, return
`HookResult::Continue { updated_input: Some(value) }` — never mutate the
context struct in place. This is decision D2 (revised after Codex and Claude
Agent SDK research) and exists to make cancellation safe.

Every Hook lifecycle context carries a `CancellationToken`; long-running
hooks should `select!` against `cancelled().await`.

The lifecycle has **nine** events. `post_tool_use` fires on success;
`post_tool_use_failure` is a separate event fired on tool error,
cancellation, or timeout — audit hooks should override both.

## Permission modes — important contract

`PermissionMode::Plan` denies only tools whose `ToolAnnotations.destructive`
is `true`. Read-only and network-accessing tools are allowed (decision
D4 = C, more permissive than the original proposal). This means **tool
authors must annotate `destructive` accurately** — a tool that performs
a network mutation but declares `destructive: false` will run in plan
mode. See the rustdoc on `ToolAnnotations` for the full contract.

## Stability

`0.x` — API surface is iterating. Once two harnesses (finance + rental)
have been built against this crate the API will be frozen as `1.0`. See
`IMPLEMENTATION_PLAN.md` for the milestone map.

## License

Licensed under the Apache License, Version 2.0. See `LICENSE`.
