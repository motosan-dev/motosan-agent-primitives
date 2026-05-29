# M10 Sub-Agent Dispatch Prompts

Copy each phase verbatim into a fresh sub-agent. Run them **in order** (Phase N depends on Phase N-1 being committed + pushed).

**Plan reference:** [M10_PLAN.md](M10_PLAN.md). Read §3 (design decisions D-M10-1..8) and §5 (acceptance gates) before any dispatch.

**Pre-conditions verified (2026-05-29):**
- primitives @ `4aff1b0` clean on main, with `documentation` + `readme` metadata
- All 8 repos clean and in sync (post-publish-prep)
- llms.txt + CHANGELOG drift fixed at `c8d3d34` (ai), `60c00e8` (harness), `e4a725c` (loop)

**Inter-phase verification:**
```
cd <repo> && git log -1 --oneline && cargo build --locked && cargo test
```

**Chain-verify after Phase H:**
```
for r in primitives tool loop ai subagent harness sandbox agemo; do
  echo "=== $r ===" && cd /Users/daiwanwei/Projects/wade/motosan-agent-$r && \
    git log -1 --oneline && cargo build --locked && cargo test
done
```

---

## Phase A — Primitives 0.2.0 (D-M10-2 + D-M10-3 only)

```
You are executing Phase A of the M10 migration plan: additive field changes to two ctx structs in motosan-agent-primitives, then bumping to 0.2.0.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-primitives/

## Pre-conditions
- Branch: main, clean working tree (verify with `git status`)
- HEAD: 4aff1b0 (publish-prep)

## Required reading
1. M10_PLAN.md §3 D-M10-2 and §3 D-M10-3 (exact field specs)
2. src/hook.rs lines 218-251 (current PostToolUseFailureCtx shape)
3. src/permission.rs lines 112-130 (current PermissionContext shape)
4. src/tool.rs — find ToolResult definition (needed for D-M10-2)
5. src/message.rs (or wherever Message is defined) — needed for D-M10-3

## Scope (IN)
- D-M10-2: Add `pub result: ToolResult` field to `PostToolUseFailureCtx`
- D-M10-3: Add `pub recent_messages: &'a [Message]` field to `PermissionContext<'a>`
- Bump Cargo.toml: 0.1.1 → 0.2.0
- CHANGELOG.md: new 0.2.0 entry covering both changes
- Update all in-tree call sites (test fixtures construct ctx structs literally)

## Scope (OUT — explicitly NOT this phase)
- D-M10-1 (session_id wiring) — lives in loop, Phase C
- D-M10-4 (ToolDef.internal_name) — lives in tool, Phase B
- ANY change to Hook trait signatures (these are additive field adds only)

## Open question resolutions (locked in from §7)
- recent_messages uses `&'a [Message]` borrowed slice (not owned Vec) — keeps PermissionContext zero-alloc
- result is owned `ToolResult` in PostToolUseFailureCtx (matches existing PostToolUseCtx.tool_result pattern)

## Acceptance
- `cargo build --locked` green
- `cargo test` green (account for any test fixtures constructing the changed structs — add the new field literally)
- CHANGELOG.md has 0.2.0 entry mentioning both D-M10-2 and D-M10-3 with rationale
- Cargo.toml shows `version = "0.2.0"`
- Commit message: `feat!: primitives 0.2.0 — PostToolUseFailureCtx.result + PermissionContext.recent_messages (M10 D-M10-2 + D-M10-3)`
- Push to origin/main after green

## Report back
- HEAD sha after push
- Number of test fixtures updated
- Any surprises (lifetime annotations propagating, etc)
- Confirm cargo test count delta (should be ≥ existing baseline + 1-2 new tests covering the additive fields)

DO NOT touch: anything outside primitives, loop session_id wiring, ToolDef internal_name.
```

---

## Phase B — Tool 0.5.0 (D-M10-4 ToolDef.internal_name)

```
You are executing Phase B of M10: add ToolDef.internal_name to motosan-agent-tool, bump to 0.5.0.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-tool/

## Pre-conditions
- Phase A pushed: primitives at 0.2.0 on origin/main
- Branch: main, clean working tree

## Required reading
1. M10_PLAN.md §3 D-M10-4
2. src/tool.rs — current ToolDef definition

## Scope (IN)
- Add `pub internal_name: String` field to `ToolDef`
- Default behavior: if not set explicitly, `internal_name = name.clone()` (backward compat)
- Update ToolDef::new constructor + any builder methods
- Bump Cargo.toml: 0.4.0 → 0.5.0
- CHANGELOG entry
- Update in-tree tools if any (the 20 built-in tools already set name; leave internal_name defaulted)

## Scope (OUT)
- Do NOT bump primitives dep (primitives 0.2.0 is path dep, already wired)
- Do NOT change tool collision logic — that lives in loop, Phase C

## Acceptance
- `cargo build --locked` + `cargo test` green
- All 20 built-in tools continue to compile without explicit internal_name (defaulting works)
- New unit test: ToolDef::new("foo") produces internal_name == "foo"
- Commit: `feat!: tool 0.5.0 — ToolDef.internal_name field (M10 D-M10-4)`
- Push to origin/main

## Report back
- HEAD sha after push
- Confirm built-in tool count compile-clean
- Any edge cases (ToolDef serde format change? if yes, document)
```

---

## Phase C — Loop 0.26.0 (consume primitives 0.2.0 + tool 0.5.0; implement D-M10-1)

```
You are executing Phase C of M10: bump loop to 0.26.0, consume new primitives/tool, implement D-M10-1 session_id flow + EngineBuilder setters.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-loop/

## Pre-conditions
- Phase A + B pushed: primitives 0.2.0, tool 0.5.0
- Branch: main, clean

## Required reading
1. M10_PLAN.md §3 D-M10-1 (full)
2. src/core/hook_adapter.rs — find HookInterceptorAdapter and the hardcoded "default" session_id helper
3. src/engine/builder.rs (EngineBuilder)
4. src/core/interceptor.rs LoopInterceptor trait

## Scope (IN)
- Cargo.toml: bump motosan-agent-primitives dep to 0.2.0; motosan-agent-tool to 0.5.0; loop version 0.25.1 → 0.26.0
- D-M10-1: Engine grows `session_id: String` field
  - `EngineBuilder::session_id(impl Into<String>)` setter (NEW)
  - Auto-generate UUID v4 if unset (add `uuid = { version = "1", features = ["v4"] }` dep if needed)
  - `HookInterceptorAdapter::new(name, hook, session_id)` accepts session_id at construction
  - Delete the hardcoded `session_id()` helper returning "default"; all ctx-struct construction sites populate from stored value
- D-M10-3 plumbing: `EngineBuilder::permission_context_window(usize)` setter, default 10
  - Engine maintains a sliding window of recent messages
  - Permission check sites populate `PermissionContext.recent_messages` from this window
- ANY adjustments needed because the body still references `*Extension` names — leave those alone (rename was 0.23, post-rename names already correct in code)
- Adjust in-tree LoopInterceptor impls if they construct ctx structs (add new fields with default Empty)
- ToolDef changes (Phase B) flow through — verify dispatch / collision check still uses `name` for LLM, `internal_name` for uniqueness

## Scope (OUT)
- Do NOT change loop's public surface beyond the new setters
- Do NOT touch motosan-ai (Phase D), subagent (Phase E), harness/finance (Phase F), agemo (Phase G)
- Do NOT add new Hook lifecycle methods (out of M10 per §2 OUT)

## Acceptance
- `cargo build --locked --all-features` green
- `cargo test --all-features` green
- NEW test: session_id flows from EngineBuilder through HookInterceptorAdapter to a Hook impl; assert NOT "default"
- NEW test: permission_context_window populates recent_messages with last N
- All existing tests pass; if a test relied on the "default" literal, update assertion to read what builder set
- Commit: `feat!: loop 0.26.0 — D-M10-1 session_id flow + permission_context_window (consume primitives 0.2.0 + tool 0.5.0)`
- Push to origin/main

## Report back
- HEAD sha after push
- Test count delta (should add ≥ 2 new tests)
- Confirm "default" literal no longer appears in hook_adapter.rs
- Any surprises with the LoopInterceptor body code still using `*Extension` aliases (it's intentional; the rename was at the struct level and aliases the old names — flag if you find genuinely broken references)
```

---

## Phase D — AI 0.17.0 (downstream of tool 0.5.0)

```
You are executing Phase D of M10: bump motosan-ai Rust SDK to consume tool 0.5.0.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-ai/

## Pre-conditions
- Phases A, B, C pushed
- Branch: main, clean

## Scope (IN)
- sdks/rust/Cargo.toml: motosan-agent-tool dep 0.4 → 0.5; version 0.16.0 → 0.17.0
- Update root Cargo.toml if it pins versions
- sdks/rust/CHANGELOG.md: 0.17.0 entry
- Root CHANGELOG.md: rust-0.17.0 entry pointing at sdks/rust
- Update llms.txt: "Python 0.12.0 · Rust 0.17.0" + install snippet
- Verify --features agent-tool still compiles

## Scope (OUT)
- Python SDK (untouched)
- Public Rust SDK API (no signature changes expected; bump is purely transitive crate identity)

## Acceptance
- `cd sdks/rust && cargo build --locked && cargo test` green
- `cd sdks/rust && cargo build --locked --features agent-tool` green
- Commit: `feat!: rust SDK 0.17.0 — bump motosan-agent-tool dep to 0.5 (M10 transitive)`
- Push

## Report back
- HEAD sha + confirm rust-0.17.0 in both CHANGELOGs and llms.txt
```

---

## Phase E — Subagent 0.5.0 (Cargo.toml + test fixtures)

```
You are executing Phase E of M10: bump motosan-agent-subagent to consume new primitives + loop.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-subagent/

## Pre-conditions
- Phases A, B, C, D pushed
- Branch: main, clean

## Scope (IN)
- Cargo.toml: primitives 0.1 → 0.2; tool 0.4 → 0.5; loop 0.25 → 0.26; version 0.4.0 → 0.5.0
- Migrate any test fixtures that construct PostToolUseFailureCtx (must add `result:` field) or PermissionContext (must add `recent_messages:` field)
- subagent's own Hook/PermissionPolicy impls READ fields — they don't need migration for additive struct changes
- CHANGELOG entry

## Acceptance
- `cargo build --locked --all-features` green
- `cargo test --all-features` green
- Commit: `chore!: subagent 0.5.0 — consume primitives 0.2.0 + tool 0.5.0 + loop 0.26.0 (M10 cascade)`
- Push

## Report back
- HEAD sha
- Count of test fixtures migrated (should be small — likely ≤ 5)
```

---

## Phase F — Harness 0.2.0 + Finance 0.2.0 (simplify AuditLogHook + add internal_name)

```
You are executing Phase F of M10: harness workspace cascade.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-harness/

## Pre-conditions
- Phases A-E pushed
- Branch: main, clean

## Required reading
1. finance/AWKWARDNESS.md — items 3, 4 (this phase resolves them)
2. finance/src/audit.rs — AuditLogHook (will be simplified)
3. finance/src/tools/*.rs — get internal_name added

## Scope (IN)
- root Cargo.toml workspace deps: primitives 0.1.1 → 0.2.0; tool 0.4.0 → 0.5.0
- harness/Cargo.toml: version 0.1.2 → 0.2.0; CHANGELOG entry
- finance/Cargo.toml: version 0.1.0 → 0.2.0; CHANGELOG entry
- Simplify finance/src/audit.rs AuditLogHook::post_tool_use_failure to read `ctx.result` directly instead of synthesizing one from `ctx.failure` (~5 LOC simpler)
- Add `internal_name: "finance.{tool}"` to all 3 finance tools (get_quote → finance.get_quote, get_position → finance.get_position, place_order → finance.place_order)
- Mark AWKWARDNESS.md items 3 + 4 as RESOLVED with M10 commit reference

## Scope (OUT)
- Do NOT change finance's Harness trait surface beyond what the primitives/tool bumps require
- Do NOT add new tools or change ApprovalPolicy logic (out of M10)

## Acceptance
- `cargo build --locked` (workspace root) green
- `cargo test` workspace green
- AuditLogHook simplification visible in `git diff finance/src/audit.rs`
- finance/AWKWARDNESS.md items 3+4 struck through with commit ref
- Commit (one bundled workspace commit): `feat!: harness + finance 0.2.0 — consume primitives 0.2.0; simplify AuditLogHook with ctx.result; finance tools gain internal_name (M10 closes AWK#3 + #4)`
- Push

## Report back
- HEAD sha
- LOC delta on audit.rs (should be negative)
- Confirm AWKWARDNESS.md updates
```

---

## Phase G — agemo 0.2.0 (wire EngineBuilder::session_id from agemo's wire session_id)

```
You are executing Phase G of M10: agemo CLI cascade + wire real session_id end-to-end.

## Working directory
/Users/daiwanwei/Projects/wade/agemo/

## Pre-conditions
- Phases A-F pushed
- Branch: main, clean

## Required reading
1. agemo's wire session_id source (look in src/wire/ or src/session/)
2. Where Engine is constructed in agemo (probably src/main.rs or src/runner.rs)

## Scope (IN)
- Cargo.toml: loop 0.25 → 0.26; primitives 0.1 → 0.2; tool 0.4 → 0.5; version 0.1.3 → 0.2.0
- At Engine construction site: call `.session_id(wire_session_id.to_string())` on EngineBuilder
- Verify `--harness finance` demo still works (run a smoke test)
- CHANGELOG entry

## Acceptance
- `cargo build --locked` + `cargo test` green
- Smoke test: `cargo run -- --harness finance --prompt "what's AAPL's price?"` — confirm completes without crash
- Audit log (if generated) now contains real session_id, not "default"
- Commit: `feat!: agemo 0.2.0 — consume loop 0.26.0; wire EngineBuilder::session_id from wire session (M10)`
- Push

## Report back
- HEAD sha
- Smoke test transcript snippet (last ~10 lines)
- Audit log session_id sample (confirm non-default)
```

---

## Phase H — Chain checkpoint ⚠️ CHECKPOINT — STOP AND REPORT

```
You are executing Phase H of M10: cross-repo chain verification + AWKWARDNESS.md closure report.

## Working directory
/Users/daiwanwei/Projects/wade/

## Pre-conditions
- Phases A-G pushed

## Scope (IN)
- For each of 8 repos (primitives, tool, loop, ai, subagent, harness, agemo, sandbox), run:
  - `git log -1 --oneline` (confirm latest is M10 commit)
  - `cargo build --locked`
  - `cargo test` (subagent + loop with `--all-features`)
- Build a status matrix table: repo × {commit sha, build, test, version}
- Cross-reference AWKWARDNESS.md items 1-5 → mark each as DONE (with commit) or DEFERRED with reason
- Cross-reference M9_GATE_DIAGRAM.md §5 items 1-6 → mark resolution status

## Acceptance
- 8x8 green matrix
- AWKWARDNESS closure report
- Gate diagram §5 closure report
- DO NOT make any commits in this phase — observational only
- STOP and report to user before Phase I

## Report back
- The full matrix
- Closure tables
- Any anomaly (test flake, version mismatch, etc) — flag for user decision
```

---

## Phase I — Sandbox patch (test-only fixup if any)

```
You are executing Phase I of M10: sandbox test-only adjustments if needed.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-sandbox/

## Pre-conditions
- Phase H reported green

## Scope (IN)
- If `cargo test` was failing in Phase H, identify and fix
- If green, this phase is a no-op — report and skip

## Acceptance
- `cargo build --locked` + `cargo test` green
- If commits made: `chore: sandbox test-fixture migration for M10 cascade`
- Push only if commits made

## Report back
- HEAD sha (changed or unchanged)
- Whether this was a no-op
```

---

## Phase J — Cross-cutting verification (final gate)

```
You are executing Phase J of M10: final cross-cutting verification + acceptance gate check.

## Working directory
/Users/daiwanwei/Projects/wade/motosan-agent-primitives/

## Pre-conditions
- Phases A-I done

## Scope (IN)
- Re-run the full chain matrix from Phase H
- Verify all 9 acceptance gates from M10_PLAN.md §5
- Append to IMPLEMENTATION_PLAN.md (primitives) a short "M10 shipped <date>; see individual CHANGELOGs" line per §7 Q5 resolution

## Acceptance
- All 9 gates pass
- IMPLEMENTATION_PLAN.md note added + committed: `docs: M10 shipped — cross-repo cascade complete`
- Push

## Report back
- Gate-by-gate pass/fail table
- Final HEAD sha for primitives
- Time spent total (Phase A start → Phase J end) — calendar days
```

---

## After M10

- M10 ships → primitives at 0.2.0 → downstream all migrated
- M11 dispatch: rental harness against 0.2.0 (separate plan, not this file)
- crates.io publish: M11 or later, NOT M10
