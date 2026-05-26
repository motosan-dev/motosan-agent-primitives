# motosan-agent-primitives

Core types and abstract traits for the **Motosan agent framework**.

This crate has **no runtime dependencies** — no agent loop, no LLM client, no
sandbox, no tokio. It only defines the contracts that downstream crates
implement:

- `motosan-ai` — LLM provider abstraction (depends on this crate)
- `motosan-sandbox` — OS-level execution isolation
- `motosan-agent-loop` — ReAct loop engine
- `motosan-agent-harness-*` — domain bundles (finance, rental, etc.)

## What lives here

| Module      | Purpose                                                          |
|-------------|------------------------------------------------------------------|
| `message`   | `Message`, `Role`, `ContentBlock` — what an LLM sees             |
| `tool`      | `Tool` trait, `ToolCall`, `ToolResult`, `ToolAnnotations`        |
| `permission`| `Permission`, `PermissionPolicy` trait, `PermissionMode`         |
| `hook`      | `Hook` trait + lifecycle event context structs                   |
| `harness`   | `Harness` trait — the domain bundle contract                     |
| `event`     | `AgentEvent` — what the agent emits during a run                 |
| `memory`    | `MemorySchema` — what long-term context a harness expects        |
| `error`     | `Error` types                                                    |

## Stability

`0.x` — API surface is iterating. Once two harnesses (finance + rental) have
been built against this crate the API will be frozen as `1.0`.
