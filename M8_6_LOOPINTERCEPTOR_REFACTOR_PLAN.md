# M8.6 — LoopInterceptor refactor + close #193/#194

**Date:** 2026-05-27
**Repos touched:** `motosan-agent-loop` (0.23.0 → 0.24.0), `motosan-agent-subagent` (0.2.0 → 0.3.0), `agemo` (0.1.0 → 0.1.1), `motosan-sandbox` (test-only patch — see note below)
**Parent issues:** [#193](https://github.com/motosan-dev/motosan-agent-loop/issues/193), [#194](https://github.com/motosan-dev/motosan-agent-loop/issues/194)
**Position in roadmap:** new milestone between [M8.5](M8_5_AGEMO_PLAN.md) (done) and M9 (blocked on this)
**Estimated effort:** 2-3 weeks (12-13 focused days)
**Status:** Awaiting approval

---

## 0. Origin story (why this plan replaces M9_PREP_PLAN.md)

The original M9 prep work (filed as #193 + #194) was scoped as "add 3 builder setters" — half a day. Investigation revealed two deeper truths:

1. **The loop's `Extension` trait was conceptually conflated.** It bundled three unrelated concerns: lifecycle observation (overlaps with primitives' `Hook`), pipeline middleware (loop-internal mutation + deferral), and tool registration (`tool_defs`). The right fix isn't "merge with Hook" — it's "separate the conflated concerns and rename what's left."

2. **The engine has zero `PermissionPolicy` consultation.** Adding setters alone wouldn't make `Harness::permission_policy()` actually fire. The Engine needs a real consultation path injected into tool dispatch.

After ruling out γ-2a (Hook absorbs all of Extension — pollutes primitives) and γ-2b (split Extension into 5 traits — over-corrects), the chosen direction is **α-renamed**:

- Rename `Extension` → `LoopInterceptor` (signals "loop-internal pipeline middleware, not contract")
- Extract `tool_defs` to a separate `ToolProvider` trait (its own concern)
- Build `HookInterceptorAdapter` so primitives' `Hook` can register as a `LoopInterceptor`
- Add `EngineBuilder` setters for `hooks`/`permission_policy`/`memory_schema` (closes #193)
- Wire `PermissionPolicy::AskUser` through existing `AskUserAnswer` deferral (closes #194)

Primitives stays untouched at 0.1.1.

---

## 1. Purpose

Make `Harness::hooks()` and `Harness::permission_policy()` actually take effect when the engine runs an agent. Without this, M9's `FinanceHarness` with `FinanceApprovalPolicy` and `AuditLogHook` would silently bypass approval gates — undermining the whole demo.

Secondary purpose: fix the long-running `Extension` naming confusion so future contributors don't keep asking "what's the difference between Hook and Extension."

## 2. Scope

### IN
- Trait surface refactor in `motosan-agent-loop`: `Extension` → `LoopInterceptor`, extract `ToolProvider`
- `HookInterceptorAdapter`: bridge primitives `Hook` into the engine
- `EngineBuilder` setters: `hooks(...)`, `permission_policy(...)`, `memory_schema(...)`
- `PermissionPolicy` consultation injected at the top of `execute_tools_with_policy`
- `Permission::AskUser` routed through existing `AskUserAnswer` deferral
- `motosan-agent-subagent`: migrate 3 impl sites + struct renames (`SubagentExtension` → `SubagentInterceptor`, `DelegationExtension` → `DelegationInterceptor`, `DenyAllSpawnsExtension` → `DenyAllSpawnsInterceptor`)
- `agemo`: wire all 5 `Harness` fields + add `AskOnce` harness + stdin approval bridge
- Loop 0.24.0, subagent 0.3.0, agemo 0.1.1 committed and pushed

### OUT — deferred
- `motosan-agent-workflow` migration (has 7 Extension impls — punted per parent plan)
- `motosan-chat-tool` migration (heavy — punted)
- Deprecation / removal of `LoopInterceptor` itself (this plan keeps it as the canonical loop-internal trait; it is NOT a transition state)
- `MemorySchema` runtime enforcement (builder setter only; storage stays out per primitives M5)
- Multi-stacked harness composition (Harness contract documents it; this plan consumes one)
- Anything M9-specific (FinanceHarness, AuditLogHook, sequence-diagram gate)

### Out — never belongs here
- Primitives changes — explicitly forbidden by D-M86-10
- Any merge of `Hook` and `LoopInterceptor` — they are intentionally separate concerns (see [the rationale doc embedded in M8_5_AGEMO_PLAN.md §0](M8_5_AGEMO_PLAN.md) and the Hook/Extension distinction lesson learned)

## 3. Shared design decisions (the spec)

### D-M86-1. Adapter pattern, not absorption

Build `HookInterceptorAdapter` that wraps `Arc<dyn Hook>` and implements `LoopInterceptor`. `Hook` impls' lifecycle methods get forwarded to corresponding `LoopInterceptor` callsites; loop-internal methods (transform_request, rewrite_tool_result, on_op, deferral) default to no-op since `Hook` has no concept of them.

### D-M86-2. Rename `Extension` → `LoopInterceptor`

The name `Extension` is too generic and reads as "the loop's version of Hook" — which is the exact confusion that started this refactor. `LoopInterceptor` makes it explicit: this is a pipeline middleware, owned by the loop, not a contract.

Mechanical scope (verified 2026-05-27):
- `motosan-agent-loop/src/core/extension.rs` → rename file to `interceptor.rs`, rename trait + supporting types
- **328** `Extension`-bearing reference lines in loop → rename
- **20** in subagent → rename
- **37** `impl Extension for X` sites total (loop + subagent), of which ~6 are test fixtures — all migrate to `impl LoopInterceptor for X`
- `ExtError` keeps its name (it's a generic error type, not trait-specific)
- `HookCtx`, `ToolCallItem`, `ToolDecision`, `OpDecision`, `FlowDecision` keep their names (orthogonal to the trait)

### D-M86-3. Extract `ToolProvider` trait

`tool_defs(&self) -> Vec<ToolDef>` is neither lifecycle observation nor pipeline middleware — it's a separate "I provide tools" concern. Pull it out:

```rust
// motosan-agent-loop/src/core/tool_provider.rs (NEW)
pub trait ToolProvider: Send + Sync {
    fn tool_defs(&self) -> Vec<motosan_agent_tool::ToolDef>;
}
```

`LoopInterceptor` loses `tool_defs`. Engine accepts both:

```rust
EngineBuilder::interceptor(Arc<dyn LoopInterceptor>) -> Self
EngineBuilder::tool_provider(Arc<dyn ToolProvider>) -> Self
```

A type that implements both (like `SubagentInterceptor`) registers via both calls. No magic.

### D-M86-4. Subagent struct renames

Wade decision 2026-05-27. To keep names consistent with the new trait naming:

| Old | New |
|---|---|
| `SubagentExtension` | `SubagentInterceptor` |
| `DelegationExtension` | `DelegationInterceptor` |
| `DenyAllSpawnsExtension` (test) | `DenyAllSpawnsInterceptor` (test) |

This is breaking-public-API in subagent (the structs are `pub use`'d from `lib.rs`), hence 0.2.0 → 0.3.0 minor bump. agemo will need the new names; out-of-tree consumers (none known) will too.

### D-M86-5. HookResult → ToolDecision mapping (new design question — see §8 Q1)

The adapter must translate `HookResult` to `ToolDecision` for `intercept_tool_call`:

| HookResult | ToolDecision |
|---|---|
| `Continue { updated_input: None }` | `Proceed(item.clone())` |
| `Continue { updated_input: Some(v) }` | `Replace(item.with_input(v))` |
| `Skip { reason }` | `ShortCircuit(ToolOutput::error(&format!("skipped by hook: {reason}")))` |
| `Abort { reason }` | **No direct mapping — see Q1** |

`HookResult::Abort` means "stop the entire agent run", but `ToolDecision` is per-tool-call. Solution per Q1 resolution: extend `ToolDecision` with an `Abort { reason }` variant. The engine already handles aborts via `StopReason::AbortedByHook`; we surface it through ToolDecision so the propagation path is explicit, not spooky-at-a-distance via shared state.

### D-M86-6. #194 Q1 — Approval semantics: **Option B** (parse string, fail-closed)

Engine looks for substrings "allow", "yes", "approve" → approval; "deny", "no", "reject" → rejection; anything else (including empty / whitespace / unrecognized) → log warning, treat as rejection.

**Why fail-closed:** a typo never accidentally approves a `place_order`. Brittleness is bounded — the safety failure mode is "approval surprisingly denied", not "tool surprisingly approved."

Match uses **word-boundary** regex, not substring contains, so "I don't allow that" matches "allow" cleanly but "I'm not sure, let me think" does not (Risk #4 from M9_PREP_PLAN.md addressed).

### D-M86-7. #194 Q2 — Timeout: **Option B** (configurable, default infinite)

```rust
RunBuilder::permission_timeout(Duration)  // NEW
```

agemo exposes `--permission-timeout-secs` (default 0 = infinite).

### D-M86-8. #194 Q3 — Permission::AskUser schema: unchanged

No primitives change. Engine derives tool_use_id from the call being gated.

### D-M86-9. #194 Q4 — `Permission::AskUser { prompt: None }`: **Option A** (generic default)

```rust
format!("Approve call to tool '{}' with args {}?", tool_name, args_json)
```

If the M9 FinanceHarness UX feels too generic in practice, escalate to deprecating `Option<>` in a follow-up. Not this milestone.

### D-M86-10. Primitives stays at 0.1.1

Hard constraint. Primitives has been the frozen contract since M6. The whole point of α-renamed (over γ-2a) is to avoid touching primitives. If any phase below tries to add a method to `Hook` or change `Permission` shape, **STOP** and revisit this plan.

### D-M86-11. Workflow + chat deferred

`motosan-agent-workflow` (7 Extension impls, pins loop 0.8) and `motosan-chat-tool` (heavy pub-use wall, pins tool 0.2) are NOT touched in this milestone — consistent with how M8 deferred them. They will need their own catch-up cycle eventually.

### D-M86-12. Policy consultation point

`PermissionPolicy::check()` fires at the **top** of `execute_tools_with_policy` (`engine.rs:3089`), BEFORE any `LoopInterceptor::intercept_tool_call`. Order:

1. PermissionPolicy::check — Deny → emit ToolCallEnd is_error + skip; AskUser → defer; Allow → fall through
2. LoopInterceptor::intercept_tool_call (existing path) — can still defer/rewrite further but not bypass
3. Tool dispatch

This means policy is the outermost authority. Consistent with primitives D3 "most-restrictive wins" — interceptors cannot weaken the policy's decision.

### D-M86-13. Three new `LoopInterceptor` lifecycle methods (Hook coverage)

`Harness::hooks()` returns `Vec<Arc<dyn Hook>>` and Hook has 9 lifecycle methods, but `LoopInterceptor` today only has callsites for 6 of them. The remaining 3 — `session_start`, `pre_compact`, `subagent_stop` — would be silently dead if HookInterceptorAdapter wraps a Hook that implements them, because nothing in the engine would invoke the wrapped method.

To make all 9 Hook methods actually fire (and to satisfy acceptance gate 5), add 3 new methods to `LoopInterceptor` with default no-op impls (so the 37 existing impls don't break), and wire engine callsites:

| New LoopInterceptor method | Engine callsite |
|---|---|
| `async fn on_session_start(&mut self, ctx: &mut HookCtx<'_>)` | top of `Engine::run` setup, before first iteration |
| `async fn before_compact(&mut self, ctx: &mut HookCtx<'_>, msgs: &[Message])` | inside autocompact extension's trigger point (which already exists at `src/extensions/autocompact/extension.rs`) |
| `async fn on_subagent_stop(&mut self, ctx: &mut HookCtx<'_>, result: &SubagentResult)` | invoked by motosan-agent-subagent's termination path (subagent crate calls into the loop's interceptor dispatcher) |

The adapter then forwards:
- `Hook::session_start` → `LoopInterceptor::on_session_start`
- `Hook::pre_compact` → `LoopInterceptor::before_compact`
- `Hook::subagent_stop` → `LoopInterceptor::on_subagent_stop`

All 3 are additive — zero impact on existing 37 LoopInterceptor impls (they get default no-op behavior). The `on_subagent_stop` callsite requires a one-line change in subagent's termination path during Phase F to invoke the dispatcher.

### D-M86-14. Versioning

| Crate | Old | New | Reason |
|---|---|---|---|
| motosan-agent-loop | 0.23.0 | **0.24.0** | trait rename + additions (minor: existing impls migrate via rename, no semantic break) |
| motosan-agent-subagent | 0.2.0 | **0.3.0** | struct renames + trait migration (breaking public API) |
| agemo | 0.1.0 | **0.1.1** | additive (5-field wiring + AskOnce harness) |
| motosan-sandbox | (patch) | **patch** | test-only: `loop_integration.rs` Extension → LoopInterceptor (see scope note) |
| primitives, tool, ai, harness | unchanged | — | not touched |

**Scope note — 4th repo (sandbox).** The original plan listed 3 edit targets based on the M8 audit, which classified `motosan-sandbox` as "Trivial: 1 Tool impl" and **missed** the 2 `impl Extension` blocks in `crates/motosan-sandbox/tests/loop_integration.rs` (`SandboxApprovalExtension`, `DeferGateExtension`). The D-M86-2 trait rename ripples to any `impl Extension`, so sandbox's test file needed a mechanical 3-line migration to compile against loop 0.24.0. This is test-only — sandbox's production code and public API are unchanged. Applied as [motosan-sandbox commit `c667463`](https://github.com/daiwanwei/motosan-sandbox/commit/c667463). Not real scope expansion; correcting an audit undercount.

## 4. Cross-repo file layout

### `motosan-agent-loop` changes

```
src/core/
├── interceptor.rs            # NEW name (was extension.rs): LoopInterceptor trait
├── tool_provider.rs          # NEW: ToolProvider trait
├── hook_adapter.rs           # NEW: HookInterceptorAdapter (Hook → LoopInterceptor)
├── permission_runtime.rs     # NEW: PermissionPolicy consultation + AskUser routing
├── interceptor_set.rs        # RENAMED from extension_set.rs; dispatcher logic
├── decision.rs               # +Abort variant on ToolDecision (per D-M86-5 + Q1)
├── engine.rs                 # +3 builder setters, +interceptor/tool_provider setters,
│                             #   wire permission_runtime + adapters
├── extension.rs              # DELETED (re-exports kept via lib.rs for one deprecation cycle? — see Q3)
└── mod.rs                    # rename re-exports

src/lib.rs                    # rename pub use; possibly keep deprecated Extension alias (Q3)

tests/
├── permission_gating.rs      # NEW: Deny/Allow/AskUser-approve/AskUser-deny all behave
├── hook_lifecycle.rs         # NEW: 9 Hook methods fire at expected callsites
└── hook_interceptor_parity.rs  # NEW: adapter doesn't drop/duplicate events
```

### `motosan-agent-subagent` changes

```
Cargo.toml                   # version 0.2.0 → 0.3.0, loop dep → 0.24.0
src/lib.rs                   # pub use renames
src/delegation/
├── extension.rs             # RENAMED to interceptor.rs; trait + struct rename
└── mod.rs                   # re-export rename
src/subagent/
├── extension.rs             # RENAMED to interceptor.rs; trait + struct rename;
│                            #   split tool_defs into separate impl ToolProvider
└── mod.rs                   # re-export rename
tests/
└── opt_out_layers.rs        # struct + trait rename
CHANGELOG.md                 # 0.3.0 entry (BREAKING)
```

### `agemo` changes

```
Cargo.toml                   # version 0.1.0 → 0.1.1, loop → 0.24.0, subagent (not a dep, n/a)
src/
├── cli.rs                   # +--permission-timeout-secs flag
├── harness_registry.rs      # +AskOnce variant, +AskOnceHarness inline impl
├── main.rs                  # wire harness.hooks() + permission_policy() + memory_schema();
│                            #   add stdin → ops_rx approval bridge for AskUser event
└── provider.rs              # unchanged
tests/
├── ask_once.rs              # NEW: subprocess + scripted stdin → assert approval flow
└── smoke.rs / sigint.rs     # unchanged
CHANGELOG.md                 # 0.1.1 entry
README.md                    # +AskOnce demo paragraph
```

## 5. Implementation phases (gated, sequential)

```
Phase A — Loop trait refactor (rename + ToolProvider split + 3 new methods)  [3 days]
   └── gate: cargo build + cargo test green, 101 sites renamed, no double-defs

Phase B — Hook adapter                                                    [1.5 days]
   └── gate: hook_interceptor_parity.rs test green

Phase C — EngineBuilder setters + policy consultation                     [2 days]
   └── gate: permission_gating.rs test green for Deny + Allow

Phase D — AskUser routing                                                 [1.5 days]
   └── gate: permission_gating.rs test green for AskUser-approve + AskUser-deny

Phase E — Loop checkpoint (commit + push 0.24.0)                          [0.5 day]  ⚠️ CHECKPOINT

Phase F — Subagent migration                                              [1 day]
   └── gate: subagent cargo test green, agemo not yet updated

Phase G — Subagent checkpoint (commit + push 0.3.0)                       [0.5 day]  ⚠️ CHECKPOINT

Phase H — agemo wiring + AskOnce harness + stdin approval bridge          [1.5 days]
   └── gate: ask_once integration test green

Phase I — agemo commit + push 0.1.1                                       [0.5 day]

Phase J — Cross-cutting verification (all 7 repos still green)            [0.5 day]
```

Total: **11 days focused work + 1-2 days unknown-unknowns = 12-13 days. Calendar: 2-3 weeks.**

### Phase A — Loop trait refactor (3 days)

1. Rename `src/core/extension.rs` → `src/core/interceptor.rs`. Rename `Extension` → `LoopInterceptor` inside.
2. Remove `tool_defs` method from `LoopInterceptor`.
3. Create `src/core/tool_provider.rs` with the `ToolProvider` trait.
4. Rename `src/core/extension_set.rs` → `src/core/interceptor_set.rs`. Internal `extensions:` field → `interceptors:`. Dispatch methods follow the rename.
5. Update `src/core/mod.rs` re-exports.
6. Update `src/lib.rs` re-exports.
7. Mechanical sweep with `fastmod` — note actual scope is 328 reference lines in loop, not the ~100 originally estimated:
   ```bash
   fastmod 'Extension\b' 'LoopInterceptor' --extensions rs --include 'src/**'
   fastmod 'extension_set' 'interceptor_set' --extensions rs --include 'src/**'
   fastmod 'pending_extensions' 'pending_interceptors' --extensions rs --include 'src/**'
   ```
8. Run `cargo build` — fix remaining renames manually (likely `ExtError` mentions in error variants, doc comments, etc.).
9. The 37 in-tree `impl Extension for X` sites become `impl LoopInterceptor for X` automatically via fastmod. Each one's `tool_defs` (if any) needs to be MOVED to a separate `impl ToolProvider for X` block — manual per site. **Enumerated in-tree `tool_defs` sites that need splitting (verified by grep):**
   - `src/extensions/ask_user/extension.rs:279` — `AskUserExtension::tool_defs`
   - `src/extensions/planning/extension.rs:65` — `PlanningExtension::tool_defs`
   - (Subagent's `SubagentExtension::tool_defs` is handled in Phase F, not here. `engine.rs:146` has `fn tool_defs(&self) -> &[ToolDef]` but it is an engine-internal method with a different signature — unaffected.)
10. **Add 3 new `LoopInterceptor` lifecycle methods** per D-M86-13:
    - `on_session_start(&mut self, ctx)` with default no-op
    - `before_compact(&mut self, ctx, msgs)` with default no-op
    - `on_subagent_stop(&mut self, ctx, result)` with default no-op
11. **Wire engine callsites** for the 3 new methods:
    - `on_session_start` — invoke at top of `Engine::run` before first iteration
    - `before_compact` — invoke from `extensions/autocompact/extension.rs` at the compaction trigger point
    - `on_subagent_stop` — interceptor dispatcher exposes a `dispatch_subagent_stop(result)` entry point; subagent's termination path will call it in Phase F (so a stub callsite in loop is enough here; subagent wires it up later)

**Gate:** `cargo build && cargo test --all-features` green. `grep -rn 'Extension' src/` returns nothing (allow CHANGELOG mentions). All 9 Hook lifecycle methods now have a corresponding `LoopInterceptor` method on the trait surface. (`--all-features` is mandatory — loop has 4 feature flags gating 7 integration test files; without the flag those test files compile to empty and silently "pass.")

### Phase B — Hook adapter (1.5 days)

1. Create `src/core/hook_adapter.rs` with `HookInterceptorAdapter`.
2. Add `ToolDecision::Abort { reason }` variant in `decision.rs` (per D-M86-5).
3. Implement `LoopInterceptor for HookInterceptorAdapter` with the full 9-method mapping per D-M86-5 + D-M86-13:
   - `pre_tool_use` → `intercept_tool_call` (per the HookResult→ToolDecision table in D-M86-5)
   - `post_tool_use` / `post_tool_use_failure` → `after_tool_result` (branch on `is_error`)
   - `user_prompt_submit` → `before_iteration`
   - `session_end` / `stop` → `on_terminal`
   - `session_start` → `on_session_start` (new in Phase A step 10)
   - `pre_compact` → `before_compact` (new)
   - `subagent_stop` → `on_subagent_stop` (new)
4. Engine must handle `ToolDecision::Abort` → propagate to `StopReason::AbortedByHook`.
5. `tests/hook_interceptor_parity.rs`: a hand-written interceptor and a Hook-wrapped-by-adapter fire equivalent events for **all 9 lifecycle points**. Use a counting fixture.

**Gate:** parity test green for all 9 methods, all existing tests still green.

### Phase C — EngineBuilder setters + policy consultation (2 days)

1. Add 3 builder setters to `EngineBuilder` in `engine.rs`:
   ```rust
   pub fn hooks(mut self, hooks: impl IntoIterator<Item = Arc<dyn Hook>>) -> Self
   pub fn permission_policy(mut self, policy: Arc<dyn PermissionPolicy>) -> Self
   pub fn memory_schema(mut self, schema: MemorySchema) -> Self
   ```
2. `build()` wraps each Hook in `HookInterceptorAdapter` and registers via `interceptor()` (so they share the dispatch path).
3. Create `src/core/permission_runtime.rs`:
   ```rust
   pub(crate) async fn consult_policy(
       policy: Option<&Arc<dyn PermissionPolicy>>,
       tool: &str,
       args: &Value,
       ctx: &PermissionContext,
   ) -> Permission { ... }
   ```
4. Inject the call at the top of `execute_tools_with_policy` (engine.rs:3089). On `Permission::Deny`, emit ToolCallEnd with is_error and skip. On `Permission::Allow`, fall through to existing path.
5. `tests/permission_gating.rs`: write Deny + Allow sub-tests (AskUser comes in Phase D).

**Gate:** Deny + Allow sub-tests green.

### Phase D — AskUser routing (1.5 days)

1. Extend `permission_runtime.rs` to handle `Permission::AskUser`: emit `AgentEvent::AskUser`, defer via `ToolDecision::Defer { call_id, reason }`, await `AgentOp::AskUserAnswer` on `ops_rx` (reuse existing `AskUserExtension` infrastructure at `extensions/ask_user/extension.rs:56+`).
2. Parse answer per D-M86-6 (word-boundary regex, fail-closed).
3. `Permission::AskUser { prompt: None }` → generic default per D-M86-9.
4. Add `RunBuilder::permission_timeout(Duration)` per D-M86-7.
5. `tests/permission_gating.rs`: add AskUser-approve and AskUser-deny sub-tests. Use `tokio::sync::mpsc` to send scripted `AskUserAnswer` ops.

**Gate:** all 4 sub-tests in `permission_gating.rs` green; `hook_lifecycle.rs` green; `cargo build && cargo test --all-features` green for `motosan-agent-loop` 0.24.0-rc.

### Phase E — Loop checkpoint (0.5 day) ⚠️ MANDATORY

- Final loop test pass + `grep -rn 'Extension' src/` clean
- CHANGELOG 0.24.0 entry
- Commit: `feat: motosan-agent-loop 0.24.0 — LoopInterceptor refactor + Hook/Policy wiring`
- Push to `motosan-dev/motosan-agent-loop`
- **Report back** with: commit SHA, test count delta, CI status, any deviations from plan
- **Wait for Wade approval** before Phase F starts
- This checkpoint protects subagent + agemo work from compounding any loop-side mistakes

### Phase F — Subagent migration (1 day)

1. `Cargo.toml`: bump version 0.3.0, dep loop → 0.24.0
2. `src/delegation/extension.rs` → rename file to `interceptor.rs`; `DelegationExtension` → `DelegationInterceptor`; `impl Extension` → `impl LoopInterceptor`
3. `src/subagent/extension.rs` → rename file to `interceptor.rs`; `SubagentExtension` → `SubagentInterceptor`; **split `tool_defs` into separate `impl ToolProvider for SubagentInterceptor`**
4. `src/lib.rs` re-export renames: `DelegationInterceptor`, `SubagentInterceptor`
5. `tests/opt_out_layers.rs` → `DenyAllSpawnsExtension` → `DenyAllSpawnsInterceptor`
6. **Wire `on_subagent_stop` invocation** per D-M86-13: at the subagent termination path (`src/subagent/driver.rs` or `manager.rs` — locate during execution), call into the loop's interceptor dispatcher `dispatch_subagent_stop(result)`. This is the callsite stub left in Phase A step 11.
7. CHANGELOG 0.3.0 entry (BREAKING)

**Gate:** `cargo build && cargo test --all-features` green in subagent. (The `--all-features` flag is mandatory — subagent's integration tests are all `#![cfg(feature = "testing")]` gated and would trivially "pass" without the flag. This is the gap that hid the broken `opt_out_layers.rs` test post-M8 Step 4; see [motosan-agent-subagent commit `e2e50de`](https://github.com/motosan-dev/motosan-agent-subagent/commit/e2e50de). Subagent only has one custom feature so `--all-features` ≡ `--features testing`, but the flag is consistent with the loop gate.)

### Phase G — Subagent checkpoint (0.5 day) ⚠️ MANDATORY

- Commit: `feat: motosan-agent-subagent 0.3.0 — LoopInterceptor migration + struct renames`
- Push
- Report back

### Phase H — agemo wiring + AskOnce harness (1.5 days)

1. `Cargo.toml`: bump 0.1.1, loop → 0.24.0
2. `src/cli.rs`: add `--permission-timeout-secs`
3. `src/harness_registry.rs`: add `HarnessKind::AskOnce`, define `AskOnceHarness` inline. Tools: same as EchoAdd. PermissionPolicy: returns AskUser for first call, Allow afterwards.
4. `src/main.rs`: wire `.hooks(harness.hooks())`, `.permission_policy(harness.permission_policy())`, `.memory_schema(harness.memory_schema())` on EngineBuilder. Add stdin reader spawn: on each `AgentEvent::AskUser`, read one line from stdin, send `AgentOp::AskUserAnswer { call_id, answer }` via ops_sender.
5. `tests/ask_once.rs`: spawn agemo with AskOnce harness + stub provider + scripted stdin (`echo "allow\n"`). Assert JSONL stream contains `ask_user` event, then `tool_call_start`/`tool_call_end`.

**Gate:** ask_once test green; existing 3 agemo tests still pass.

### Phase I — agemo commit + push (0.5 day)

- Commit: `feat: agemo 0.1.1 — wire 5 Harness fields + AskOnce demo`
- Push
- Verify CI status (will fail until org billing fixed — orthogonal)

### Phase J — Cross-cutting verification (0.5 day)

From clean checkouts:
```bash
for repo in motosan-agent-tool motosan-agent-loop motosan-ai/sdks/rust \
            motosan-agent-subagent motosan-sandbox motosan-agent-harness agemo; do
  cd /Users/daiwanwei/Projects/wade/$repo
  case "$repo" in
    # Loop has 4 feature flags (testing/cancellation/mcp-client/motosan-ai)
    # gating 7 integration test files; subagent has 1 (testing) gating 7.
    # Without --all-features, those files compile to empty and trivially
    # "pass" with 0 tests — silently masking regressions.
    motosan-agent-loop|motosan-agent-subagent)
      cargo build && cargo test --all-features
      ;;
    *)
      cargo build && cargo test
      ;;
  esac
  [ $? -eq 0 ] || { echo "FAIL: $repo"; exit 1; }
done
```

Note: `--all-features` on loop pulls in `motosan-ai` and `mcp-client` deps, adding ~1-2 min of compile time. Live tests inside (`live_anthropic.rs`, `mcp_http_live.rs`) self-skip when env vars / external services are absent — they're compile-checked but don't run.

All must pass. Primitives untouched (no commits).

## 6. Acceptance gates (final)

1. `cargo build && cargo test` green in: agemo 0.1.1. For loop 0.24.0 and subagent 0.3.0: `cargo build && cargo test --all-features` (the flag is mandatory — both crates have integration tests behind `#![cfg(feature = ...)]` gates; see Phase F gate note and the chain-verify rationale in §J for the post-mortem detail).
2. `cargo build` + `cargo test` still green (unchanged) in: primitives 0.1.1, tool 0.4.0, ai 0.16.0, sandbox, harness
3. `grep -rn 'Extension\b' motosan-agent-loop/src/ motosan-agent-subagent/src/` returns nothing (CHANGELOG-only matches OK)
4. `tests/permission_gating.rs` green: Deny + Allow + AskUser-approve + AskUser-deny all behave correctly
5. `tests/hook_lifecycle.rs` green: each of 9 Hook methods fires at the expected callsite
6. `tests/hook_interceptor_parity.rs` green: HookInterceptorAdapter and raw LoopInterceptor produce equivalent event sequences
7. `agemo --list-harnesses` shows null + echo-add + **ask-once**
8. `agemo --harness ask-once` with scripted stdin produces JSONL with `ask_user` event followed by `tool_call_start`/`tool_call_end`
9. `tests/ask_once.rs` green on macOS + Linux CI (when CI billing unblocked — orthogonal)
10. agemo src LOC ≤ 350 (relaxed from 300 to accommodate AskOnce + stdin bridge — see Risk #1)
11. **No primitives commit during this cycle** (D-M86-10 invariant)

## 7. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| **agemo LOC bursts past 350** with AskOnce + stdin bridge | High | Plan §6 already relaxes to 350. If 350 isn't enough, the bridge logic moves to a separate module (`src/approval_bridge.rs`); harder cap is 400 |
| **HookResult::Abort propagation** via new ToolDecision::Abort breaks an existing path | Medium | Add an explicit test in Phase B: a Hook returning Abort during pre_tool_use causes the loop to emit AgentStop with AbortedByHook reason, no further events |
| **Mapping table (D-M86-5) loses semantic info** when Hook context fields don't exist on LoopInterceptor side (or vice versa) | Medium | Phase B parity test should catch this; if mapping is ambiguous, document the lossy direction in adapter doc-comments |
| **D-M86-12 ordering** (policy before interceptor) breaks an existing interceptor that assumed it could see every tool call | Medium | Audit interceptors in Phase C before policy injection lands. Worst case: interceptors that need to see denied calls observe via a new lifecycle event (out of scope; document as M9 follow-up) |
| **Workflow / chat refactor catch-up debt grows** because workflow still pins loop 0.8 and uses Extension surface | Out of scope | Deferred per D-M86-11; track as M11 work |
| **Subagent struct rename surfaces an out-of-tree consumer** breaking | Low | None known. CHANGELOG 0.3.0 entry calls out BREAKING. Mitigation if surfaces post-ship: brief deprecation alias `pub use ...Interceptor as ...Extension` in subagent 0.3.1 |
| **Phase A's fastmod sweep misses a rename** (e.g. in doc-comment code blocks) | Low | Final acceptance gate 3's grep catches it |
| **CI billing still red** at Phase I push | Out of scope | Decided separately; this plan doesn't block on it |
| **Loop's 40 impl Extension sites have a `tool_defs` we miss when splitting** | Medium | Explicit grep in Phase A step 9: `grep -B1 -A3 "fn tool_defs" src/` enumerates every site that needs splitting |

## 8. Open questions resolved

### Q1 (NEW). `HookResult::Abort` → `ToolDecision::?` mapping

**Resolved during plan drafting:** add `ToolDecision::Abort { reason: String }` variant. Engine catches it in tool dispatch and propagates to `StopReason::AbortedByHook`. Cleaner than smuggling abort signals through shared state or via `ShortCircuit` with a magic-string error. Single-line additive change to `decision.rs`.

### Q2 (NEW). Deprecate `Extension` alias temporarily, or hard rename?

After the rename, should we keep `pub use LoopInterceptor as Extension` in `lib.rs` for one deprecation cycle to ease migration for any out-of-tree consumer?

- **Hard rename (recommended):** clean break, forces consumers to update on the 0.24.0 bump, no zombie alias to maintain. Justified because the only known consumer (subagent) is migrating in the same cycle.
- Soft alias: friendlier, but the whole point of the rename is conceptual clarity — a deprecated alias keeps both names alive and undermines the lesson.

**Pick hard rename.** If we later discover an out-of-tree consumer breaks, ship a 0.24.1 with a deprecated alias.

### Q3 (NEW). Should `ToolProvider` impls auto-register their tools, or require explicit `EngineBuilder::tool_provider(...)`?

- **Auto-register:** `impl LoopInterceptor + ToolProvider` for the same type → tools registered automatically when the interceptor is.
- **Explicit:** caller does `builder.interceptor(x.clone()).tool_provider(x)` — verbose but no magic.

**Pick explicit.** Auto-registration adds "spooky" behavior that hides what's happening; explicit calls are 1 extra line and make the registration intent visible. Documented in the new `ToolProvider` rustdoc.

### Q4–Q7 (from #194)

Pre-baked in D-M86-6 through D-M86-9. No further decisions needed.

## 9. What this plan does NOT cover

- M9 itself (FinanceHarness + AuditLogHook + sequence-diagram gate) — separate plan
- Workflow + chat migration to loop 0.24 / tool 0.4 — separate milestone
- `MemorySchema` runtime enforcement — separate concern
- CI billing fix on motosan-dev — orthogonal
- Republishing any crate to crates.io — out of scope per parent IMPLEMENTATION_PLAN.md
- `IMPLEMENTATION_PLAN.md` itself — needs a new M8.6 entry added between M8.5 and M9; recommend doing that as part of the commit that introduces this plan, but separate edit

## 10. Effort estimate

| Phase | Days | Notes |
|---|---|---|
| A. Loop trait refactor | **3** | rename 328 ref lines + ToolProvider split (incl. 2 in-tree sites: ask_user, planning) + 3 new lifecycle methods + engine callsites |
| B. Hook adapter | 1.5 | new file + parity test for **all 9** methods + Abort variant |
| C. EngineBuilder setters + policy | 2 | 3 setters + permission_runtime injection + 2 sub-tests |
| D. AskUser routing | 1.5 | reuse existing deferral + 2 sub-tests + timeout |
| E. Loop checkpoint | 0.5 | commit + push + Wade review |
| F. Subagent migration | 1 | 4 files + struct renames + tool_defs split + on_subagent_stop wiring |
| G. Subagent checkpoint | 0.5 | commit + push |
| H. agemo wiring + AskOnce | 1.5 | bridge + harness + test |
| I. agemo commit + push | 0.5 | CHANGELOG + push |
| J. Cross-cutting verify | 0.5 | 7-repo build chain |
| **Total** | **12.5** | + 1-2 unknown-unknowns |

**Calendar: 2-3 weeks** at ~half-time focus, ~13-14 working days at full focus.
