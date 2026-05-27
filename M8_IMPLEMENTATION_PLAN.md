# M8 Implementation Plan — motosan-agent-tool 0.4 migration

**Date:** 2026-05-26 (revised after review)
**Scope:** 6 repos (5 refactors + 1 interface upgrade) — `motosan-agent-tool`, `motosan-agent-loop`, `motosan-ai`, `motosan-agent-subagent`, `motosan-sandbox`, `motosan-agent-harness`
**Companion docs:** [M8_AUDIT.md](M8_AUDIT.md), [IMPLEMENTATION_PLAN.md](IMPLEMENTATION_PLAN.md) (the parent plan)
**Status:** Step 1 partially executed — Cargo.toml + tool.rs + 7/20 built-ins already changed in the working tree, **NOT YET COMMITTED**. Steps 2–6 unstarted. See §0 for verified state.

---

## 0. Revisions after independent review (2026-05-26, updated 2026-05-27)

**v3 patch (2026-05-27)** — second-pass review fixed three internal contradictions / factual errors that survived the v2 revision:

- **D-T2 / §1.5(d) / §2.7 contradicted D-T4 on JSON handling.** All three said "stringify JSON into `ContentBlock::Text`" — the exact regression D-T4 was added to prevent. Now consistent: `ContentBlock::Json { value }` everywhere (the variant shipped in primitives 0.1.1).
- **§2.6 falsely claimed `async-trait` was transitively available.** Rust proc-macro attribute resolution does NOT see transitive deps. Now: §2.1 (loop `[dev-dependencies]`), §4.1 (subagent `[dependencies]`), §5.2 (sandbox `[dev-dependencies]`), §6.3 (harness `[dev-dependencies]`) all explicitly declare `async-trait = "0.1"`.
- **§2.4-a Extension sweep was "non-exhaustive"** — replaced with verified census from `rg`: 40 `impl Extension` sites total, but only 15 bodies actually override `rewrite_tool_result` / `after_tool_result` and need hand-migration. Production overrides: just 1 (redact). The rest is engine-internal test plumbing.

A fresh reviewer audited the v1 plan against actual repo state and surfaced one blocker scope-gap, one stale-state blocker, and several smaller issues. The relevant changes since v1:

1. **Step 1 is partly done in the working tree.** `motosan-agent-tool/Cargo.toml` already declares `version = "0.4.0"` with `motosan-agent-primitives` / `async-trait` / `tokio-util` deps; `src/tool.rs` already implements the `Tool` trait with `#[async_trait]`, mandatory `annotations()`, `ToolOutput`, `cancellation_token: CancellationToken`, and the conversion to `primitives::ToolResult`. `git log` does NOT contain a 0.4 commit — the changes sit uncommitted. 7 of 20 built-in tools are modified (`fetch_url`, `generate_pdf`, `js_eval`, `read_file`, `read_pdf`, `read_spreadsheet`, `web_search`); the other 13 (`datetime`, `currency_convert`, `cost_calculator`, all 7 `browser_*`, `python_eval`, plus `mod.rs` / `registry.rs` checks) still need work. §1 is now a verify-and-complete checklist instead of a from-scratch rewrite. Recommended first action: confirm what's in the working tree matches §3, then finish the 13 remaining tools, then commit as a single 0.4.0 release.

2. **Step 2 scope creep (Blocker).** The `Extension::rewrite_tool_result` / `after_tool_result` trait methods on [`src/core/extension.rs:112,123`](src/core/extension.rs) take `&ToolResult` and return `Option<ToolResult>` — and `ToolResult` here is the 0.3.2 rich type. This isn't a "swap imports in redact" job — it's a public trait signature change that ripples to every `impl Extension` site. §2.4 now contains an explicit sub-step for redesigning these signatures, with a survey of impacted impls.

3. **Concern: `ToolOutput::json()` → flat string regression in redact.** Primitives' `ContentBlock` has no `Json` variant — only `Text/Image/Document/ToolUse/ToolResult`. The plan's mapping `ToolContent::Json(v) → ContentBlock::Text { text: serde_json::to_string(&v) }` means the redact extension's recursive `redact_json` pass at [`redact/extension.rs:117`](src/extensions/redact/extension.rs) silently degrades to flat-string regex matching. **Resolution: add `ContentBlock::Json { value: serde_json::Value }` to primitives** — a forward-compatible new variant. Primitives isn't published yet, so this is cheap. See updated D-T4.

4. **D-T1 rationale was wrong.** v1 claimed "Rust's auto-Default gives `destructive=false`". In fact primitives ships an *explicit* `impl Default for ToolAnnotations` at [`primitives/src/tool.rs:143-154`](src/tool.rs) whose rustdoc calls itself "maximally cautious" while setting `destructive=false, network_access=false`. Under `PermissionMode::Plan` (D4=C) a `destructive=false, network_access=false` tool slips through. The mandatory-`annotations()` decision is correct; the *reason* is "the published default contradicts its own doc and isn't safe under D4=C", not "auto-Default is wrong". §3 D-T1 updated; a follow-up cleanup ticket for primitives' Default rustdoc/impl belongs in M10.

5. **Fixture enumeration.** §2.6 v1 referenced "the rest enumerated by grep". `rg 'impl Tool for' tests/` returns 14 impls across 7 test files (`cancellation:1, contract:1, interactive_ops:2, live_anthropic:6, planning:1, run_builder:1, session:2`). Plus `engine_tests.rs` (~4 sites) and `streaming_executor.rs:124` (inside `mod tests`, already test-gated — reviewer's "not test-gated" claim was wrong; the impl is at indent depth that places it inside the test module). Updated count: ~19 fixture sites in motosan-agent-loop.

6. **TimestampTool (Important #1 in review) — reviewer mistake.** [`streaming_executor.rs:124`](src/streaming_executor.rs) sits inside the `mod tests {}` block (verifiable by the 4-space indent on `impl Tool for TimestampTool {`). It is implicitly test-only via the parent module. No action needed; v1 plan's parenthetical was already correct.

7. **License inconsistency.** Now tracked in §6 risks (low priority — MIT/Apache-2.0 are compatible; track for M10 cleanup).

8. **Step 2 effort bumped.** With the Extension trait scope expansion + redact behavioural test + ~19 fixture sweep + motosan-ai feature interaction, 12 h was too round. New estimate: 16–20 h. Total plan: ~30 h.

9. **MCP annotations.** Step 2.5's annotations for `McpToolAdapter` should map directly from the MCP protocol's tool-annotations hints (`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) per [MCP 2025-06-18](https://modelcontextprotocol.io/specification/2025-06-18) — not invent a generic "permissive default". Updated.

The reviewer's full findings are referenced inline below at each affected section. Sections not mentioned above are unchanged.

---

## 1. Why this exists

Phase A built `motosan-agent-primitives` as the new contract layer (M0–M6). `motosan-agent-tool 0.3.2` predates that work and:

- Defines its own `ToolResult`, `ToolContext`, `ToolContent` instead of consuming primitives types
- Has no `ToolAnnotations` (violates D10=A)
- Has no `CancellationToken` on `ToolContext` (violates D5=A)
- Uses hand-rolled `Pin<Box<dyn Future>>` instead of `#[async_trait]` (inconsistent with primitives' `Hook` trait)

This plan brings `motosan-agent-tool` to 0.4.0 and migrates the 5 downstream consumers in dependency order.

## 2. Execution order (gated, sequential)

```
Step 1.  motosan-agent-tool        0.3.2 → 0.4.0      [foundation]
   └── gate: cargo test --all-features green
Step 2.  motosan-agent-loop        0.22.x → 0.23.0    [keystone consumer]
   └── gate: cargo test green; motosan-ai feature builds
Step 3.  motosan-ai (Rust SDK)     0.15.x → 0.16.0    [SDK signature]
   └── gate: feature `agent-tool` builds + tests
Step 4.  motosan-agent-subagent    current → +1 minor [7 impl Tool sites]
   └── gate: cargo test green
Step 5.  motosan-sandbox           current → patch    [1 integration test]
   └── gate: cargo test green
Step 6.  motosan-agent-harness     0.1.0 → 0.1.1      [example refactor]
   └── gate: cargo run --example null_harness && two_tool_harness
```

**Each gate must pass before starting the next step.** No parallel work — every step depends on the previous crate's published Cargo.lock-resolvable version.

For the local migration we use `path = "../<crate>"` deps throughout so we don't need to publish 0.4.0 to crates.io before the chain is verified. Publishing happens once Step 6 is green.

## 3. Shared design decisions (the spec)

These decisions apply across all 6 steps. **Do not relitigate during execution.** If something genuinely doesn't fit, stop and revisit this section.

### D-T1. Tool trait — `#[async_trait]` + mandatory annotations

Final shape, in `motosan-agent-tool/src/tool.rs`:

```rust
use async_trait::async_trait;
use motosan_agent_primitives::ToolAnnotations;
use serde_json::Value;

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDef;
    /// MANDATORY — no default impl. See type docs on ToolAnnotations.
    fn annotations(&self) -> ToolAnnotations;
    async fn call(&self, args: Value, ctx: &ToolContext) -> ToolOutput;
}
```

**Why mandatory annotations (revised):** primitives ships an explicit `impl Default for ToolAnnotations` at [`primitives/src/tool.rs:143-154`](src/tool.rs) whose rustdoc calls it "maximally cautious" — but it sets `destructive=false, network_access=false`, which under `PermissionMode::Plan` (D4=C) means **the default annotation makes the tool look safe to plan-mode and lets it run unattended**. That's the opposite of cautious. The load-bearing warning on lines 100–115 of the same file tells tool authors to set `destructive=true` when in doubt. Mandatory `annotations()` is the only way to force the author to face that decision; allowing any default — explicit or derived — keeps the unsafe fallback path. Cost: ~30 mechanical additions across the ecosystem (every `impl Tool` gets one short method). Follow-up M10 ticket: rewrite or delete primitives' misleading `impl Default for ToolAnnotations`.

**Why async-trait:** matches primitives' `Hook` trait. `Arc<dyn Tool>` still works (async-trait boxes the future internally).

### D-T2. `ToolOutput` — new type in motosan-agent-tool, replaces 0.3.2 `ToolResult`

```rust
use motosan_agent_primitives::ContentBlock;

pub struct ToolOutput {
    pub content: Vec<ContentBlock>,
    pub is_error: bool,
    pub citation: Option<String>,
    pub inject_to_context: bool,
    pub duration_ms: Option<u64>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self { ... }
    pub fn json(v: Value) -> Self { /* wraps ContentBlock::Json { value: v } — see D-T4 */ }
    pub fn error(s: impl Into<String>) -> Self { ... }
    pub fn with_citation(self, c: impl Into<String>) -> Self { ... }
    pub fn with_inject(self, b: bool) -> Self { ... }
    pub fn with_duration(self, ms: u64) -> Self { ... }
    pub fn as_text(&self) -> Option<&str> { ... }

    /// Convert to the wire-level primitives::ToolResult. Drops
    /// citation/inject_to_context/duration_ms (engine-side metadata).
    pub fn into_tool_result(
        self,
        tool_use_id: impl Into<String>,
    ) -> motosan_agent_primitives::ToolResult {
        motosan_agent_primitives::ToolResult {
            tool_use_id: tool_use_id.into(),
            content: self.content,
            is_error: self.is_error,
        }
    }
}
```

**Why a separate ToolOutput:** primitives' `ToolResult` carries `tool_use_id` (the call id), which a tool author cannot know at construction time. The engine stamps the id when it converts ToolOutput → ToolResult at the wire boundary. `citation`, `inject_to_context`, `duration_ms` stay on ToolOutput as engine-side metadata that doesn't belong on the wire.

### D-T3. `ToolContext` — add `cancellation_token`, keep serde

```rust
use tokio_util::sync::CancellationToken;

pub struct ToolContext {
    pub caller_id: String,
    pub platform: String,
    pub cwd: Option<std::path::PathBuf>,
    pub extra: HashMap<String, Value>,
    #[serde(skip, default)]
    pub cancellation_token: CancellationToken,
}

impl ToolContext {
    pub fn new(caller_id: impl Into<String>, platform: impl Into<String>) -> Self { ... }
    // Existing: with_cwd, with, get_str, get_u64, get_bool
    pub fn with_cancellation(self, token: CancellationToken) -> Self { ... }
    pub fn is_cancelled(&self) -> bool { self.cancellation_token.is_cancelled() }
}
```

`CancellationToken` is not Serialize; `#[serde(skip, default)]` preserves existing wire-format round-trips. Deserialized contexts get a fresh, never-cancelled token — `serde_roundtrip_tool_context_without_cwd_is_backward_compatible` (current test at line 514) must still pass.

### D-T4. `ToolContent` — DELETE; add `ContentBlock::Json` to primitives

Replaced by `motosan_agent_primitives::ContentBlock`. The 0.3.2 `Text` variant maps cleanly:
- `ToolContent::Text(s)` → `ContentBlock::Text { text: s }`

The `Json` variant requires a primitives change. **Add a new variant** to [`primitives/src/message.rs`](src/message.rs):

```rust
pub enum ContentBlock {
    Text { text: String },
    Image { source: ImageSource },
    Document { source: DocumentSource },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: Vec<ContentBlock>, is_error: bool },
    /// NEW — structured JSON payload that downstream processors (redact,
    /// citation extractors, validators) can walk recursively without
    /// re-parsing a string.
    Json {
        /// The JSON value; serialized as a normal JSON tree, not a string.
        value: serde_json::Value,
    },
}
```

Serde uses `#[serde(tag = "type", rename_all = "snake_case")]` on the enum, so the wire tag is `"json"`. New mapping:
- `ToolContent::Json(v)` → `ContentBlock::Json { value: v }`

**Why this is a primitives change, even though primitives was "frozen":** primitives is not published yet, and the alternative (stringifying JSON into `ContentBlock::Text`) causes a real regression in motosan-agent-loop's redact extension — `redact_json` recursively walks the structure to scrub PII inside nested objects ([`redact/extension.rs:117`](src/extensions/redact/extension.rs)). After stringification, that path silently degrades to flat-string regex, which can miss embedded emails / API keys that span line boundaries or whose serialization differs from the original. The cost of adding the variant now (≤ 30 lines in primitives, including doctest) is far less than carrying a silent security regression across the migration.

Procedure:
1. Add `ContentBlock::Json { value: serde_json::Value }` to primitives `src/message.rs`, with rustdoc and a serde round-trip test.
2. Bump `motosan-agent-primitives` version (0.1.0 → 0.1.1) — it's a purely additive variant.
3. Tool 0.4's `ToolOutput::json(v)` then writes `ContentBlock::Json { value: v }` instead of `ContentBlock::Text { text: serde_json::to_string(&v).unwrap() }`.
4. Redact extension's match arm becomes `ContentBlock::Json { value } => recurse with redact_json(value)`.

Audit-flagged callers to update: `motosan-agent-loop/src/extensions/redact/extension.rs:7`. Behavioural test required: a `redact_json_inside_tool_output_still_scrubs_nested_emails` integration test in motosan-agent-loop to prove the round-trip preserves structural redaction.

### D-T5. lib.rs re-exports

After refactor, `motosan-agent-tool/src/lib.rs`:

```rust
pub mod error;
pub mod registry;
pub mod tool;
pub mod tools;

pub use error::{Error, Result};
pub use registry::ToolRegistry;
pub use serde_json::Value;
pub use tool::{Tool, ToolContext, ToolDef, ToolOutput};

// Convenience re-exports so downstream can `use motosan_agent_tool::*`
// without separately depending on primitives:
pub use motosan_agent_primitives::{ContentBlock, ToolAnnotations, ToolCall, ToolResult};
```

Note: `ToolContent` is gone. `ToolResult` now resolves to the primitives wire type, not the 0.3.2 rich type.

### D-T6. Versioning

| Crate | Old | New | Reason |
|---|---|---|---|
| motosan-agent-tool | 0.3.2 | **0.4.0** | breaking trait surface |
| motosan-agent-loop | 0.22.x | **0.23.0** | LlmClient::chat sig breaks |
| motosan-ai sdks/rust | 0.15.x | **0.16.0** | tool_defs sig breaks under `agent-tool` feature |
| motosan-agent-subagent | next minor | breaking via tool 0.4 |
| motosan-sandbox | patch | only dev-dep + test |
| motosan-agent-harness | 0.1.0 | **0.1.1** | examples updated |

### D-T7. No GitHub pushes

Every step commits to local main; nothing leaves the machine until Wade reviews the entire chain. Crates.io publishes are explicitly out of scope for this plan.

---

## Step 1 — motosan-agent-tool 0.4.0 (verify-and-complete)

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-agent-tool/`
**State:** uncommitted partial work in the working tree as of 2026-05-26. `Cargo.toml` is at 0.4.0; `src/tool.rs` already implements the §3 surface; 7 of 20 built-in tools are modified. Steps below split into **verify what's there** and **finish the gap**, then commit as one 0.4.0 release.

### 1.0 Verify existing work

Run these checks against the current working tree before any new edits:

```bash
cd /Users/daiwanwei/Projects/wade/motosan-agent-tool
git status                             # confirm modified files: Cargo.toml, lib.rs, registry.rs, tool.rs, src/tools/{fetch_url,generate_pdf,js_eval,read_file,read_pdf,read_spreadsheet,web_search}.rs
grep -E "^version" Cargo.toml          # expect: version = "0.4.0"
grep -E "ContentBlock::Json" src/tool.rs # expect: ToolOutput::json uses ContentBlock::Json (after D-T4 primitives change lands)
grep -c "#\[async_trait\]" src/tools/  # expect: matches in 7 modified tools, NOT in the 13 unmodified
cargo check                            # may fail if primitives doesn't yet have ContentBlock::Json
cargo test --lib                       # lib-level tests must pass
```

If `cargo check` fails because `ToolOutput::json` uses something not in primitives, that's the cue to apply D-T4 step 1 (add `ContentBlock::Json`) first.

### 1.1 Cargo.toml

```toml
[package]
name = "motosan-agent-tool"
version = "0.4.0"                 # was 0.3.2
edition = "2021"
license = "MIT"
# ...other metadata unchanged...

[dependencies]
motosan-agent-primitives = { path = "../motosan-agent-primitives" }   # NEW
async-trait = "0.1"                                                    # NEW
tokio-util = { version = "0.7", default-features = false }             # NEW (CancellationToken)
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
tokio = { version = "1", features = ["sync", "rt"] }
# feature-gated deps unchanged: reqwest, pdf-extract, calamine, boa_engine, chrono, chrono-tz, printpdf
```

### 1.2 src/tool.rs — full rewrite

- Replace the existing 0.3.2 surface entirely.
- Implement `Tool`, `ToolDef`, `ToolOutput`, `ToolContext` per §3.
- Drop `ToolContent` and the old `ToolResult` struct definition.
- Keep `ToolDef` validation methods (`validate_input_schema`, `validate_args`, `parse_args`) unchanged — they're orthogonal to the trait surface.

### 1.3 src/lib.rs

Apply the re-export block from D-T5.

### 1.4 src/registry.rs

Likely no change needed — `ToolRegistry` already operates on `Arc<dyn Tool>`. Verify object safety holds after the async-trait switch; if not, the only adjustment is the `Tool` import line.

### 1.5 src/tools/*.rs — 20 built-in tools

**Status as of this revision:** 7 done (modified in working tree), 13 still on the 0.3 surface. The 13 remaining are: `datetime`, `currency_convert`, `cost_calculator`, `python_eval`, and all 7 `browser_*` tools (`browser_act`, `browser_auth`, `browser_navigate`, `browser_read`, `browser_screenshot`, `browser_snapshot`, `browser_tab`, `browser_wait`). (`browser_common.rs` is shared helpers — not an `impl Tool`.) `mod.rs` may need a tweak depending on how the tool list is registered; verify after the per-file sweep.

Every file needs:

(a) Switch `impl Tool` from manual `Pin<Box<dyn Future>>` to `#[async_trait] impl Tool` with `async fn call`.

(b) Add `fn annotations(&self) -> ToolAnnotations` per this table (cautious defaults — destructive=true when in doubt):

| Tool | read_only | destructive | network_access | idempotent |
|---|---|---|---|---|
| `web_search` | false | false | true | false |
| `fetch_url` | false | false | true | false |
| `read_file` | true | false | false | true |
| `read_pdf` | true | false | false | true |
| `read_spreadsheet` | true | false | false | true |
| `generate_pdf` | false | true | false | false |
| `js_eval` | false | true | false | false |
| `python_eval` | false | true | false | false |
| `datetime` | true | false | false | false |
| `currency_convert` | true | false | true | false |
| `cost_calculator` | true | false | true | false |
| `browser_act` | false | true | true | false |
| `browser_auth` | false | true | true | false |
| `browser_common` | n/a (not a Tool impl — shared helpers) | | | |
| `browser_navigate` | false | true | true | false |
| `browser_read` | false | false | true | false |
| `browser_screenshot` | false | true | true | false |
| `browser_snapshot` | false | true | true | false |
| `browser_tab` | false | true | true | false |
| `browser_wait` | false | false | false | false |

If a file holds multiple Tool impls (audit shows `mod.rs` re-exports), repeat the annotations() method per impl.

(c) Replace return-type sites:
- `ToolResult::text(...)` → `ToolOutput::text(...)`
- `ToolResult::error(...)` → `ToolOutput::error(...)`
- `ToolResult::json(...)` → `ToolOutput::json(...)`
- Builder methods (`with_citation`, `with_inject`, `with_duration`) keep the same names on `ToolOutput`.

(d) If a tool returns `ToolContent::Json(value)` directly (not through the helper), replace with `ContentBlock::Json { value }` (the variant landed in primitives 0.1.1 per D-T4 — do NOT stringify, that would silently regress redact's structural walk).

### 1.6 Tests

In `src/tool.rs`, the existing tests cover ToolResult/ToolContext serde. Replace ToolResult tests with ToolOutput equivalents; keep ToolContext tests. Add:

- `tool_output_into_tool_result_strips_metadata` — citation/duration not on primitives::ToolResult
- `tool_context_cancellation_field` — `with_cancellation()`, `is_cancelled()`
- `tool_context_serde_skips_cancellation` — round-trip preserves other fields; deserialized token is fresh + not cancelled
- `arc_dyn_tool_object_safe` — `Arc<dyn Tool>` compiles + `annotations()` reachable

The back-compat test `serde_roundtrip_tool_context_without_cwd_is_backward_compatible` (current line 514) must still pass — extend the asserted JSON to also omit `cancellation_token` and verify deserialization succeeds.

### 1.7 CHANGELOG.md

Prepend a 0.4.0 entry:

```
## 0.4.0 — 2026-05-26

BREAKING:
- Tool trait uses #[async_trait]; signature changed from manual Pin<Box<Future>> to async fn call(...).
- Tool::annotations() is now mandatory (no default impl). Tool authors must declare ToolAnnotations explicitly.
- ToolResult removed from this crate. Use motosan_agent_primitives::ToolResult on the wire and the new ToolOutput type for in-crate tool returns.
- ToolContent removed. Use motosan_agent_primitives::ContentBlock.
- ToolContext gained a cancellation_token field (tokio_util::sync::CancellationToken); marked #[serde(skip, default)] for wire-format back-compat.

ADDED:
- ToolOutput struct with content/is_error/citation/inject_to_context/duration_ms fields and an into_tool_result(tool_use_id) conversion to the primitives wire type.
- Re-exports from motosan-agent-primitives: ContentBlock, ToolAnnotations, ToolCall, ToolResult.

DEPS:
- New: motosan-agent-primitives, async-trait, tokio-util.
```

### 1.8 Acceptance gate for Step 1

- `cargo check --all-features` passes
- `cargo test --all-features` passes
- `cargo doc --no-deps` clean
- `grep -r 'ToolContent' src/` empty (allow matches in CHANGELOG only)
- `Cargo.toml` version is `0.4.0`
- New commit on local main, NOT pushed

---

## Step 2 — motosan-agent-loop 0.23.0

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-agent-loop/`
**Scale:** 47 files, 488 references, 23 impl Tool fixtures (per [M8_AUDIT.md](M8_AUDIT.md))

This is the heavyweight. Treat it as a single atomic commit — partial state will not compile.

### 2.1 Cargo.toml

Change `motosan-agent-tool = "0.3"` to `motosan-agent-tool = { path = "../motosan-agent-tool" }` (or version `0.4` if published).
Bump own `version = "0.23.0"`.

**Add `async-trait` explicitly** — the ~13 test-fixture `impl Tool` blocks use `#[async_trait::async_trait]`, and Rust's attribute-macro resolution requires the crate to be declared in motosan-agent-loop's own manifest even though it's already a transitive dep through motosan-agent-tool:

```toml
[dev-dependencies]
async-trait = "0.1"   # NEW — required for #[async_trait] on test-fixture Tool impls
```

If any production `impl Tool` site uses it (e.g. McpToolAdapter, PlanningTool — see §2.5), promote to `[dependencies]`.

### 2.2 LlmClient trait — sig change

[src/llm.rs:174–176](src/llm.rs):
```rust
// before
async fn chat(&self, messages: &[Message], tools: &[ToolDef]) -> Result<ChatOutput>;
// after — same signature; ToolDef still comes from motosan_agent_tool (kept in crate)
async fn chat(&self, messages: &[Message], tools: &[ToolDef]) -> Result<ChatOutput>;
```

**`ToolDef` is unchanged in 0.4** — so this signature stays identical at the type level. The break is at the trait-object call site: every external `LlmClient` impl will need its impl block re-checked because `ToolDef`'s crate identity changed (still in motosan-agent-tool, but the dep version moved). Cargo's coherence rules treat 0.3 and 0.4 as different crates.

Mitigation: document in CHANGELOG that consumers must bump `motosan-agent-tool` dep alongside `motosan-agent-loop`.

### 2.3 EngineBuilder — `tool_context` and result conversions

`EngineBuilder::tool_context(ctx: ToolContext)` keeps the same signature — `ToolContext` is a struct, just with an extra non-serializable field. Existing callers that build a ToolContext via `ToolContext::new(...)` will get a default `CancellationToken` — fine for back-compat.

If the engine wants real cancellation, downstream callers can pass one via the new builder:
```rust
let cancel = CancellationToken::new();
builder = builder.tool_context(
    ToolContext::new("agent-1", "platform").with_cancellation(cancel.clone())
);
// later: cancel.cancel();
```

### 2.4-a `Extension` trait surface change (NEW — from review)

[`src/core/extension.rs:112,123`](src/core/extension.rs) declares two trait methods that take the 0.3.2 rich `ToolResult` directly:

```rust
async fn rewrite_tool_result(
    &mut self,
    call: &ToolCallItem,
    result: &ToolResult,                // <-- the OLD rich type
    ctx: &mut HookCtx<'_>,
) -> Result<Option<ToolResult>, ExtError>;

async fn after_tool_result(
    &mut self,
    call: &ToolCallItem,
    result: &ToolResult,
    ctx: &mut HookCtx<'_>,
) -> Result<(), ExtError>;
```

After 0.4 the rich `ToolResult` is gone — author-facing returns are `ToolOutput`, wire form is `primitives::ToolResult`. Pick one of these for the trait surface:

**Option A (recommended):** Use `ToolOutput` — extensions operate on the rich engine-side type so they see citation / inject_to_context / duration_ms.

```rust
async fn rewrite_tool_result(
    &mut self,
    call: &ToolCallItem,
    output: &ToolOutput,                          // CHANGED
    ctx: &mut HookCtx<'_>,
) -> Result<Option<ToolOutput>, ExtError>;
```

This preserves redact's current power (it can also redact `citation` URLs) and other extensions that want to react to engine metadata.

**Option B:** Use `primitives::ToolResult` — extensions only see the wire data. Simpler but loses the metadata channel. Reject unless we discover an extension that explicitly needs the wire shape.

Pick A unless a concrete need for B turns up during sweep.

Survey every `impl Extension` site and migrate the trait method bodies accordingly.

**Exhaustive impl Extension census (verified 2026-05-27 via `rg -n 'impl.*Extension for' motosan-agent-loop/{src,tests}/ motosan-agent-subagent/{src,tests}/`):** 40 sites total across 19 files. The vast majority take default impls and need NO body edits — only the trait signature change in [`extension.rs:112,123`](src/core/extension.rs) forces them to recompile against the new types.

**Sites that actually override `rewrite_tool_result` or `after_tool_result` (verified via `rg -n 'fn (rewrite_tool_result|after_tool_result)\b'`) — these are the bodies that must be hand-migrated:**

Production (1 site):
- `motosan-agent-loop/src/extensions/redact/extension.rs:102` — `rewrite_tool_result` (also adds `ContentBlock::Json` arm per §2.7)

Engine dispatcher (2 sites — wrapper methods, not Extension impls but pass through):
- `motosan-agent-loop/src/core/extension_set.rs:229` — `after_tool_result` (pub async dispatcher)
- `motosan-agent-loop/src/core/extension_set.rs:264` — `rewrite_tool_result` (pub async dispatcher)

Engine internal tests (4 sites in extension_set.rs):
- `extension_set.rs:511` `after_tool_result` (HaltingToolOutputExt)
- `extension_set.rs:535` `after_tool_result` (CountingToolOutputExt)
- `extension_set.rs:558` `rewrite_tool_result` (RewriteAppend)
- `extension_set.rs:579` `rewrite_tool_result` (ErroringRewrite)

engine_tests.rs (7 sites):
- 4272 (HaltOnToolOutput::after), 4292 (InjectAfterToolOutput::after), 4309 (UppercaseRewrite::rewrite), 4412 (AssertStateVisibleInToolHooks::rewrite), 4426 (…::after), 4450 (AssertRewrittenAfterTool::rewrite), 4459 (…::after)

External integration tests (1 site):
- `motosan-agent-loop/tests/extension_resume_e2e.rs:59` — `after_tool_result` (DeferringExt)

Subagent (0 sites override these methods — `DelegationExtension`, `SubagentExtension`, `DenyAllSpawnsExtension` use defaults; they still recompile against the new sig but bodies are untouched).

**Body-edit total: 1 production + 6 engine-internal + 7 engine_tests + 1 external integration = 15 bodies.** Plus the 2 dispatcher wrappers in extension_set.rs. Plus the trait definition itself in extension.rs:112,123.

The other 24 of the 40 `impl Extension` blocks take defaults for both methods and need no body work — but they DO need to recompile, so they're free monitoring for whether the new types break anything subtle (e.g. lifetime bounds on `ToolOutput`).

This sub-step **must complete before** §2.4-b (engine boundary conversion) because the engine calls `rewrite_tool_result` before producing the wire `ToolResult`.

### 2.4-b Tool result → wire conversion at the engine boundary

The engine currently does roughly `let result: ToolResult = tool.call(args, &ctx).await;` then constructs a `Message::tool_result(...)` from it. After 0.4:

```rust
// before
let result = tool.call(args, &ctx).await;
let msg = Message::tool_result(call_id.clone(), &result.as_text().unwrap_or_default());

// after
let start = std::time::Instant::now();
let output = tool.call(args, &ctx).await;
let duration_ms = start.elapsed().as_millis() as u64;
// Engine reads ToolOutput-side metadata BEFORE conversion:
let inject = output.inject_to_context;
let citation = output.citation.clone();
// Convert to wire-level ToolResult for the message:
let wire_result = output.into_tool_result(call_id.clone());
// Build the loop's Message::tool_result from the wire result's content + is_error.
```

The engine should attach `duration_ms` from its own timing (more accurate than tool-self-reported); `inject_to_context` and `citation` still flow to engine logging / context-injection logic per existing semantics. Audit:
- [src/core/engine.rs](src/core/engine.rs) — main tool dispatch site
- [src/core/decision.rs:10](src/core/decision.rs) — `use motosan_agent_tool::ToolResult` becomes `ToolOutput` for in-engine handling, or stays as `ToolResult` if it's already wire-side
- [src/core/event.rs:12](src/core/event.rs) — same
- [src/core/extension.rs:17](src/core/extension.rs) — same
- [src/core/extension_set.rs:23](src/core/extension_set.rs) — same
- [src/core/hook_ctx.rs:181](src/core/hook_ctx.rs) — `use motosan_agent_tool::ToolResult` — review whether the hook context wants the rich ToolOutput or the wire ToolResult

Decision rule: anywhere the engine **constructs** a tool's reply uses `ToolOutput`; anywhere it **sends** the reply out (to LLM or persistence) uses `primitives::ToolResult`. The boundary is `ToolOutput::into_tool_result(...)`.

### 2.5 Production impl Tool sites — 2

[src/mcp/adapter.rs:46](src/mcp/adapter.rs) `impl Tool for McpToolAdapter`:
- Switch to `#[async_trait]`
- Add `fn annotations(&self) -> ToolAnnotations` — **map directly from MCP protocol hints**, not a generic default. Per MCP 2025-06-18, each tool exposes `annotations.readOnlyHint`, `annotations.destructiveHint`, `annotations.idempotentHint`, `annotations.openWorldHint`. Map them as:
  - `read_only = readOnlyHint.unwrap_or(false)`
  - `destructive = destructiveHint.unwrap_or(true)`  // safe default when server is silent
  - `network_access = openWorldHint.unwrap_or(true)` // MCP servers usually proxy external state
  - `idempotent = idempotentHint.unwrap_or(false)`
  Cache the annotations on `McpToolAdapter` construction (server's tools/list response carries them); don't re-fetch on every `annotations()` call.
- Return `ToolOutput` from `call`

[src/planning.rs:335](src/planning.rs) `impl Tool for PlanningTool`:
- Same switch
- Annotations: read_only=true, destructive=false, network_access=false, idempotent=true (it's a pure planning prompt)

### 2.6 Test fixture impl Tool sites — ~19 enumerated

Mechanical sweep. For each `impl Tool for SomeTestTool`:
1. Add `#[async_trait::async_trait]` attribute on the impl block. **`async-trait` must be in motosan-agent-loop's own `[dev-dependencies]`** (added in §2.1) — proc-macro attribute resolution does NOT see transitive deps, despite `async-trait` being pulled in by motosan-agent-tool. A naive "it's already there transitively" assumption will fail compilation on the first fixture.
2. Rewrite `fn call(...) -> Pin<Box<dyn Future + Send + '_>>` as `async fn call(...) -> ToolOutput`
3. Add `fn annotations(&self) -> ToolAnnotations { ToolAnnotations { read_only: true, destructive: false, network_access: false, idempotent: true } }` — test stubs are always safe
4. Replace `ToolResult::text(...)` with `ToolOutput::text(...)`

Files (live count via `rg 'impl Tool for' src/ tests/`):

| File | Impl count | Notes |
|---|---|---|
| `tests/cancellation.rs` | 1 | |
| `tests/contract.rs` | 1 | |
| `tests/interactive_ops.rs` | 2 | |
| `tests/live_anthropic.rs` | 6 | feature-gated; verify they compile under feature flags |
| `tests/planning.rs` | 1 | |
| `tests/run_builder.rs` | 1 | |
| `tests/session.rs` | 2 | |
| `src/core/engine_tests.rs` | ~4 | grep `^impl Tool for` inside `#[cfg(test)] mod tests` |
| `src/streaming_executor.rs:124` | 1 | inside `mod tests {}` — already test-gated by parent module, no `#[cfg(test)]` needed |
| **Total** | **~19** | re-run `rg` before starting to confirm |

Also: `src/llm.rs:345, 371` define `MockLlmClient::chat` — that's the `LlmClient` trait signature, not `Tool`, so it doesn't need this sweep. But verify the `tools: &[ToolDef]` parameter still compiles after the dep bump.

### 2.7 Redact extension — ToolContent removal

[src/extensions/redact/extension.rs:7](src/extensions/redact/extension.rs):
```rust
// before
use motosan_agent_tool::{ToolContent, ToolResult};
// after
use motosan_agent_tool::{ContentBlock, ToolOutput};
```

Replace pattern matches:
- `ToolContent::Text(s)` → `ContentBlock::Text { text }`
- `ToolContent::Json(v)` → `ContentBlock::Json { value }` (the variant exists in primitives 0.1.1 per D-T4; the redact extension's `redact_json` recursive walk at [`extension.rs:117`](src/extensions/redact/extension.rs) must continue to operate on the structured value, NOT on a stringified form).

Add a new match arm to redact's content-block dispatch: `ContentBlock::Json { value } => /* recurse with redact_json(value) */`.

### 2.8 motosan-ai feature compat

`src/motosan_ai_impl.rs:18` uses `motosan_agent_tool::ToolDef` — keep, but verify motosan-ai dep is bumped to 0.16.0 to consume the new tool-side surface. If motosan-ai is bumped first, that's fine because tool 0.4 keeps `ToolDef` unchanged.

### 2.9 CHANGELOG.md (motosan-agent-loop)

```
## 0.23.0 — 2026-05-26

BREAKING:
- motosan-agent-tool dep bumped to 0.4. All Tool impls (including external LlmClient implementors' test fixtures) must migrate per motosan-agent-tool's 0.4.0 release notes.
- Tool call sites now produce ToolOutput; the engine converts to primitives::ToolResult at the wire boundary.
- Redact extension switched from ToolContent to ContentBlock.

ADDED:
- Engine-side duration_ms measurement around tool.call() (previously read off tool result).

DEPS:
- Bump motosan-agent-tool to 0.4 (path/version).
- motosan-ai feature targets 0.16 — bump downstream dep accordingly if using the feature.
```

### 2.10 Acceptance gate for Step 2

- `cargo build` passes
- `cargo build --features motosan-ai` passes (requires Step 3 done first if using path deps with a circular feature — see §3 for the order)
- `cargo build --features mcp-client` passes
- `cargo build --features cancellation` passes
- `cargo test` passes (all 23 fixture impls compile + run)
- `grep -r 'ToolContent' src/` returns nothing except CHANGELOG
- `grep -r 'Pin<Box<dyn Future' src/` returns nothing in Tool impls (allowed elsewhere)
- Version in Cargo.toml is `0.23.0`
- Commit on local main, NOT pushed

---

## Step 3 — motosan-ai (Rust SDK) 0.16.0

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-ai/`
**Scale:** 2 files, 2 reference lines (per audit)

### 3.1 Cargo.toml (sdks/rust/Cargo.toml)

Bump `motosan-agent-tool` dep from `0.3` to `0.4` (path or version).
Bump own `version = "0.16.0"`.

### 3.2 sdks/rust/src/tool_compat.rs

[Line 2](sdks/rust/src/tool_compat.rs): `use motosan_agent_tool::ToolDef` — unchanged at the type level. Verify everything compiles after the dep bump; if any internal helper used the removed `ToolContent` or `ToolResult` rich form, replace with the primitives types.

### 3.3 sdks/rust/src/types.rs

[Line 474](sdks/rust/src/types.rs):
```rust
pub fn tool_defs(mut self, defs: &[motosan_agent_tool::ToolDef]) -> Self {
    // unchanged — ToolDef is unchanged in 0.4
    ...
}
```

The signature stays valid. The semver bump is because the **resolved transitive crate identity** of `motosan_agent_tool` changed — any consumer of `motosan-ai` with `--features agent-tool` will need to also bump their tool dep to 0.4.

### 3.4 Tests

Run the full `sdks/rust` test suite under `--features agent-tool`. The `agent-tool` feature is the only surface affected.

### 3.5 CHANGELOG.md (motosan-ai/sdks/rust/CHANGELOG.md)

```
## 0.16.0 — 2026-05-26

BREAKING:
- motosan-agent-tool dep bumped to 0.4. Consumers using --features agent-tool must bump their tool dep alongside.

NOTE: No public SDK signature changed at the type level. The semver bump
reflects the transitive crate identity change in motosan-agent-tool.
```

### 3.6 Acceptance gate for Step 3

- `cargo build` passes
- `cargo build --features agent-tool` passes
- `cargo test --features agent-tool` passes
- Version is `0.16.0`
- Commit on local main, NOT pushed

---

## Step 4 — motosan-agent-subagent

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-agent-subagent/`
**Scale:** 13 files, 7 impl Tool sites, 1 public constructor leak

### 4.1 Cargo.toml

Bump `motosan-agent-tool` 0.3 → 0.4 (path or version).
Bump `motosan-agent-loop` to 0.23 (it was already on 0.22 per audit — verify; if older, this is also where the loop bump catches up).
Bump own version one minor.

**Add `async-trait` to `[dependencies]`** — same reasoning as §2.1, but here it goes to `[dependencies]` (not `[dev-dependencies]`) because the 7 production `impl Tool` sites in §4.3 are non-test code:

```toml
[dependencies]
async-trait = "0.1"   # NEW — required for #[async_trait] on production Tool impls
```

### 4.2 DelegationExtension constructor

[src/delegation/extension.rs:32](src/delegation/extension.rs):
```rust
// signature unchanged in shape; ToolContext now has cancellation_token field
pub fn new(
    delegates: Vec<Arc<DelegateAgentTool>>,
    tool_context: ToolContext,
) -> Self { ... }
```

Callers constructing `ToolContext::new(...)` continue to work — they just get a default (never-cancelled) token. Document in CHANGELOG that wiring up cancellation requires `ToolContext::with_cancellation(token)`.

### 4.3 The 7 impl Tool sites

For each, apply the same mechanical refactor as Step 2.6:

1. [src/delegation/tool.rs:87](src/delegation/tool.rs) — `DelegateAgentTool`
   - Annotations: read_only=false, destructive=true, network_access=false, idempotent=false (delegates a subagent which can do anything)
2. [src/subagent/tools/close.rs](src/subagent/tools/close.rs) — `CloseSubagentTool`
   - Annotations: read_only=false, destructive=true, network_access=false, idempotent=true
3. [src/subagent/tools/list.rs](src/subagent/tools/list.rs) — `ListSubagentsTool`
   - Annotations: read_only=true, destructive=false, network_access=false, idempotent=false
4. [src/subagent/tools/followup.rs](src/subagent/tools/followup.rs) — `FollowupSubagentTool`
   - Annotations: read_only=false, destructive=false, network_access=false, idempotent=false
5. [src/subagent/tools/send_message.rs](src/subagent/tools/send_message.rs) — `SendMessageSubagentTool`
   - Annotations: read_only=false, destructive=false, network_access=false, idempotent=false
6. [src/subagent/tools/wait.rs](src/subagent/tools/wait.rs) — `WaitSubagentTool`
   - Annotations: read_only=true, destructive=false, network_access=false, idempotent=false (just blocks)
7. [src/subagent/tools/spawn.rs](src/subagent/tools/spawn.rs) — `SpawnSubagentTool`
   - Annotations: read_only=false, destructive=true, network_access=false, idempotent=false

Each impl: add `#[async_trait]`, rewrite return type to `ToolOutput`, replace `ToolResult::text/error/json` constructors with `ToolOutput::*`, add `fn annotations()`.

### 4.4 CHANGELOG.md

```
## next-minor — 2026-05-26

BREAKING:
- motosan-agent-tool dep bumped to 0.4.
- motosan-agent-loop dep bumped to 0.23.
- All 7 internal Tool impls now use #[async_trait] and return ToolOutput.
- DelegationExtension::new still accepts ToolContext; callers wiring cancellation should construct via ToolContext::new(...).with_cancellation(token).
```

### 4.5 Acceptance gate for Step 4

- `cargo build` passes
- `cargo test` passes
- All 7 Tool impls use `#[async_trait]` and return `ToolOutput`
- Commit on local main, NOT pushed

---

## Step 5 — motosan-sandbox

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-sandbox/`
**Scale:** 1 integration test (per audit)

### 5.1 Cargo.toml

Move `motosan-agent-tool` from `[dependencies]` to `[dev-dependencies]` (audit flagged this — it's only used in tests). Bump 0.3 → 0.4. Also bump `motosan-agent-loop` dev-dep to 0.23.

### 5.2 tests/loop_integration.rs

[Lines 14, 44, 52–56, 95, 115, 118](tests/loop_integration.rs):

```rust
// before
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolResult};
// after
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolOutput};
use motosan_agent_primitives::ToolAnnotations;

fn to_tool_result(out: ExecOutput, kind: motosan_sandbox::SandboxKind) -> ToolOutput {
    // ToolResult::error(...) → ToolOutput::error(...)
    // ToolResult::text(...) → ToolOutput::text(...)
}

impl Tool for SandboxedExecTool {
    fn def(&self) -> ToolDef { ... }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations {
            read_only: false,
            destructive: true,         // sandbox exec is by definition destructive-capable
            network_access: false,     // sandbox should not have network
            idempotent: false,
        }
    }
    // Drop the manual Pin<Box> wrapper, switch to async fn:
    async fn call(&self, args: Value, ctx: &ToolContext) -> ToolOutput {
        if /* missing command */ {
            return ToolOutput::error("missing/invalid `command`");
        }
        // ... existing body
    }
}
```

Add `#[async_trait::async_trait]` on the impl block. **`async-trait` must be added to motosan-sandbox's `[dev-dependencies]` explicitly** — Rust proc-macro attribute resolution does NOT see transitive deps, so the fact that motosan-agent-tool pulls in async-trait does not make `#[async_trait::async_trait]` resolvable in motosan-sandbox's own test code.

### 5.3 CHANGELOG (sandbox)

```
## next-patch — 2026-05-26

CHANGED:
- Tests bumped to motosan-agent-tool 0.4 + motosan-agent-loop 0.23.
- motosan-agent-tool moved from [dependencies] to [dev-dependencies] (only used in integration tests).
```

### 5.4 Acceptance gate for Step 5

- `cargo build` passes (sandbox's own crate doesn't actually use tool 0.4 outside tests)
- `cargo test` passes
- `motosan-agent-tool` is in `[dev-dependencies]` only
- Commit on local main, NOT pushed

---

## Step 6 — motosan-agent-harness interface upgrade

**Repo:** `/Users/daiwanwei/Projects/wade/motosan-agent-harness/`
**Scale:** 3 files (`src/harness.rs`, 2 examples), 6 reference lines
**This is what you originally asked for as a plan.** Smallest impacted repo — trivial migration.

### 6.1 Cargo.toml

Bump `motosan-agent-tool` path dep — no version change needed since it's a path dep, but the new tool 0.4 surface will be picked up automatically.
Bump own `version = "0.1.1"`.

### 6.2 src/harness.rs

The `Harness` trait itself does NOT change — it references `Arc<dyn motosan_agent_tool::Tool>`, which remains a valid trait object after 0.4. Verify the rustdoc references to `Tool::call` signatures still match (the docstrings show the trait surface; update if they spell out the 0.3.2 `Pin<Box<Future>>` return type).

The two unit tests in `src/harness.rs` (defaults + object-safety) likely use stub Tools — if so, those need the `#[async_trait]` + `fn annotations()` updates per Step 2.6.

### 6.3 examples/two_tool_harness.rs

`EchoTool` and `AddTool` are the only impl Tool sites in this crate. Apply:

```rust
use async_trait::async_trait;
use motosan_agent_tool::{Tool, ToolContext, ToolDef, ToolOutput};
use motosan_agent_primitives::ToolAnnotations;

struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    fn def(&self) -> ToolDef { /* unchanged */ }
    fn annotations(&self) -> ToolAnnotations {
        ToolAnnotations {
            read_only: true,
            destructive: false,
            network_access: false,
            idempotent: true,
        }
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
        ToolAnnotations {
            read_only: true,
            destructive: false,
            network_access: false,
            idempotent: true,
        }
    }
    async fn call(&self, args: Value, _ctx: &ToolContext) -> ToolOutput {
        let a = args.get("a").and_then(Value::as_i64).unwrap_or(0);
        let b = args.get("b").and_then(Value::as_i64).unwrap_or(0);
        ToolOutput::text((a + b).to_string())
    }
}
```

Add `async-trait = "0.1"` to motosan-agent-harness's Cargo.toml under `[dev-dependencies]` (examples). **Required — must be explicit, not transitive.** Proc-macro attribute resolution does NOT inherit transitive deps; the macro lookup happens against the harness crate's own manifest at example-compile time.

### 6.4 examples/null_harness.rs

No changes needed — `NullHarness` returns empty Vec<Arc<dyn Tool>>, doesn't construct any Tool. Just verify it still runs.

### 6.5 README.md

Update the quick-start code block if it shows a Tool impl — switch to the async-trait pattern.

### 6.6 CHANGELOG.md

Create one if absent:

```
## 0.1.1 — 2026-05-26

CHANGED:
- Bumped motosan-agent-tool to 0.4.
- Examples updated for new async-trait Tool + mandatory annotations() + ToolOutput.
```

### 6.7 Acceptance gate for Step 6

- `cargo check` passes
- `cargo run --example null_harness` runs
- `cargo run --example two_tool_harness` runs and prints the same `demo.echo` / `demo.add` table as before
- `cargo test` passes
- Commit on local main, NOT pushed

---

## 4. Cross-cutting verification — full chain green

After all 6 steps, run from a clean state:

```bash
for repo in motosan-agent-tool motosan-agent-loop motosan-ai/sdks/rust \
            motosan-agent-subagent motosan-sandbox motosan-agent-harness; do
  echo "=== $repo ==="
  (cd /Users/daiwanwei/Projects/wade/$repo && cargo build && cargo test) \
    || { echo "FAIL: $repo"; break; }
done
```

All six must pass. If any fails, do not proceed to publishing.

## 5. Rollback strategy

Each step is a single commit on its repo's local main. Rollback is a per-repo `git revert <sha>`:

```bash
cd /Users/daiwanwei/Projects/wade/<repo>
git log --oneline | head -3   # find the M8 commit
git revert <sha>
```

Because nothing is pushed and crate versions aren't on crates.io, rollback has no external consequences. The earlier the failure, the cheaper — that's why the gates are sequential.

## 6. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| `#[async_trait]` breaks object safety for some Tool with a borrowed return | Low | Fixed by `dyn Tool` being `Send + Sync`; async-trait handles boxing |
| A built-in tool's annotations table guess wrong (e.g., browser_read NOT actually mutating cookies) | Medium | These are best-guess defaults; tool authors should review the table and adjust before publishing. Plan errs cautious (destructive=true when unsure) per the load-bearing warning. |
| `ToolContent::Json` had downstream callers we missed | Low | Grep `ToolContent` across all motosan-* repos before starting Step 2; the audit identified one in motosan-agent-loop redact extension. |
| Step 2's `motosan-ai` feature creates a circular dependency during local path-dep iteration | Medium | Build motosan-ai with `--no-default-features` first; bump motosan-agent-loop after motosan-ai compiles. Order is enforced by §2 gate. |
| External `LlmClient` impls (out-of-tree custom providers) silently break on the 0.4 transitive crate identity change | High | Document loudly in motosan-agent-loop 0.23 CHANGELOG: bumping motosan-agent-loop requires bumping motosan-agent-tool in lockstep. |
| ~19 test fixture impls in motosan-agent-loop have subtle bugs after the sweep | Medium | Sweep mechanically with a `sed`/`fastmod` script (provided below) where possible, run full test suite, address surviving failures one-by-one. |
| motosan-agent-workflow (HEAVY per audit, not in this plan) lags | High but out-of-scope | Workflow is deliberately deferred — its motosan-agent-loop pin (0.8) is so far behind that bringing it forward is a separate refactor. |
| Redact extension regresses silently if `ContentBlock::Json` isn't added (review concern) | Was high, now mitigated | D-T4 mandates the new variant. Add `redact_json_inside_tool_output_still_scrubs_nested_emails` integration test in motosan-agent-loop to lock in the behaviour. |
| License inconsistency (primitives/harness Apache-2.0; tool/loop/subagent MIT) | Low | Legally compatible (MIT can be combined into Apache-2.0 work). Track as M10 cleanup; do NOT block this plan. |
| Primitives' `impl Default for ToolAnnotations` is misleading (claims cautious, behaves permissive under D4=C) | Low | Mandatory `annotations()` (D-T1) eliminates the path that hits this default. M10 ticket: rewrite or delete the Default impl. |
| `motosan-agent-loop`'s `cancellation` feature becomes partly redundant — tool 0.4 ships `CancellationToken` on `ToolContext` unconditionally | Low | Decide in Step 2: either drop the feature, keep it for engine-side cancellation surface (different concern), or rename. Document in CHANGELOG. |

## 7. Mechanical sweep helpers

For Step 2's 23 fixture impls, the following `fastmod`/`sed` patterns cover ~80% of the mechanical work (run from each repo's root):

```bash
# 1. Switch Tool impl blocks from manual Pin<Box> to async_trait
#    (manual review required around the function body — this just flags sites)
rg -n 'fn call\s*\([^)]*\)\s*->\s*Pin<Box<dyn Future<Output\s*=\s*ToolResult>' --type rust

# 2. Sweep ToolResult constructors → ToolOutput
fastmod 'ToolResult::text\(' 'ToolOutput::text(' --extensions rs
fastmod 'ToolResult::error\(' 'ToolOutput::error(' --extensions rs
fastmod 'ToolResult::json\(' 'ToolOutput::json(' --extensions rs

# 3. Find ToolContent uses (manual review required for replacements)
rg -n 'ToolContent::' --type rust
```

These are starting points only — compile failures will guide the remaining edits.

## 8. Open questions (resolve before execution)

1. ~~**Should `ToolOutput::json()` serialize the Value as a JSON string inside ContentBlock::Text, or should we add a `ContentBlock::Json` variant to primitives?**~~ **RESOLVED in revision:** add `ContentBlock::Json { value: serde_json::Value }` to primitives now. JSON-as-string causes a real silent regression in redact. See D-T4.

2. **Does motosan-agent-loop want a separate path for ToolOutput.inject_to_context to control whether tool results enter the model's context next turn?** Audit shows the field is currently consumed somewhere in the engine. Step 2 must preserve this semantic — the engine reads `output.inject_to_context` before calling `into_tool_result()`.

3. **Are there any in-tree `Tool` impls in repos NOT covered by this plan (i.e., motosan-chat, motosan-agent-workflow)?** Yes — both per the audit. They're deferred. Wade decides when to do those refactors.

4. **License mismatch:** primitives is Apache-2.0; tool is MIT; loop is MIT. Mixing is legally fine (MIT-compatible-with-Apache-2.0) but inconsistent. Out of scope for this plan; raise as a separate cleanup ticket.

## 9. Estimated effort

| Step | Mechanical | Judgment | Total |
|---|---|---|---|
| 0. Primitives `ContentBlock::Json` variant (D-T4) | 15 min | 15 min (rustdoc + serde test) | 30 min |
| 1. motosan-agent-tool 0.4 | 2 h (13 remaining tools — 7 already done) | 1 h (annotations review, commit hygiene) | 3 h |
| 2. motosan-agent-loop 0.23 | 10 h (47 files, ~19 fixtures, redact migration, Extension trait sig sweep) | 6 h (Extension trait redesign, engine boundary, redact JSON test, cancellation feature decision, motosan-ai feature) | 16 h |
| 3. motosan-ai 0.16 | 30 min | 30 min | 1 h |
| 4. motosan-agent-subagent | 2 h | 1 h | 3 h |
| 5. motosan-sandbox | 30 min | 30 min | 1 h |
| 6. motosan-agent-harness 0.1.1 | 30 min | 30 min | 1 h |
| | | **Total** | **~25.5 h** |

The audit estimated 5–10 days for the whole M8 (10 repos). This plan covers 6 of those 10 in ~3 focused workdays; the deferred 4 (workflow HEAVY, chat HEAVY, chain TRIVIAL, plus harness already counted) add another ~2 days. The bump from v1's "~24 h" reflects the Extension trait scope expansion (Blocker #2 from review) and the primitives `ContentBlock::Json` addition.

## 10. What this plan does NOT cover

- Publishing any crate to crates.io
- Updating crates.io/docs.rs metadata
- motosan-agent-workflow (HEAVY, deferred per Wade)
- motosan-chat / motosan-chat-tool (HEAVY, deferred per Wade)
- motosan-chain (TRIVIAL, but deferred)
- The Phase D / Phase E work (CLI runner + finance harness) — blocked on this plan completing
