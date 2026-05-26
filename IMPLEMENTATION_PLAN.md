# motosan-agent-primitives — Implementation Plan

Status: **APPROVED — all 12 decisions answered, M0 ready to start**
Owner: Wade
Last updated: 2026-05-26 (D1-D10 + naming + license all resolved)

---

## 1. Purpose & position

This crate is the **contract layer** of the Motosan agent framework. It defines
the shared types and abstract middleware traits that every other crate in the
family agrees on, **without owning the `Tool` or `Harness` trait** — those
live in their own crates so this one stays a thin contract layer with only
a minimum runtime dep (`tokio-util` sync, see Section 2).

```text
   ┌────────────────────────────────────────────┐
   │ motosan-agent-harness-{finance,rental,…}   │  vertical implementations
   ├────────────────────────────────────────────┤
   │ motosan-agent-loop      (ReAct engine)     │  runs Harness + Tools
   ├────────────────────────────────────────────┤
   │ motosan-agent-harness   (Harness trait)    │  composition contract  [NEW]
   ├────────────────────────────────────────────┤
   │ motosan-agent-tool │ motosan-ai │ sandbox  │  capability + infra
   ├────────────────────────────────────────────┤
   │ motosan-agent-primitives  ← THIS CRATE     │  shared types + Hook + Permission
   └────────────────────────────────────────────┘
```

It exists to:

1. Eliminate the implicit duplication between `motosan-agent-loop`
   (`Message`, `MessageId`, `Role`, multimodal `ContentBlock`) and
   `motosan-ai` (`ChatRequest`, `ChatResponse`).
2. Give third-party tool / hook / middleware authors a minimal-dependency
   crate to target (just `tokio-util` for `CancellationToken`; no agent
   loop, no sandbox, no LLM client, no provider SDKs).
3. Provide a single source of truth for serialization schemas, ready for
   future IPC / FFI work (app-server, pyo3 binding, etc.).

---

## 2. Scope

### IN

- Pure data types: `Message`, `ContentBlock`, `ToolCall`, `ToolResult`,
  `ToolAnnotations`, `Permission`, `AgentEvent`, ...
- Abstract `async` traits that **do not depend on `Tool` trait**:
  `Hook`, `PermissionPolicy`.
- `MemorySchema` declaration types (schema only, no storage).
- Streaming event enum (`AgentEvent`).
- Re-exports surface (single `pub use` at the crate root).
- **Minimum runtime dep**: `tokio-util` (sync feature) for `CancellationToken`
  in Hook contexts. Was originally targeted as zero-runtime-dep; relaxed
  after Hook cancellation research.

### OUT — moved to other crates

| Type / trait                        | Home crate                       |
|-------------------------------------|----------------------------------|
| `Tool` trait                        | `motosan-agent-tool` (existing)  |
| `ToolContext`                       | `motosan-agent-tool`             |
| `ToolOutput`                        | `motosan-agent-tool`             |
| `ToolError`                         | `motosan-agent-tool`             |
| `Harness` trait                     | `motosan-agent-harness` (**new**) |
| `ChatRequest` / `ChatResponse`      | `motosan-ai`                     |
| Sandbox `Policy` etc.               | `motosan-sandbox`                |

### Also OUT — never belongs here

- Any concrete implementation (no default tools, no default hooks).
- Any I/O, runtime, or async executor.
- Built-in middleware (autocompact, ask_user, redact — in
  `motosan-agent-loop` as `impl Hook`).

### Deliberate non-goals

- **No `Default` for trait objects** — every trait will be explicit at
  registration time. No "magic" registries.
- **No builder pattern in primitives** — builders belong in
  `motosan-agent-loop` or a separate `motosan-agent-builders` crate.

---

## 3. Module layout (intended, no code yet)

| Module       | Public types / traits                                              | LOC | Notes |
|--------------|--------------------------------------------------------------------|-----|-------|
| `message`    | `Message`, `MessageId`, `Role`, `ContentBlock`, `ImageSource`, `DocumentSource` | ~150 | Multi-modal single enum |
| `tool`       | `ToolCall`, `ToolResult`, `ToolAnnotations` (**data only**)        | ~80  | `Tool` trait is in `motosan-agent-tool` |
| `permission` | `PermissionPolicy` trait, `Permission`, `PermissionMode`, `PermissionContext` | ~90  | One policy per session |
| `hook`       | `Hook` trait, `HookResult`, **9** lifecycle `*Ctx` structs (PostToolUse split into success/failure), `StopReason` | ~210 | Multiple hooks per session, default `Continue`. Ctx structs carry `CancellationToken` |
| `event`      | `AgentEvent` enum (10 variants), `SubagentResult`                  | ~110 | Streaming output |
| `memory`     | `MemorySchema`, `MemoryKey`, `MemoryKind`                          | ~40  | Schema only — no storage |
| `lib.rs`     | crate doc + re-exports                                              | ~40  | flat `pub use *` |

Target total: **~720 LOC including doc comments**. If it grows past 1200 we
have abstracted something we shouldn't have.

**Cargo.toml impact from research-driven changes**: primitives now depends
on `tokio-util` (sync feature) for `CancellationToken` in Hook contexts.
This pulls in tokio transitively. The "zero runtime dep" goal is relaxed
to "minimum runtime dep" — tokio is universal in Rust agent code so the
practical cost is zero, but record this honestly.

Removed from original plan: `Tool` trait + `ToolContext` + `ToolOutput`
(moved to `motosan-agent-tool`), `Harness` trait (moved to new
`motosan-agent-harness`), `error` module (`ToolError` moved with `Tool`).

---

## 4. Design decisions (all answered)

All 12 decisions were resolved on 2026-05-26. The full trade-off analysis is
kept below for future reference / onboarding new contributors. Final
answers are also summarised in Section 8 as a flat table.

Three of these were flipped from their original recommendation after
research / push-back (marked **REVISED / flipped**): D1, D2, D5. Treat
those as the "we learned something" decisions worth re-reading first.

### D1. Tool trait location — REVISED

- **Option A** (original): Tool trait in `motosan-agent-primitives`. Rename
  `motosan-agent-tool` to `motosan-agent-tools-std` (becomes std tool pack).
- **Option B** (revised, recommended): Tool trait **stays in
  `motosan-agent-tool` (no rename)**. Add new crate `motosan-agent-harness`
  for the `Harness` trait. `primitives` holds only `ToolCall` / `ToolResult` /
  `ToolAnnotations` (data types).

Trade-off: A is a simpler dep graph (one crate to depend on) at the cost of
renaming an existing repo and breaking downstream. B preserves all existing
crates as-is, at the cost of adding `motosan-agent-harness` crate.

**Recommended: B**. Zero breaking changes to existing `motosan-agent-tool`;
clean SRP (each trait its own crate); only cost is one new crate name.

Status: **ANSWERED — Option B**

### D2. How do hooks rewrite context? — REVISED after research

- **Option A** (original): mutate via `&mut Ctx`. Hook receives
  `&mut PreToolUseCtx`, mutates fields, returns `HookResult::Continue`.
- **Option B** (chosen, flipped after research): return-by-value. Hook
  returns `HookResult::Continue { updated_input: Option<Value> }`.

**Decision flipped to Option B** after researching Codex + Claude Agent SDK:

- **Codex** (`hook_runtime.rs:54`) uses
  `Continue { updated_input: Option<Value> }, Blocked(String)` — explicit
  return-by-value.
- **Claude Agent SDK** docs say "Always return a new object rather than
  mutating the original tool_input".
- Original assumption that "Agent SDK uses A" was **incorrect** — TS uses
  return-by-value; Python's dict mutation is a Python idiom, not a design
  endorsement.

**Why this matters under D5=A (cancellation)**:

With `&mut Ctx`, if a hook is cancelled mid-mutation the ctx is in a
half-mutated state with no rollback. With return-by-value, cancellation
simply discards the return value — the original ctx is untouched.

**Cost of B**: hooks doing rewrites need slightly more ceremony
(`Continue { updated_input: Some(new) }` instead of `ctx.field = new`).
Observation-only hooks (80% of hooks) are unaffected — just return
`Continue { updated_input: None }`.

Status: **ANSWERED — Option B** (flipped after research)

### D3. PermissionPolicy composition semantics

When multiple stacked harnesses each declare a `PermissionPolicy`, how do
they combine?

- **Option A**: most-restrictive wins (any `Deny` denies; otherwise any
  `AskUser` asks; otherwise allowed). Safer.
- **Option B**: registration order matters — first policy decides.
- **Option C**: harnesses can declare a `priority` and the highest priority
  policy wins.

**Recommended: A** for safety; the framework composes harness policies into
a single `CompositePolicy` that the agent loop sees.

Status: **ANSWERED — Option A**

### D4. What does `PermissionMode::Plan` actually mean?

- **Option A** (strict): deny ALL tools including read-only.
- **Option B** (conservative, original recommendation): deny `destructive`
  AND `network_access`; allow `read_only`.
- **Option C** (chosen): deny only `destructive`; allow `read_only` AND
  `network_access`. Lets agent fetch docs / research while planning.

**Final decision: Option C** — chose more permissive than the original
recommendation. Rationale: motosan framework's research / browsing use
cases need network during plan mode. Risk: a tool with `network_access = true`
that actually mutates remote state (e.g., POST request) will slip through;
mitigated by relying on accurate `destructive` annotation from tool authors.

Status: **ANSWERED — Option C**

### D5. Cancellation in `ToolContext`?

Do we include a `CancellationToken` in `ToolContext`?

- **Option A** (chosen, after Codex research): yes — adds `tokio-util`
  dependency to `motosan-agent-tool`.
- **Option B** (original recommendation, overridden): no — `ToolContext`
  stays runtime-free; drop = cancel.

**Decision flipped to Option A** after researching Codex's `ToolInvocation`
struct in `core/src/tools/context.rs:53-63`, which contains
`pub cancellation_token: CancellationToken` directly. Codex's shell exec
(`core/src/unified_exec/process.rs:197-213`) explicitly calls
`self.cancellation_token.cancel()` to terminate subprocesses — drop alone
doesn't kill child processes, so explicit token is required for shell tools.

Original Option B rationale ("95% of tools don't need it") was correct for
API-only tools but wrong for any framework that wants to support shell /
subprocess / long-running tools. Token is a single field; tools that don't
need it can ignore it.

Status: **ANSWERED — Option A** (CancellationToken in ToolContext)

### D6. Time type: `chrono::DateTime<Utc>` or `std::time::SystemTime`?

- `chrono`: richer API, well-known, adds dep.
- `SystemTime`: zero-dep.

**Recommended: chrono**, `default-features = false`, only `serde` + `clock`.
Matches Goose / Rust agent ecosystem convention.

Status: **ANSWERED — chrono with `default-features = false`**

### D7. `async-trait` macro vs native AFIT?

- `async-trait` (Box-based): well-tested, object-safe.
- Native AFIT (Rust 1.75+): zero-overhead, but `dyn Trait` is painful.

**Recommended: `async-trait`**. We require `dyn Hook` / `dyn PermissionPolicy`
everywhere.

Status: **ANSWERED — `async-trait` macro**

### D8. Memory schema scope

Should `MemorySchema` be in primitives at all? Memory implementation is
out of scope; schema declaration is small.

- **Option A**: include `MemorySchema` so Harness can declare its memory
  keys.
- **Option B**: drop for v0.1.0, add when needed.

**Recommended: A** — small cost, real value.

Status: **ANSWERED — Option A** (MemorySchema included)

### D9. Where does `ToolContext` live? [NEW after D1-revised]

`ToolContext` holds `session_id` + `tool_use_id`, passed to `Tool::call`.

- **Option A**: in `motosan-agent-tool` (next to `Tool` trait).
- **Option B**: in `motosan-agent-primitives` (along with `PermissionContext`
  / Hook contexts — unified "session identity" types).

Trade-off: A keeps `Tool` trait self-contained but creates near-duplicate
context structs across crates. B unifies "session identity" in one place
but slightly couples primitives to Tool concerns.

**Recommended: A** (in `motosan-agent-tool`). The two-field `ToolContext`
is trivial to duplicate compared to the cross-crate dep cost.

**Note**: per D5-flipped, `ToolContext` now also carries a
`CancellationToken`, so it's three fields. Still small, still belongs with
`Tool` trait.

Status: **ANSWERED — Option A** (ToolContext in motosan-agent-tool)

### D10. Where does `ToolAnnotations` live? [NEW after D1-revised]

`ToolAnnotations` is declared by `Tool::annotations()` and consumed by
`PermissionPolicy` (inside `PermissionContext`).

- **Option A**: in `motosan-agent-primitives` (so `PermissionContext`
  doesn't need a `motosan-agent-tool` dep).
- **Option B**: in `motosan-agent-tool` (so `Tool` trait is self-contained).

Trade-off: A means `motosan-agent-tool` depends on `primitives` for
`ToolAnnotations` (natural — primitives is the leaf). B means primitives
must depend on `motosan-agent-tool` for `PermissionContext` to compile —
**reverses the dep direction we want**.

**Recommended: A** (in primitives). The dep direction settles it.

Status: **ANSWERED — Option A** (ToolAnnotations in primitives)

---

## 5. Milestones

### M0: Repo & scaffolding — 0.5 day

- Create `motosan-dev/motosan-agent-primitives` on GitHub
- `Cargo.toml` deps:
  - `async-trait` (per D7)
  - `chrono` `default-features = false`, features `["serde", "clock"]` (per D6)
  - `serde` + `serde_json`
  - `thiserror`
  - `uuid` `features = ["v4", "serde"]`
  - `tokio-util` `features = ["sync"]` — for `CancellationToken` in Hook ctx
- Empty modules + `lib.rs` re-exports (modules per Section 3)
- README with scope statement
- `.gitignore`, Apache-2.0 LICENSE (per Naming + License decisions), basic CI yaml

**Acceptance**: `cargo check` passes on empty modules.

### M1: Message types — 1 day

- Implement `message.rs` (Message, MessageId, Role, ContentBlock,
  ImageSource, DocumentSource)
- Round-trip serde tests for every variant
- Convenience constructors (`Message::text`, `Message::tool_calls`,
  `Message::tool_results`)

**Acceptance**:
- `cargo test` passes serde round-trips
- A handwritten JSON sample for each `ContentBlock` variant exists in
  `tests/fixtures/`

### M2: Tool data types — 0.5 day [scope reduced from original]

- Implement `tool.rs` with **data types only**: `ToolCall`, `ToolResult`,
  `ToolAnnotations` (per D10 → in primitives).
- No `Tool` trait, no `ToolContext`, no `ToolOutput`, no `ToolError` here —
  those live in `motosan-agent-tool` (per D1-B, D9).
- Convenience constructors (`ToolResult::text`, `ToolResult::error`).
- `ToolAnnotations` rustdoc must include **load-bearing warning**: "Tool
  authors MUST set `destructive: true` accurately. Under
  `PermissionMode::Plan` (D4=C), only `destructive` blocks execution;
  mis-annotation can let a network-mutating tool slip through plan mode."

**Acceptance**:
- `cargo test` passes serde round-trips
- Doc tests demonstrate a `ToolCall` round-trip
- `ToolAnnotations` rustdoc contains the destructive-warning paragraph

### M3: Permission system — 0.5 day

- Implement `permission.rs` per D3 (most-restrictive-wins composition) and
  D4 (Plan mode = allow read_only + network, deny destructive only):
  `PermissionPolicy`, `Permission`, `PermissionMode`, `PermissionContext`
- Document composition semantics in `permission.rs` rustdoc
- **`PermissionMode::Plan` rustdoc must spell out**: "Reads
  `ToolAnnotations.destructive`; tools with `destructive: false` may run
  in plan mode even if they hit the network. Tool authors are responsible
  for accurate annotations."
- Object-safety test for `Arc<dyn PermissionPolicy>`

**Acceptance**:
- `Arc<dyn PermissionPolicy>` compiles
- Doc tests demonstrate Allow / Deny / AskUser cases
- `PermissionMode::Plan` rustdoc contains the destructive-trust paragraph

### M4: Hook surface — 1.5 days [scope expanded]

- Implement `hook.rs` per **D2-revised** (return-by-value
  `Continue { updated_input: Option<Value> }, Skip { reason }, Abort { reason }`):
  `Hook` trait, `HookResult`, **9 lifecycle Ctx structs**, `StopReason`
- 9 lifecycle methods (defaults to `Continue { updated_input: None }`):
  `session_start`, `session_end`, `user_prompt_submit`,
  `pre_tool_use`, `post_tool_use` (success), **`post_tool_use_failure`** (cancel/error, new),
  `pre_compact`, `stop`, `subagent_stop`
- Every Ctx struct carries `cancellation_token: CancellationToken`
  (mirrors Tool side per D5; needed because hooks can be async and
  long-running, especially `pre_tool_use` doing PII redaction etc.)
- Object-safety test for `Arc<dyn Hook>`
- Document: **mutation via return value only, never `&mut Ctx`** — explain
  why (cancellation safety)
- Document: **`post_tool_use` fires on success, `post_tool_use_failure`
  fires on tool error / cancel** — audit hooks override both, rewrite
  hooks usually only `post_tool_use`

**Acceptance**:
- Trivial `LoggingHook` example compiles (overrides both `post_tool_use`
  variants)
- Doc tests demonstrate `Continue { updated_input: Some(...) }`, `Skip`,
  `Abort`
- Doc test demonstrates a hook respecting `cancellation_token`

### M5: Event stream + Memory schema — 0.5 day

- Implement `event.rs` (AgentEvent, SubagentResult)
- Implement `memory.rs` per D8=A (MemorySchema, MemoryKey, MemoryKind)
- Serde round-trip tests

**Acceptance**:
- JSON schema for each `AgentEvent` variant matches expected shape
- Doc tests demonstrate streaming consumption

### M6: Documentation pass — 0.5 day

- Every public item has rustdoc
- `cargo doc --no-deps --document-private-items` produces clean output
- README explains scope and stability promise
- Crate-level docs include the layering diagram

**Acceptance**: `cargo doc --no-deps -- -D missing_docs` passes; README updated.

### M7: New crate `motosan-agent-harness` — 1 day [NEW after D1-revised]

- Create `motosan-dev/motosan-agent-harness` GitHub repo
- `Cargo.toml` depending on `motosan-agent-primitives` + `motosan-agent-tool`
- Implement `Harness` trait
- Document composition rules (tool-name uniqueness, hook ordering, policy
  composition)

**Acceptance**:
- `cargo check` passes
- A `NullHarness` example compiles
- A `TwoToolHarness` example compiles using a stub Tool from
  `motosan-agent-tool`

### M8: Downstream wiring — 5-10 days [estimate increased]

**Step 1 — Audit (0.5 day, do this BEFORE any code)**:
- `grep -rn "motosan_agent_tool::" ~/Projects/wade/motosan-*` to enumerate
  every crate that uses `motosan-agent-tool`'s types
- List which symbols each downstream uses (Tool trait, ToolCall, ToolResult,
  etc.) — affects how invasive the refactor will be
- Result: a concrete list of impacted repos and files, pasted into M8 docs

**Step 2 — Refactor (4-9 days based on audit)**:
- Adopt primitives in `motosan-agent-loop`:
  - Replace internal `Message` / `Role` / `ContentBlock` with primitives
  - Rename existing `Extension` trait → `Hook` (matches new naming)
  - Add `Harness` builder method (uses `motosan-agent-harness::Harness`)
- Adopt primitives in `motosan-ai`:
  - Make `ChatRequest` consume `&[primitives::Message]`
  - Conversion at the provider layer only
- Adopt primitives in `motosan-agent-tool`:
  - `Tool` trait references `primitives::{ToolCall, ToolResult,
    ToolAnnotations}` instead of its own types
- For every other repo found in Step 1: update or pin to old version

**Acceptance**:
- All crates from Step 1 audit compile against new primitives
- All existing tests pass
- Audit list is committed to `M8_AUDIT.md` for traceability

### M8.5: Minimal runner for harness validation — 2-3 days [NEW]

Without something to actually invoke an agent, M9's "buy 10 shares of AAPL"
demo cannot run. Build the smallest possible runner:

- New repo `motosan-dev/motosan-agent-cli` (≤300 LOC binary crate)
  - Reads prompt from CLI arg or stdin
  - Loads a configured `Harness` and `Provider`
  - Streams `AgentEvent` as JSONL to stdout
  - **No TUI**, no fancy formatting, no config file system — JSONL only
- Alternative: implement as `cargo run --example chat` in
  `motosan-agent-loop` if you don't want a new repo

This is **scaffolding**, not a product — it exists only so M9 has something
to drive. A real CLI / TUI is out of scope until after 1.0.0.

**Acceptance**:
- `echo "list files in /tmp" | motosan-agent-cli --harness null` produces
  a sensible JSONL event stream
- Can be killed mid-run via Ctrl-C without leaving zombie processes

### M9: First harness consumer — 2-5 weeks [estimate widened]

**Step 0 — M9 gate (BEFORE writing any code, 1-2 days)**:

This is the mitigation for the "Harness model is structurally wrong" risk
(Section 6, top of risk table).

- Write a 1-page **sequence diagram** of a finance use case end-to-end
  ("user asks: buy 10 AAPL if under $200" → agent loops → `place_order`
  with approval), using ONLY the planned `Harness` / `Hook` /
  `PermissionPolicy` / Tool / loop event APIs.
- If the diagram requires concepts not yet in the API (multi-agent
  orchestration, shared state between hooks, etc.), **stop and redesign
  primitives** before writing any harness code.
- Output: committed to `M9_GATE_DIAGRAM.md` in the finance harness repo.

**Step 1 — Implementation (after gate passes)**:

- Build `motosan-agent-harness-finance` (separate repo, depends on
  `motosan-agent-harness`)
- Implement 3-5 finance tools (`get_quote`, `get_position`, `place_order`,
  `backtest`)
- Implement `FinanceApprovalPolicy` (place_order needs confirmation,
  read-only auto-allow)
- Implement `AuditLogHook` (logs every tool call to file; overrides both
  `post_tool_use` and `post_tool_use_failure` per D2-B)
- Implement `FinanceHarness::system_prompt` (domain persona)
- **Document every place where primitives or Harness trait felt awkward** —
  these become M10 inputs

**Acceptance**:
- `M9_GATE_DIAGRAM.md` exists and passes inspection (no missing concepts)
- A demo where the agent does "buy 10 shares of AAPL if it's under $200"
  works end-to-end
- The "awkwardness list" has ≥3 concrete items

### M10: Refactor based on consumer feedback — 1-2 weeks

- Address the awkwardness list from M9
- Breaking changes to primitives / harness are OK (still 0.x)
- Bump to 0.2.0

**Acceptance**: all ≥3 awkwardness items resolved or explicitly punted with
rationale; finance harness migrated to 0.2.0.

### M11: Second harness validates → freeze 0.1.0 → publish — 1 week

- Build `motosan-agent-harness-rental` against 0.2.0
- If it surfaces NEW primitives changes, bump to 0.3.0 and migrate finance
- If it works clean → freeze API → bump to 1.0.0
- Publish `motosan-agent-primitives` + `motosan-agent-harness` to crates.io
- Tag GitHub releases with CHANGELOG

**Acceptance**:
- `cargo publish` succeeds for both crates
- Both finance and rental harnesses compile against published versions

---

## 6. Risks

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| **Harness model is structurally wrong** — finance reveals that "one agent + stacked Harnesses" doesn't capture the domain (e.g. need genuine multi-agent orchestration that Harness can't express) | Medium | **Catastrophic** | **Dry-run BEFORE M9**: hand-write a 1-page sequence diagram of the finance use case using only the planned `Harness` / `Hook` / `PermissionPolicy` API. If the diagram requires concepts not in the API, redesign primitives BEFORE writing any harness code. This is M9's gate. |
| **motosan-agent-tool migration blast radius unknown** — Tool trait signature change ripples to unaudited downstream | High | High | M8 Step 1 audit explicitly enumerates impacted repos before any refactor; estimate widens accordingly |
| **Premature abstraction** — designing Harness trait without real consumer | High | High | M9 validates BEFORE M11 freeze; first harness is non-trivial finance, not toy |
| **Type churn** — Message / ContentBlock shape changes after motosan-ai/loop adoption | Medium | Medium | M8 ("downstream wiring") finds churn early before M9 |
| ~~`ToolAnnotations` / `ToolContext` placement wrong~~ | — | — | **Resolved by D9 (ToolContext in tool) + D10 (ToolAnnotations in primitives)** |
| **Solo dev burnout** — 12-15 weeks (realistic) before first harness ships value | High | High | M8-M9 prioritise visible value per milestone; if a phase overruns 50%, re-baseline rather than power through |
| **Hook lifecycle list wrong** — only learn when implementing complex hook | Medium | Low | Goose has 13 events, we have 9 (PostToolUse split into success / failure per Agent SDK pattern). Additive in 0.2.0 if needed — not breaking |
| **`async-trait` deprecation** | Low | Low | Migrate at major version; not a 0.x concern |

---

## 7. Estimate

Two columns: **Best case** assumes everything goes right (no D-revisits, no
ecosystem surprises). **Realistic** assumes normal solo-dev attrition
(scope creep, motosan-agent-tool blast radius, life happens).

| Phase                                        | Best case  | Realistic   | Calendar (realistic) |
|----------------------------------------------|------------|-------------|----------------------|
| M0–M6 (the crate itself)                     | 4 days     | 6-8 days    | Week 1-2             |
| M7 (`motosan-agent-harness` crate)           | 1 day      | 2 days      | Week 2               |
| M8 (downstream wiring **+ audit**)           | 3 days     | 5-10 days   | Week 3-4             |
| M8.5 (minimal runner)                        | 2 days     | 2-3 days    | Week 4               |
| **M9 gate**: Harness dry-run diagram         | 0.5 day    | 1-2 days    | Week 5               |
| M9 (finance harness end-to-end)              | 10-15 days | 15-25 days  | Week 5-9             |
| M10 (refactor)                               | 5-10 days  | 10-15 days  | Week 10-12           |
| M11 (rental + publish)                       | 5-7 days   | 7-14 days   | Week 13-15           |

**Best case: ~8 weeks** to 1.0.0.
**Realistic: ~12-15 weeks** (3-4 months) at solo part-time pace.

Plan against the realistic column; treat best-case as the "everything went
right" stretch goal. If you hit a 50% overrun on any phase, that's normal —
re-baseline and continue.

---

## 8. Decisions (ANSWERED)

All 12 decisions resolved on 2026-05-26. M0 can start.

| # | Question | Answer |
|---|---|---|
| D1 | Tool trait location | **Option B**: stays in `motosan-agent-tool`; new `motosan-agent-harness` crate for Harness trait |
| D2 | Hook context mutation | **Option B** (flipped after Codex + Agent SDK research): return-by-value `Continue { updated_input: Option<Value> }`. Hook ctx ALSO carries `CancellationToken`. PostToolUse splits into success + failure variants. |
| D3 | PermissionPolicy composition | **Option A**: most-restrictive wins (Deny > AskUser > Allow); order-independent |
| D4 | `PermissionMode::Plan` semantics | **Option C**: allow `read_only` AND `network_access`; deny only `destructive`. More permissive than original recommendation B; chosen for research / browsing use cases |
| D5 | CancellationToken in ToolContext | **Option A** (flipped from B): yes, include. Based on Codex's `ToolInvocation` design. `motosan-agent-tool` will dep `tokio-util` |
| D6 | Time type | **chrono** with `default-features = false`, features `serde` + `clock` |
| D7 | Async trait machinery | **`async-trait` macro** (needed for object safety on `dyn Hook` etc.) |
| D8 | MemorySchema in primitives | **Option A**: include. Small cost, schema declaration is useful for Harness |
| D9 | ToolContext location | **Option A**: `motosan-agent-tool` (3 fields: session_id, tool_use_id, cancellation_token) |
| D10 | ToolAnnotations location | **Option A**: `motosan-agent-primitives` (needed by `PermissionContext`; dep direction stays clean) |
| Naming | crate names on crates.io | `motosan-agent-primitives` + `motosan-agent-harness` |
| License | OSS license | **Apache-2.0** (matches motosan-* defaults) |

### Knock-on effects to record

- **D4 → C** means tool authors must annotate `destructive: true` accurately,
  otherwise plan mode is unsafe. Document this clearly in `Tool` rustdoc.
- **D5 → A** means `motosan-agent-tool`'s `Cargo.toml` gains `tokio-util`
  with the `sync` feature (for `CancellationToken`).
- **D2 → B (flipped after research)** means:
  - Hook trait uses return-by-value, not `&mut Ctx`
  - `motosan-agent-primitives` now also depends on `tokio-util` (sync feature)
    because Hook contexts carry `CancellationToken`. "Zero runtime dep"
    becomes "minimum runtime dep" — tokio is universal in Rust agent code.
  - PostToolUse splits into `post_tool_use` (success) + `post_tool_use_failure`
    (cancel / error). 8 → 9 lifecycle events.
- The draft code in `src/tool.rs` and `src/hook.rs` (Appendix A) is now wrong
  on more counts (see Appendix A for the full list).

---

## 9. Out of scope (intentionally)

These are explicitly NOT in this plan:

- `motosan-agent-app-server` (daemon for IDE plugins) — future v0.2 work
- pyo3 / napi-rs bindings — future after 1.0.0
- **MCP client integration** — separate `motosan-agent-mcp` crate.
  **Forward-compatibility note**: the current `Tool` trait is an in-process
  Rust callback (per D1=B, lives in `motosan-agent-tool`). Goose's lesson
  is that "tool = MCP server" is the right long-term model. We are NOT
  doing that now to avoid scope creep, but the `Tool` trait should be
  designed defensively:
    - Avoid `&Agent` / `&Session` borrowed-state in tool signatures
      (those don't serialize for out-of-process MCP).
    - Keep `input: serde_json::Value` (already MCP-compatible).
    - Treat `Tool` impls as if they'd be wrapped as MCP servers later.
  Done right, the future `motosan-agent-mcp` is an adapter, not a Tool
  rewrite.
- TUI / CLI — application layer, not framework
- Eval harness — separate `motosan-agent-eval` crate
- Migration of `motosan-ai` provider types into primitives — those stay in
  `motosan-ai` (provider-internal, not cross-cutting)

---

## Appendix A: Existing draft code

While drafting an earlier version of this plan I wrote `.rs` files at
`src/{lib,message,tool,permission,hook,harness,event,memory,error}.rs`.

**These files predate the answered decisions** and are wrong on multiple
counts:

- `src/tool.rs` — contains `Tool` trait + `ToolContext` + `ToolOutput`.
  Per D1=B, only `ToolCall` / `ToolResult` / `ToolAnnotations` stay here;
  the rest moves to `motosan-agent-tool`. **Additionally** the draft's
  `ToolContext` has no `CancellationToken` — under D5=A (flipped) it must
  add one when it lives in `motosan-agent-tool`.
- `src/harness.rs` — must be **removed**; `Harness` trait moves to its own
  crate `motosan-agent-harness` (per D1=B).
- `src/error.rs` — `ToolError` must be **removed** here (moves to
  `motosan-agent-tool` per D1=B).
- `src/hook.rs` — wrong on **three** counts:
  - Uses `&mut Ctx` mutation pattern → must change to return-by-value
    `Continue { updated_input: Option<Value> }` (per D2 flipped to B).
  - Has 8 lifecycle methods → must add `post_tool_use_failure` (9 total).
  - Ctx structs lack `cancellation_token` field → must add to every Ctx.

Action options:

- Keep the draft as inspiration for code shape, but rewrite to match the
  answered decisions during M1-M5.
- `rm -rf src/*.rs` to start clean (recommended for clarity now that
  the answers diverge from the draft on multiple points).
- Diff against the final design after M2-M5 as a learning exercise.

The plan takes precedence over the draft code.
