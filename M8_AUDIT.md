# M8 Audit — motosan-agent-tool downstream impact
Date: 2026-05-26
Auditor: sub-agent

## Summary
- Total impacted repos: 8 (motosan-agent-loop, motosan-agent-subagent, motosan-agent-workflow, motosan-agent-harness, motosan-ai, motosan-chain, motosan-chat, motosan-sandbox)
- Trivial: 3 (motosan-chain, motosan-sandbox, motosan-agent-harness)
- Medium: 2 (motosan-agent-subagent, motosan-ai)
- Heavy: 3 (motosan-agent-loop, motosan-agent-workflow, motosan-chat)

Scope note: the rust file grep was filtered to exclude `/target/` build
output and `motosan-agent-loop/.claude/worktrees/*` (in-flight branch copies
of the same crate). Per-repo line/file counts below count the canonical
working tree only.

motosan-agent-tool's current public surface is:
`Tool` (trait), `ToolDef`, `ToolResult`, `ToolContext`, `ToolContent` (in
`tool.rs`); `ToolRegistry` (in `registry.rs`); `Error`/`Result` (in
`error.rs`); plus a `tools` module of built-in implementations.
`ToolCall` and `ToolAnnotations` do not currently live in this crate —
under D1=B they will be introduced fresh in `motosan-agent-primitives`,
so the migration is "swap the import path + adjust trait signatures",
not "rename existing types."

## Per-repo detail

### motosan-agent-loop
- Files using motosan-agent-tool: 47 (src + tests + 1 example)
- Reference lines: 488
- Cargo dep: `motosan-agent-tool = "0.3"` (regular + dev-dep)
- Symbols imported: `Tool`, `ToolContext`, `ToolDef`, `ToolResult`,
  `ToolContent` (the full surface). Heavily used via `use
  motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolResult}` at the top
  of nearly every src and test file.
- `impl Tool for ...` sites: 23 (mostly test fixtures, plus
  production: `McpToolAdapter`, `PlanningTool`, `StreamingExecutor`
  helpers).
- Public API leak: **yes** — these are load-bearing types in the loop's
  public surface, even though `lib.rs` does not `pub use` them:
  - `EngineBuilder::tool(Arc<dyn Tool>)` and
    `EngineBuilder::tools(impl IntoIterator<Item = Arc<dyn Tool>>)`
  - `EngineBuilder::tool_context(ToolContext)`
  - `LlmClient::chat(messages, tools: &[ToolDef])` trait method —
    every downstream LlmClient impl sees `ToolDef`
  - `AgentLoopState.tools: &'a [ToolDef]`
  These leak into every consumer of motosan-agent-loop (subagent,
  workflow, chain, sandbox).
- Effort estimate: **Heavy**
- Notes: This is the keystone. Everything downstream rebuilds on this
  crate's API. Touching the `LlmClient::chat` signature alone forces
  every implementor (motosan-ai's `MotosanAiClient`, workflow's clients,
  chain's tests, sandbox's mock) to recompile and possibly adjust.

### motosan-agent-subagent
- Files using motosan-agent-tool: 13
- Reference lines: 13
- Cargo dep: `motosan-agent-tool = "0.3"` (regular + dev-dep)
- Symbols imported: `Tool`, `ToolContext`, `ToolDef`, `ToolResult`.
- `impl Tool for ...` sites: 1 (`DelegateAgentTool` + 6 subagent
  tools — `close`, `followup`, `list`, `send_message`, `spawn`, `wait`).
- Public API leak: **yes (moderate)** —
  `DelegationExtension::new(delegates: Vec<Arc<DelegateAgentTool>>,
  tool_context: ToolContext)` exposes `ToolContext`. `DelegateAgentTool`
  is itself `pub use`'d from `lib.rs` and implements `Tool`.
- Effort estimate: **Medium**
- Notes: 7 `impl Tool` sites need updated method signatures if the trait
  changes. Public footprint is small (just `ToolContext` in one
  constructor), so once `motosan-agent-loop` is done, this is mostly
  mechanical.

### motosan-agent-workflow
- Files using motosan-agent-tool: 32 (across 5 sub-crates: cli, core,
  model, runtime, swarm)
- Reference lines: 80
- Cargo dep: `motosan-agent-tool = "0.3"` in cli/core/model/runtime/swarm
- Symbols imported: `Tool`, `ToolContext`, `ToolDef`, `ToolResult`.
- `impl Tool for ...` sites: 7 (`NotifyHumanTool` in runtime, plus 6
  test fixtures across core/tests and model tests).
- Public API leak: **yes (heavy)** —
  - `crates/runtime/src/builder.rs`:
    `WorkflowBuilder::with_tool(impl motosan_agent_tool::Tool + 'static)`,
    `with_tools(Vec<Arc<dyn motosan_agent_tool::Tool>>)`,
    `with_tool_context(motosan_agent_tool::ToolContext)`
  - `crates/runtime/src/lib.rs`: `WorkflowRuntime.tool_context:
    Option<motosan_agent_tool::ToolContext>` (pub(crate), but flows through
    via builder)
  - `crates/runtime/src/node_dispatch.rs`: same
  - `crates/model/src/node.rs`:
    `NodeAgent.tool_definitions: Vec<motosan_agent_tool::ToolDef>` —
    this is a **pub field on a serialized model type**. Anyone who builds a
    workflow JSON/program touches this.
- Effort estimate: **Heavy**
- Notes: Multi-crate workspace, public model struct field exposes
  `ToolDef`. Migration must be coordinated across all 5 sub-crates in one
  go. Plus it pins `motosan-agent-loop = "0.8"` whereas subagent/sandbox
  pin `0.22` — workflow is already lagging, so M8 is a good forcing
  function to catch it up.

### motosan-agent-harness
- Files using motosan-agent-tool: 3 (`src/harness.rs`, 2 examples)
- Reference lines: 6
- Cargo dep: `motosan-agent-tool = { path = "../motosan-agent-tool" }`
- Symbols imported: `Tool`, `ToolContext`, `ToolDef`, `ToolResult`,
  `Value`.
- `impl Tool for ...` sites: 2 (`EchoTool`, `AddTool` — both in
  `examples/two_tool_harness.rs`).
- Public API leak: **no** — harness is a binary/example workspace, no
  downstream library consumers. The one src reference is just a docstring
  `use` example plus the `pub use motosan_agent_tool::Tool` re-export
  inside `harness.rs`.
- Effort estimate: **Trivial**
- Notes: Smallest impacted repo. Update imports, done.

### motosan-ai
- Files using motosan-agent-tool: 2 (`sdks/rust/src/tool_compat.rs`,
  `sdks/rust/src/types.rs`)
- Reference lines: 2
- Cargo dep: `motosan-agent-tool = { version = "0.3", optional = true }`
  behind feature `agent-tool`
- Symbols imported: `ToolDef` only.
- `impl Tool for ...` sites: 0.
- Public API leak: **yes** —
  `sdks/rust/src/types.rs:474`:
  `pub fn tool_defs(mut self, defs: &[motosan_agent_tool::ToolDef]) -> Self`
  is on `ChatRequestBuilder` (or equivalent), so the `ToolDef` type leaks
  into the SDK's public surface, but only behind the `agent-tool`
  feature.
- Effort estimate: **Medium**
- Notes: Tiny code surface but the leak is a real second-order concern
  for anyone using the Rust SDK with `--features agent-tool`. Easy to
  rewrite the signature once primitives is published. Also: motosan-ai is
  what `motosan-agent-loop` wraps via the `motosan-ai` feature, so a
  version-bump dance is needed.

### motosan-chain
- Files using motosan-agent-tool: 3 (`src/agent.rs`,
  `src/agent_session.rs`, `src/llm.rs`)
- Reference lines: 8
- Cargo dep: `motosan-agent-tool = "0.3"`
- Symbols imported: `ToolDef`, `ToolContext`.
- `impl Tool for ...` sites: 0.
- Public API leak: **no** — every `use motosan_agent_tool::*` is inside a
  `#[cfg(test)] mod tests` block. Tests need to call
  `LlmClient::chat(..., &[ToolDef])`, but that comes from
  `motosan-agent-loop`'s trait. `motosan-chain`'s own public API does
  not name any `motosan_agent_tool` type.
- Effort estimate: **Trivial**
- Notes: Could probably drop the direct `motosan-agent-tool` dep
  entirely once `motosan-agent-loop` re-exports through primitives. The
  dep declaration is at top-level (not `[dev-dependencies]`), which is
  wrong — flag for cleanup during M8.

### motosan-chat
- Files (workspace-wide) using motosan-agent-tool directly: 3
  (`motosan-chat-tool/src/{lib.rs,error.rs,tool.rs}`)
- Cargo dep: `motosan-agent-tool = "0.2"` in `motosan-chat-tool` only
  (note: pinned to **0.2**, older than the 0.3 every other consumer uses
  — second cleanup item)
- Symbols imported: `Tool`, `ToolContent`, `ToolContext`, `ToolDef`,
  `ToolResult`, `ToolRegistry`, `Error`, `Result`, `tools` module.
- `impl Tool for ...` sites: 9 across the chat workspace
  (`SpawnSubagentTool`, `SubAgentTool`, `McpToolAdapter`, plus 6 test/
  example fixtures).
- Public API leak: **yes — maximal**.
  `motosan-chat/rust/crates/motosan-chat-tool/src/lib.rs`:
  ```rust
  pub use motosan_agent_tool::tools;
  pub use motosan_agent_tool::ToolRegistry;
  pub use motosan_agent_tool::{Tool, ToolContent, ToolContext, ToolDef, ToolResult};
  pub use motosan_agent_tool::{Error, Result};
  ```
  Every other chat crate (chat-core, chat-agent, chat-multi, chat-mcp,
  chat-ai) imports these via `motosan_chat_tool::...`. So changing
  `motosan-agent-tool` shape ripples into the chat-tool re-export wall,
  then through 8+ chat crates and their consumers.
- Effort estimate: **Heavy**
- Notes: Largest blast radius after motosan-agent-loop. The whole point
  of motosan-chat-tool seems to be "rebrand motosan-agent-tool with extra
  retriever bits," so M8 should consider whether chat-tool should
  re-export from `motosan-agent-primitives` directly (cutting out
  motosan-agent-tool) or stay a thin wrapper. Either way the file count
  in this single Cargo crate is small (3 files); the pain is the
  downstream chat workspace.

### motosan-sandbox
- Files using motosan-agent-tool: 1
  (`crates/motosan-sandbox/tests/loop_integration.rs`)
- Reference lines: 1
- Cargo dep: `motosan-agent-tool = "0.3"` (used in `[dev-dependencies]`
  scope via tests; declared in `[dependencies]` — see note)
- Symbols imported: `Tool`, `ToolContext`, `ToolDef`, `ToolResult`.
- `impl Tool for ...` sites: 1 (`SandboxedExecTool` in the integration
  test).
- Public API leak: **no** — only test code uses it; sandbox's public
  surface doesn't expose any `motosan_agent_tool` type.
- Effort estimate: **Trivial**
- Notes: Integration test will need import updates, that's it.

## Recommended ordering for M8 Step 2
The dependency graph for the refactor is:

```
motosan-agent-primitives  (D1=B types live here)
   ↓
motosan-agent-tool        (Tool trait stays here, signatures use primitives)
   ↓
motosan-agent-loop        (Heavy: LlmClient::chat, EngineBuilder::tool, ...)
   ├─► motosan-agent-subagent (Medium)
   ├─► motosan-agent-workflow (Heavy, multi-crate)
   ├─► motosan-chain          (Trivial; only test deps)
   ├─► motosan-sandbox        (Trivial; only test deps)
   └─► motosan-ai             (Medium; feature-gated SDK surface)
       └─► back-loop via the `motosan-ai` feature

motosan-chat-tool             (Heavy; pub-use wall to many chat crates)
   ↓
motosan-chat-{core,agent,multi,mcp,ai,...}
```

Suggested order (each step compiles + tests green before the next):

1. **motosan-agent-tool** — update its own `Tool` trait + types to use
   primitives. Publish 0.4. This is the source of truth.
2. **motosan-agent-loop** — bump to consume 0.4, fix the
   `LlmClient::chat` and `EngineBuilder::tool*` signatures, update its
   ~47 files. Publish 0.23.
3. **motosan-ai** — bump to 0.4 and update the one `tool_defs`
   signature, since motosan-agent-loop depends on it via the
   `motosan-ai` feature.
4. **motosan-agent-subagent** — straightforward swap; 7 `impl Tool`
   sites + 1 public constructor.
5. **motosan-agent-workflow** (all 5 sub-crates simultaneously) — single
   bump, fix builder + pub model field. Take this opportunity to bump
   `motosan-agent-loop` from 0.8 to 0.23.
6. **motosan-chain** — trivial import swap; also consider dropping
   `motosan-agent-tool` from `[dependencies]` and moving to `[dev-deps]`.
7. **motosan-sandbox** — trivial test fix.
8. **motosan-chat-tool** — update the `pub use` block; bump from 0.2
   straight to 0.4 (consider also re-exporting from primitives directly).
9. **motosan-chat** rest of workspace — recompile, fix any breakage that
   bubbles through chat-tool's re-exports.
10. **motosan-agent-harness** — last; example/sandbox repo, lowest
    consequence.

## Risks surfaced

- **Version drift across consumers.** Cargo deps currently pin a mix of
  `motosan-agent-tool 0.2` (motosan-chat-tool) and `0.3` (everyone else),
  and `motosan-agent-loop 0.8` (workflow) vs `0.9` (chain) vs `0.22`
  (subagent, sandbox). M8 must explicitly unify these or the published
  graph breaks. The largest gap is motosan-chat-tool sitting on 0.2.
- **`LlmClient::chat(messages, tools: &[ToolDef])` is a public trait
  method on motosan-agent-loop.** Every external implementor of
  `LlmClient` (out-of-tree custom providers we cannot see) is a silent
  breaking change. Mitigation: bump to a major-equivalent loop version,
  document migration clearly.
- **motosan-chat-tool's `pub use motosan_agent_tool::*` style.** This
  was presumably chosen so chat-tool could "swap" the impl crate later
  — and now is that moment. Decide whether chat-tool re-exports from
  `motosan-agent-tool 0.4` (cheap, preserves layering) or migrates its
  re-exports to `motosan-agent-primitives` directly (cleaner, but breaks
  any chat consumer that pattern-matched on the path).
- **motosan-agent-workflow lag.** Workflow pins motosan-agent-loop 0.8
  while subagent/sandbox are on 0.22. Bringing workflow forward to 0.23
  during M8 is a separate refactor in itself; that's likely where the
  biggest schedule slip will be.
- **23 `impl Tool` test fixtures in motosan-agent-loop alone.** If the
  `Tool` trait's `run`/`call` signature gains or renames a parameter,
  every fixture needs a touch. They're trivial individually, but the
  blast count is what makes the loop work "heavy" in calendar time even
  though the conceptual change is small.
- **Hidden re-exports.** Both motosan-chat-tool (`pub use
  motosan_agent_tool::{...}`) and motosan-agent-loop (via its public
  trait signatures) re-export the surface implicitly. Anyone grepping
  `motosan_agent_tool` in *their own* code will miss the fact that they
  also pick up the same types through `motosan_chat_tool::ToolDef` or
  `motosan_agent_loop::LlmClient::chat`'s parameter list. Worth a
  callout in the migration notes.
