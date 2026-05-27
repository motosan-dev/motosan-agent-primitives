# M8 Sub-Agent Dispatch Prompts

Copy each section verbatim into a fresh sub-agent. Run them **in order** (Step N depends on Step N-1 being committed).

**Step 0** is already done — primitives commit `91701d1` on local main.

Verification between steps: `cd <repo> && git log -1 --oneline && cargo check && cargo test`.

---

## Step 1 — Complete motosan-agent-tool 0.4.0

```
You are executing Step 1 of the M8 migration plan: finishing the in-progress 0.4.0 refactor of motosan-agent-tool.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-tool/

## Critical context — working tree is NOT clean

A previous session left uncommitted partial work. Your job is verify + complete + commit, not start from scratch.

`git status` shows modified:
- Cargo.toml (already at 0.4.0 with motosan-agent-primitives + async-trait + tokio-util deps)
- src/lib.rs, src/registry.rs, src/tool.rs (already implement §3 spec)
- 7 tools already migrated: src/tools/{fetch_url, generate_pdf, js_eval, read_file, read_pdf, read_spreadsheet, web_search}.rs

13 tools remain on the 0.3 surface and must be migrated by you:
- src/tools/datetime.rs
- src/tools/currency_convert.rs
- src/tools/cost_calculator.rs
- src/tools/python_eval.rs
- src/tools/browser_act.rs
- src/tools/browser_auth.rs
- src/tools/browser_navigate.rs
- src/tools/browser_read.rs
- src/tools/browser_screenshot.rs
- src/tools/browser_snapshot.rs
- src/tools/browser_tab.rs
- src/tools/browser_wait.rs

(browser_common.rs is shared helpers — leave alone unless it imports old types.)

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §3 (D-T1 through D-T7) and §1
2. /Users/daiwanwei/Projects/wade/motosan-agent-tool/src/tools/read_file.rs — your migration template (already migrated cleanly)
3. /Users/daiwanwei/Projects/wade/motosan-agent-tool/src/tool.rs — to see Tool / ToolOutput / ToolContext surface

## Phase A — Verify existing work

1. `grep -E "^version" Cargo.toml` → expect `version = "0.4.0"`
2. `grep -n "motosan-agent-primitives\|async-trait\|tokio-util" Cargo.toml` → 3 deps present
3. `grep -n "trait Tool\|fn annotations\|async fn call\|pub struct ToolOutput\|cancellation_token" src/tool.rs` → all five present
4. **CRITICAL:** Check `ToolOutput::json()` body in src/tool.rs. Primitives 0.1.1 (commit 91701d1) now ships `ContentBlock::Json { value }`. If `ToolOutput::json` currently stringifies via `ContentBlock::Text { text: serde_json::to_string(...) }`, fix it to use `ContentBlock::Json { value: v }` directly. Update the corresponding test in src/tool.rs.

If verify steps 1-3 fail, STOP and report — working-tree state doesn't match the plan.

## Phase B — Migrate the 13 tools

Pattern (compare to src/tools/read_file.rs lines 1-65 and 56-63):

```rust
use async_trait::async_trait;
use crate::{Tool, ToolAnnotations, ToolContext, ToolDef, ToolOutput};

#[async_trait]
impl Tool for SomeStruct {
    fn def(&self) -> ToolDef { /* unchanged */ }

    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations { read_only: ..., destructive: ..., network_access: ..., idempotent: ... }
    }

    async fn call(&self, args: Value, ctx: &ToolContext) -> ToolOutput {
        // body: ToolResult::text/error/json → ToolOutput::text/error/json
    }
}
```

Note: `crate::ToolAnnotations` is re-exported from motosan-agent-primitives. Don't import it separately.

### Annotations table (apply exactly)

| Tool | read_only | destructive | network_access | idempotent |
|---|---|---|---|---|
| datetime | true | false | false | false |
| currency_convert | true | false | true | false |
| cost_calculator | true | false | true | false |
| python_eval | false | true | false | false |
| browser_act | false | true | true | false |
| browser_auth | false | true | true | false |
| browser_navigate | false | true | true | false |
| browser_read | false | false | true | false |
| browser_screenshot | false | true | true | false |
| browser_snapshot | false | true | true | false |
| browser_tab | false | true | true | false |
| browser_wait | false | false | false | false |

If a file has multiple `impl Tool for ...` blocks, repeat annotations() per impl with the same row.

In-function changes:
- `ToolResult::text(s)` → `ToolOutput::text(s)`
- `ToolResult::error(s)` → `ToolOutput::error(s)`
- `ToolResult::json(v)` → `ToolOutput::json(v)`
- `.with_citation/with_inject/with_duration` keep the same names
- `ToolContent::Text(...)` → `ContentBlock::Text { text }`
- `ToolContent::Json(...)` → `ContentBlock::Json { value }`

Drop unused `use std::future::Future; use std::pin::Pin;` lines.

## Phase C — Sweep

After 13 tools done:

```bash
rg -n 'ToolContent' src/                              # expect empty
rg -n 'Pin<Box<dyn Future' src/tools/                 # expect empty
rg -n 'ToolResult::(text|error|json)\(' src/tools/    # expect empty
rg -c 'impl Tool for' src/tools/                      # expect ~20 total
rg -c '#\[async_trait\]' src/tools/                   # expect ~20 total
```

Also verify src/registry.rs and src/lib.rs have no stale references.

## Phase D — Tests + CHANGELOG

1. `cargo test --all-features` must pass
2. `cargo doc --no-deps` clean
3. If CHANGELOG.md lacks a 0.4.0 entry, add one matching plan §1.7

If doctests fail with `cc` linker errors, prefix: `RUSTDOCFLAGS="-L /Users/daiwanwei/.nix-profile/lib"`

## Phase E — Commit

ONE commit covering all changes (working tree was mid-refactor; goal is one coherent 0.4.0 release):

```
feat: motosan-agent-tool 0.4.0 — primitives alignment

BREAKING:
- Tool trait switched to #[async_trait] with mandatory annotations()
- ToolResult removed (use motosan_agent_primitives::ToolResult on the wire; ToolOutput for in-crate returns)
- ToolContent removed (use motosan_agent_primitives::ContentBlock incl. new Json variant)
- ToolContext gained cancellation_token (tokio_util::sync::CancellationToken)

ADDED:
- ToolOutput with content/is_error/citation/inject_to_context/duration_ms and into_tool_result(tool_use_id)
- Re-exports from primitives: ContentBlock, ToolAnnotations, ToolCall, ToolResult

DEPS:
- motosan-agent-primitives 0.1.1 (path)
- async-trait 0.1
- tokio-util 0.7
```

Do NOT push.

## Acceptance
- cargo check --all-features passes
- cargo test --all-features passes
- cargo doc --no-deps clean
- rg 'ToolContent' src/ empty
- All 20 impl Tool sites in src/tools/ use #[async_trait] + implement annotations()
- One commit on local main, NOT pushed
- Cargo.toml version 0.4.0

## Hard rules
- Do NOT modify any other motosan-* repo
- Do NOT push to GitHub
- If a tool's existing 0.3 logic doesn't survive the swap (e.g. relies on inject_to_context being read inside the tool), STOP and report

## Report
1. Phase A fix-ups (if any)
2. 13 files modified with LOC delta each
3. Phase C grep results (should be empty)
4. cargo test counts
5. cargo doc warnings (0)
6. Commit SHA
7. Any annotation row that felt wrong vs the actual code
```

---

## Step 2 — motosan-agent-loop 0.23.0 (HEAVY — consider splitting)

This is the keystone refactor. **Estimated 16h sub-agent equivalent.** If a single dispatch runs out of context, split into 2a (Extension trait + redact + engine boundary) and 2b (~19 fixture sweep). The prompt below is the unified version.

```
You are executing Step 2 of the M8 migration plan: refactoring motosan-agent-loop to 0.23.0, consuming motosan-agent-tool 0.4.0.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-loop/

## Pre-conditions (MUST be true before starting)
- motosan-agent-tool: cargo test --all-features green on commit "feat: motosan-agent-tool 0.4.0 — primitives alignment"
- motosan-agent-primitives: 0.1.1 with ContentBlock::Json (commit 91701d1)

Verify: `cd ../motosan-agent-tool && grep ^version Cargo.toml` shows 0.4.0; `cd ../motosan-agent-primitives && git log --oneline | head -1` shows ContentBlock::Json commit.

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §2 (entire Step 2), §3 D-T1..D-T7, §0 (revisions)
2. /Users/daiwanwei/Projects/wade/motosan-agent-tool/src/tool.rs — Tool / ToolOutput / ToolContext surface
3. /Users/daiwanwei/Projects/wade/motosan-agent-loop/src/core/extension.rs lines 100-140 (Extension trait — sig change target)
4. /Users/daiwanwei/Projects/wade/motosan-agent-loop/src/extensions/redact/extension.rs (current rewrite_tool_result body)
5. /Users/daiwanwei/Projects/wade/motosan-agent-loop/src/core/engine.rs (search for `tool.call` site to find engine boundary)

## Phase A — Cargo.toml
Change `motosan-agent-tool = "0.3"` → `motosan-agent-tool = { path = "../motosan-agent-tool" }` (or = "0.4" if you prefer version pinning).
Bump own version 0.22.x → 0.23.0.

## Phase B — Extension trait surface change (plan §2.4-a)

src/core/extension.rs:112,123: change trait methods to use ToolOutput (Option A from plan):

```rust
async fn rewrite_tool_result(
    &mut self,
    call: &ToolCallItem,
    output: &motosan_agent_tool::ToolOutput,
    ctx: &mut HookCtx<'_>,
) -> Result<Option<motosan_agent_tool::ToolOutput>, ExtError>;

async fn after_tool_result(
    &mut self,
    call: &ToolCallItem,
    output: &motosan_agent_tool::ToolOutput,
    ctx: &mut HookCtx<'_>,
) -> Result<(), ExtError>;
```

Why ToolOutput not primitives::ToolResult: extensions need citation/inject_to_context/duration_ms.

Survey every `impl Extension` site:
```bash
rg -n 'impl Extension for|impl.*Extension for' src/ tests/
```

Update each impl block to match the new trait sig.

## Phase C — Redact extension (plan §2.7)

src/extensions/redact/extension.rs:
- `use motosan_agent_tool::{ToolContent, ToolResult}` → `use motosan_agent_tool::{ContentBlock, ToolOutput}`
- Pattern match `ToolContent::Text(t)` → `ContentBlock::Text { text }`
- Pattern match `ToolContent::Json(v)` → `ContentBlock::Json { value }` (recursive redact_json continues to work — that's the whole point of Step 0)
- Return `Ok(Some(ToolOutput { content: new_content, ..output.clone() }))` (preserve citation/inject_to_context/duration_ms from input)

Add a behavioural test (per plan §6 risks):
```rust
#[tokio::test]
async fn redact_json_inside_tool_output_still_scrubs_nested_emails() {
    // Build a ToolOutput with ContentBlock::Json { value: json!({ "user_email": "alice@example.com" }) }
    // Run redact through it
    // Assert the email is scrubbed in the rewritten Json value (recursively)
}
```

## Phase D — Engine boundary (plan §2.4-b)

In src/core/engine.rs at the tool dispatch site:

```rust
// Before:
let result: ToolResult = tool.call(args, &ctx).await;

// After:
let start = std::time::Instant::now();
let output: ToolOutput = tool.call(args, &ctx).await;
let duration_ms = start.elapsed().as_millis() as u64;

// (Extensions get the rich ToolOutput before wire conversion)
let output = run_rewrite_tool_result_chain(extensions, &call, output, ctx).await?;

// Read engine-side metadata BEFORE conversion:
let inject = output.inject_to_context;
let citation = output.citation.clone();

// Convert to wire-level primitives::ToolResult for the message:
let wire_result = output.into_tool_result(call.id.clone());
// Build Message::tool_result(...) from wire_result.content + wire_result.is_error
```

Files likely affected (verify with grep):
- src/core/engine.rs (main dispatch)
- src/core/decision.rs (`use motosan_agent_tool::ToolResult` → likely needs ToolOutput in engine paths, primitives::ToolResult on wire paths)
- src/core/event.rs (same)
- src/core/extension_set.rs (same)
- src/core/hook_ctx.rs (same)
- src/core/state.rs (ToolDef unchanged)

Decision rule: anywhere the engine **constructs** a tool's reply uses ToolOutput; anywhere it **sends** the reply (to LLM / persistence) uses primitives::ToolResult. The boundary is `ToolOutput::into_tool_result(tool_use_id)`.

## Phase E — Production impl Tool sites (plan §2.5)

src/mcp/adapter.rs:46 `McpToolAdapter`:
- Switch to #[async_trait]
- Add `fn annotations(&self) -> ToolAnnotations` mapping from MCP protocol hints (readOnlyHint, destructiveHint, idempotentHint, openWorldHint). Map:
  - read_only = readOnlyHint.unwrap_or(false)
  - destructive = destructiveHint.unwrap_or(true)
  - network_access = openWorldHint.unwrap_or(true)
  - idempotent = idempotentHint.unwrap_or(false)
- Cache annotations from server's tools/list response on adapter construction; don't re-fetch per call
- Return ToolOutput from call

src/planning.rs:335 `PlanningTool`:
- Switch to #[async_trait]
- annotations(): read_only=true, destructive=false, network_access=false, idempotent=true
- Return ToolOutput

## Phase F — Fixture sweep (plan §2.6)

~19 sites total. For each `impl Tool for ...`:
1. Add `#[async_trait::async_trait]`
2. Rewrite `Pin<Box<dyn Future + Send + '_>>` return to `async fn call(...) -> ToolOutput`
3. Add `fn annotations(&self) -> ToolAnnotations { ToolAnnotations { read_only: true, destructive: false, network_access: false, idempotent: true } }` — test stubs are always safe
4. `ToolResult::text(...)` → `ToolOutput::text(...)`

Files:
- tests/cancellation.rs (1), tests/contract.rs (1), tests/interactive_ops.rs (2)
- tests/live_anthropic.rs (6 — feature-gated, verify under feature flag)
- tests/planning.rs (1), tests/run_builder.rs (1), tests/session.rs (2)
- src/core/engine_tests.rs (~4)
- src/streaming_executor.rs:124 (TimestampTool inside `mod tests {}` — implicitly test-gated)

Re-run `rg 'impl Tool for' src/ tests/` to confirm count before sweep.

## Phase G — `cancellation` feature decision

Tool 0.4 ships `CancellationToken` on `ToolContext` unconditionally. The current `motosan-agent-loop` `cancellation` feature in Cargo.toml adds `tokio-util` + `tokio/macros`. Decide:
- If the feature only existed to gate cancellation surface in tool: REMOVE it from Cargo.toml + lib.rs
- If it gates engine-side cancellation logic (timeouts, abort signals on the loop): KEEP and document the new boundary

Default: keep but document in CHANGELOG that cancellation surface is now always available; the feature flag governs engine-level cancellation policy.

## Phase H — motosan-ai feature

Verify `cargo build --features motosan-ai` builds. The feature uses motosan-ai 0.15.x today; if it needs motosan-ai 0.16 (Step 3) first, you have a circular path-dep problem. Order:
- Either: do Step 3 before completing this step's motosan-ai feature build
- Or: temporarily disable the motosan-ai feature; finish other phases; finish motosan-ai feature after Step 3

If you choose the "temporarily disable" path, leave a TODO comment + note in commit message.

## Phase I — Tests + CHANGELOG

- `cargo test` passes
- `cargo test --features mcp-client` passes
- `cargo test --features cancellation` passes (if kept)
- `cargo doc --no-deps` clean
- CHANGELOG.md gets 0.23.0 entry (template in plan §2.9)

If doctests fail with `cc` linker errors, prefix: `RUSTDOCFLAGS="-L /Users/daiwanwei/.nix-profile/lib"`

## Phase J — Commit

ONE commit (all changes are interdependent — partial state won't compile). Message:

```
feat: motosan-agent-loop 0.23.0 — consume motosan-agent-tool 0.4

BREAKING:
- motosan-agent-tool dep bumped to 0.4
- Extension::rewrite_tool_result / after_tool_result now take &ToolOutput / return Option<ToolOutput> (was &ToolResult / Option<ToolResult>)
- Engine boundary now produces primitives::ToolResult via ToolOutput::into_tool_result
- Redact extension migrated from ToolContent to ContentBlock (incl. new ContentBlock::Json)

ADDED:
- Engine-side duration_ms measurement around tool.call()
- redact_json_inside_tool_output_still_scrubs_nested_emails integration test

NOTE: `cancellation` feature [kept|removed] — see CHANGELOG.
```

Do NOT push.

## Hard rules
- ONE commit (split only if you genuinely run out of context — then label commits clearly)
- Do NOT modify any other motosan-* repo
- Do NOT push to GitHub
- If you encounter a tool surface in motosan-agent-loop that the plan didn't anticipate (e.g. a public API leaking ToolResult through a struct field), STOP and report — don't silently invent a new design

## Report
1. Files modified per phase (B/C/D/E/F/G/H) with LOC delta
2. Final list of `impl Extension for` sites and what changed in each
3. Output of cargo test (and --features mcp-client, --features motosan-ai if applicable)
4. cargo doc warnings
5. Commit SHA
6. cancellation feature decision + rationale
7. Any unexpected scope discoveries
```

---

## Step 3 — motosan-ai SDK 0.16.0

```
You are executing Step 3 of the M8 migration plan: bumping motosan-ai's Rust SDK to consume motosan-agent-tool 0.4.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-ai/sdks/rust/

## Pre-conditions
- motosan-agent-tool 0.4.0 committed
- motosan-agent-loop 0.23.0 committed (Step 2)

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §3 (Step 3)
2. sdks/rust/src/tool_compat.rs (line 2 uses ToolDef)
3. sdks/rust/src/types.rs (line 474: `pub fn tool_defs(mut self, defs: &[motosan_agent_tool::ToolDef]) -> Self`)

## Task
1. Bump `motosan-agent-tool` dep in sdks/rust/Cargo.toml from 0.3 → 0.4 (or path).
2. Bump own version 0.15.x → 0.16.0.
3. Verify `tool_defs` signature still compiles (ToolDef is unchanged in 0.4 — type-level no break, but transitive crate identity changed).
4. Verify any internal helper that used the removed `ToolContent` / rich `ToolResult` is migrated to primitives types.

## Tests
- `cargo build` passes
- `cargo build --features agent-tool` passes
- `cargo test --features agent-tool` passes

## CHANGELOG
Prepend to sdks/rust/CHANGELOG.md:

```
## 0.16.0 — 2026-05-26

BREAKING:
- motosan-agent-tool dep bumped to 0.4. Consumers using --features agent-tool must bump their tool dep alongside.

NOTE: No public SDK signature changed at the type level. Bump reflects transitive crate identity change.
```

## Commit
ONE commit: `feat(sdk-rust): motosan-ai 0.16.0 — consume motosan-agent-tool 0.4`. Do NOT push.

## Acceptance
- cargo build + cargo test --features agent-tool both green
- Version 0.16.0
- Commit on local main, NOT pushed

## Hard rules
- Only touch sdks/rust/. Do NOT modify other motosan-ai language SDKs or other motosan-* repos.
- Do NOT push.
```

---

## Step 4 — motosan-agent-subagent

```
You are executing Step 4 of the M8 migration plan: refactoring motosan-agent-subagent's 7 impl Tool sites to the 0.4 trait.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-subagent/

## Pre-conditions
- motosan-agent-tool 0.4.0 committed
- motosan-agent-loop 0.23.0 committed
- motosan-ai 0.16.0 committed (if subagent uses it)

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §4 (Step 4)
2. /Users/daiwanwei/Projects/wade/motosan-agent-tool/src/tools/read_file.rs — migration template
3. Each of the 7 impl Tool sites (listed below)

## Phase A — Cargo.toml
- motosan-agent-tool: 0.3 → 0.4 (or path)
- motosan-agent-loop: → 0.23 (or path)
- Bump own version one minor

## Phase B — 7 impl Tool sites

Apply async-trait + annotations + ToolOutput pattern to:

| File | Tool | read_only | destructive | network_access | idempotent |
|---|---|---|---|---|---|
| src/delegation/tool.rs:87 | DelegateAgentTool | false | true | false | false |
| src/subagent/tools/close.rs | CloseSubagentTool | false | true | false | true |
| src/subagent/tools/list.rs | ListSubagentsTool | true | false | false | false |
| src/subagent/tools/followup.rs | FollowupSubagentTool | false | false | false | false |
| src/subagent/tools/send_message.rs | SendMessageSubagentTool | false | false | false | false |
| src/subagent/tools/wait.rs | WaitSubagentTool | true | false | false | false |
| src/subagent/tools/spawn.rs | SpawnSubagentTool | false | true | false | false |

For each: add #[async_trait], add fn annotations(), rewrite call to async fn ... -> ToolOutput, swap ToolResult::* for ToolOutput::*.

## Phase C — Extension impls

If src/delegation/extension.rs or src/subagent/extension.rs implements `motosan_agent_loop::Extension`, update `rewrite_tool_result` / `after_tool_result` to take &ToolOutput (per Step 2 trait sig change).

## Phase D — DelegationExtension::new

src/delegation/extension.rs:32 `pub fn new(delegates: Vec<Arc<DelegateAgentTool>>, tool_context: ToolContext) -> Self`. Signature unchanged in shape — ToolContext now carries the new cancellation_token field but ToolContext::new(...) still works. No code change here; document in CHANGELOG that cancellation can be wired via ToolContext::with_cancellation(token).

## Phase E — Sweep + tests

```bash
rg -n 'ToolContent' src/                              # expect empty
rg -n 'Pin<Box<dyn Future' src/                       # expect empty
rg -n 'ToolResult::(text|error|json)\(' src/          # expect empty
```

`cargo test` must pass.

## CHANGELOG
```
## next-minor — 2026-05-26

BREAKING:
- motosan-agent-tool 0.4, motosan-agent-loop 0.23
- All 7 internal Tool impls now use #[async_trait] and return ToolOutput

NOTE: DelegationExtension::new still takes ToolContext; callers wiring cancellation should chain .with_cancellation(token).
```

## Commit
ONE commit. Do NOT push.

## Acceptance
- cargo build + cargo test green
- All 7 Tool impls use #[async_trait]
- Commit on local main

## Hard rules
- Only touch motosan-agent-subagent
- Do NOT push
```

---

## Step 5 — motosan-sandbox

```
You are executing Step 5 of the M8 migration plan: migrating motosan-sandbox's single integration test to motosan-agent-tool 0.4.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-sandbox/

## Pre-conditions
- motosan-agent-tool 0.4.0 + motosan-agent-loop 0.23.0 committed

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §5 (Step 5)
2. The actual file: find with `find . -name loop_integration.rs`

## Phase A — Cargo.toml
- Move `motosan-agent-tool` from `[dependencies]` to `[dev-dependencies]` (audit flagged: only used in tests)
- Bump to 0.4
- Bump `motosan-agent-loop` dev-dep to 0.23
- Add `async-trait` to [dev-dependencies] if not already pulled

## Phase B — loop_integration.rs

```rust
// before
use std::future::Future;
use std::pin::Pin;
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolResult};

fn to_tool_result(out: ExecOutput, kind: motosan_sandbox::SandboxKind) -> ToolResult {
    // ToolResult::error(...) / ToolResult::text(...)
}

impl Tool for SandboxedExecTool {
    fn def(&self) -> ToolDef { ... }
    fn call(&self, args: Value, ctx: &ToolContext)
        -> Pin<Box<dyn Future<Output = ToolResult> + Send + '_>> { ... }
}

// after
use async_trait::async_trait;
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolOutput, ToolAnnotations};

fn to_tool_output(out: ExecOutput, kind: motosan_sandbox::SandboxKind) -> ToolOutput {
    // ToolOutput::error(...) / ToolOutput::text(...)
}

#[async_trait]
impl Tool for SandboxedExecTool {
    fn def(&self) -> ToolDef { ... }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations { read_only: false, destructive: true, network_access: false, idempotent: false }
    }
    async fn call(&self, args: Value, ctx: &ToolContext) -> ToolOutput {
        if /* missing command */ {
            return ToolOutput::error("missing/invalid `command`");
        }
        // ... existing body, ToolResult → ToolOutput
    }
}
```

## Phase C — Tests
`cargo test` must pass.

## CHANGELOG
```
## next-patch — 2026-05-26

CHANGED:
- Integration tests bumped to motosan-agent-tool 0.4 + motosan-agent-loop 0.23
- motosan-agent-tool moved from [dependencies] to [dev-dependencies]
```

## Commit
ONE commit. Do NOT push.

## Acceptance
- cargo build + cargo test green
- motosan-agent-tool only in [dev-dependencies]
- Commit on local main

## Hard rules
- Only touch motosan-sandbox
- Do NOT push
```

---

## Step 6 — motosan-agent-harness 0.1.1

```
You are executing Step 6 (final): updating motosan-agent-harness examples to motosan-agent-tool 0.4.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-harness/

## Pre-conditions
- motosan-agent-tool 0.4.0 committed
- (motosan-agent-loop / ai / subagent / sandbox not strictly required — harness only depends on tool + primitives)

## Required reading
1. /Users/daiwanwei/Projects/wade/motosan-agent-primitives/M8_IMPLEMENTATION_PLAN.md §6 (Step 6)
2. examples/two_tool_harness.rs — current EchoTool + AddTool impls
3. src/harness.rs — the Harness trait (should NOT change)
4. /Users/daiwanwei/Projects/wade/motosan-agent-tool/src/tools/read_file.rs — migration template

## Phase A — Cargo.toml
- Bump own version 0.1.0 → 0.1.1
- motosan-agent-tool path dep auto picks up 0.4
- Add `async-trait` to [dev-dependencies] if examples need it

## Phase B — examples/two_tool_harness.rs

Migrate EchoTool + AddTool:

```rust
use async_trait::async_trait;
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolOutput, ToolAnnotations};

struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    fn def(&self) -> ToolDef { /* unchanged */ }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations { read_only: true, destructive: false, network_access: false, idempotent: true }
    }
    async fn call(&self, args: Value, _ctx: &ToolContext) -> ToolOutput {
        let msg = args.get("message").and_then(Value::as_str).unwrap_or("");
        ToolOutput::text(msg)
    }
}

struct AddTool;
#[async_trait]
impl Tool for AddTool {
    fn def(&self) -> ToolDef { /* unchanged */ }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations { read_only: true, destructive: false, network_access: false, idempotent: true }
    }
    async fn call(&self, args: Value, _ctx: &ToolContext) -> ToolOutput {
        let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
        ToolOutput::text((a + b).to_string())
    }
}
```

## Phase C — examples/null_harness.rs
No changes (no Tool impls). Just verify it still runs.

## Phase D — src/harness.rs
The Harness trait itself does NOT change. If unit tests in src/harness.rs construct stub Tools, migrate them per Phase B pattern.

If rustdoc anywhere in src/harness.rs spells out the OLD Tool trait signature (Pin<Box<Future>>), update to async fn.

## Phase E — README.md
If README shows a Tool impl in its quick-start, update to async-trait pattern.

## Phase F — Tests + run
- `cargo check` passes
- `cargo run --example null_harness` runs cleanly
- `cargo run --example two_tool_harness` runs and prints demo.echo + demo.add table
- `cargo test` passes

## CHANGELOG
Create or prepend to CHANGELOG.md:
```
## 0.1.1 — 2026-05-26

CHANGED:
- Bumped motosan-agent-tool to 0.4
- Examples updated for new async-trait Tool + mandatory annotations() + ToolOutput
```

## Commit
ONE commit. Do NOT push.

## Acceptance
- All 4 checks (Phase F) green
- Version 0.1.1
- Commit on local main

## Hard rules
- Only touch motosan-agent-harness
- Do NOT modify the Harness trait itself unless its rustdoc references the old Tool sig
- Do NOT push
```

---

## After all 6 steps

Run the cross-chain green check:

```bash
for repo in motosan-agent-primitives motosan-agent-tool motosan-agent-loop motosan-ai/sdks/rust \
            motosan-agent-subagent motosan-sandbox motosan-agent-harness; do
  echo "=== $repo ==="
  (cd /Users/daiwanwei/Projects/wade/$repo && cargo build && cargo test) \
    || { echo "FAIL: $repo"; break; }
done
```

If any fails, do NOT publish — review the failure, decide rollback per plan §5.
