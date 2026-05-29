# M10 Plan — primitives 0.2.0 (refactor based on M9 consumer feedback)

**Date:** 2026-05-29
**Parent milestone:** [IMPLEMENTATION_PLAN.md §M10](IMPLEMENTATION_PLAN.md)
**Inputs:** [M9 AWKWARDNESS.md (5 items)](https://github.com/motosan-dev/motosan-agent-harness/blob/main/finance/AWKWARDNESS.md) + [M9_GATE_DIAGRAM.md §5 (6 items)](M9_GATE_DIAGRAM.md)
**Estimated effort:** 1.5-2 weeks (8-10 focused days)
**Status:** Awaiting approval

---

## 0. Purpose

M10 is the first deliberate API-breaking milestone for primitives since 0.1.0 was frozen at M6. The point is to address the friction surfaced by M9's FinanceHarness consumer — the framework's first real vertical use case. After M10 ships, primitives goes to **0.2.0** and the downstream chain re-validates against the new shape. M11 then runs the same exercise with a second harness (rental) to prove the API survives a second use case before locking at 1.0.0.

## 1. Input triage (11 candidate items)

| # | Source | Item | Disposition |
|---|---|---|---|
| 1 | AWK#1 | agemo stdin conflict | ✅ DONE in agemo 0.1.3 (issue #2) |
| 2 | AWK#2 | cargo build mutates read-only lockfile | 📋 PROCESS — `cargo build --locked` in chain-verify scripts |
| 3 | AWK#3 + Gate#1 | post_tool_use_failure has thin ctx; audit needs synthetic shape; audit split across Hook + host | 🔧 **FIX (partial)** — D-M10-2 closes the thin-ctx half (AWK#3); the audit-split half (ask_user / final_answer Hook lifecycle, Gate#1) is explicitly deferred per §2 OUT |
| 4 | AWK#4 | Tool name display vs internal namespace | 🔧 **FIX** — D-M10-4 |
| 5 | AWK#5 | Denied permission loses identity | ✅ DONE in agemo 0.1.3 (issue #1) |
| 6 | Gate#2 | Two event vocabularies (loop + primitives) | 📋 DOCS — README + ARCHITECTURE.md note; no code change |
| 7 | Gate#3 | ToolStarted fires before permission check | ✅ DONE in loop 0.25.1 (#196) |
| 8 | Gate#4 | PermissionContext lacks conversation state | 🔧 **FIX** — D-M10-3 |
| 9 | Gate#5 | Hook session_id is synthetic "default" | 🔧 **FIX** — D-M10-1 |
| 10 | Gate#6 | memory_schema stored but not enforced | ⏸ DEFER to M11+ — needs storage layer design |
| 11 | (meta) | Primitives version-pinning on path deps | ✅ PROGRESS — loop's Cargo.toml already pins primitives/tool versions, prepping for crates.io publish |

**Net M10 work: 4 fixes (D-M10-1 through D-M10-4) + 2 process items + 1 deferred.**

## 2. Scope

### IN
- **D-M10-1** Real session id flowing from Engine → HookCtx (closes Gate#5)
- **D-M10-2** `Hook::post_tool_use_failure` ctx carries full `ToolResult` instead of just `ToolFailure` (closes AWK#3 + Gate#1 partially)
- **D-M10-3** `PermissionContext` carries recent messages window (closes Gate#4)
- **D-M10-4** Tool display name distinct from internal name (closes AWK#4)
- Primitives bump 0.1.1 → **0.2.0** (first breaking release since 0.1.0)
- Downstream cascade: loop, subagent, harness, finance, agemo all migrate
- Finance harness re-validated end-to-end against new primitives

### OUT — deferred to M11 or later
- New Hook lifecycle methods for ask_user / final_answer (Gate#1 second half) — would be a bigger design (when does ask_user fire from Hook perspective?); covered by D-M10-2 making `post_tool_use_failure` audit-complete instead
- Memory schema runtime enforcement (Gate#6) — needs storage backend design, separate milestone
- Workflow + chat repo catch-up (still on old loop) — same pattern as M8/M8.6 deferrals
- Multi-stacked harness composition tests — happens naturally in M11 with rental harness

## 3. Shared design decisions

### D-M10-1. Real session id in HookCtx

**Problem (Gate#5):** `HookInterceptorAdapter::session_id()` (loop) is hardcoded to return `"default"` (`loop/src/core/hook_adapter.rs`). Hooks see this literal in every `HookCtx` variant they receive, so they can't correlate audit rows with the host's wire session_id.

**Important: the `session_id` field already EXISTS** on the primitives-side ctx structs (`PostToolUseFailureCtx.session_id: String`, `PermissionContext.session_id: &'a str`, etc — verified in primitives 0.1.1). The bug is purely that loop never populates it with a real value.

**Fix — loop-internal, no primitives change:**
- Engine grows a `session_id: String` field, set via `EngineBuilder::session_id(impl Into<String>)` (new setter) or auto-generated UUID v4 if unset
- `HookInterceptorAdapter::new(name, hook, session_id)` accepts the engine's session_id at construction (was: `new(name, hook)`)
- The hardcoded `session_id()` helper in `hook_adapter.rs` gets replaced; every ctx-struct construction site there populates `session_id` from the stored value

**Migration impact:**
- 0 primitives struct changes (field already there)
- 0 Hook trait impl breaks (impls READ the field, didn't construct it)
- Loop-internal only: HookInterceptorAdapter constructor signature changes + engine threads value
- agemo: new optional wire-up — `EngineBuilder::session_id(wire_session_id)` to feed agemo's wire session_id into the hook layer for cross-correlation

**This decision ships in loop 0.26.0 but does NOT drive the primitives 0.2.0 bump on its own.** D-M10-2 + D-M10-3 are the actual primitives breaks.

### D-M10-2. `post_tool_use_failure` carries full ToolResult

**Problem (AWK#3 + Gate#1):** `post_tool_use_failure` receives `ToolFailure` enum (just the error metadata), but audit consumers want the final `ToolResult` (the wire shape the model actually sees).

**Fix:**
```rust
// Before (current 0.1.x)
async fn post_tool_use_failure(&self, ctx: &PostToolUseFailureCtx) -> HookResult;

pub struct PostToolUseFailureCtx<'a> {
    pub session_id: &'a str,         // from D-M10-1
    pub tool_call: &'a ToolCall,
    pub failure: ToolFailure,         // just the failure enum
    pub cancellation_token: CancellationToken,
}

// After (0.2.0)
pub struct PostToolUseFailureCtx<'a> {
    pub session_id: &'a str,
    pub tool_call: &'a ToolCall,
    pub failure: ToolFailure,
    pub result: ToolResult,           // NEW — same shape the model sees
    pub cancellation_token: CancellationToken,
}
```

**Migration:** AuditLogHook implementations can now read `ctx.result` directly instead of synthesizing one from `ctx.failure`. Finance's AuditLogHook becomes ~5 LOC simpler.

### D-M10-3. PermissionContext carries recent messages

**Problem (Gate#4):** `PermissionPolicy::check` gets `tool_input` and `ToolAnnotations` but no conversation state. FinanceApprovalPolicy can't render "Approve buy 10 AAPL @ $185?" without the tools having to redundantly include current price in args.

**Fix:**
```rust
pub struct PermissionContext<'a> {
    pub tool_use_id: &'a str,
    pub tool_name: &'a str,
    pub tool_input: &'a Value,
    pub annotations: &'a ToolAnnotations,
    pub mode: PermissionMode,
    pub recent_messages: &'a [Message],    // NEW — last N messages for context
}
```

**Window size:** Last 10 messages (configurable via `EngineBuilder::permission_context_window(usize)`, default 10). Small enough to not bloat the policy's reasoning surface; large enough to recover "what was the agent just doing."

**Migration:** Existing PermissionPolicy impls compile-break on the struct change (missing field) — add `_recent_messages: &[Message]` to every impl. ~5 sites across motosan-agent-loop tests + ask_once harness + FinanceApprovalPolicy.

### D-M10-4. Tool display name distinct from internal name

**Problem (AWK#4):** `Harness` trait docs recommend namespaced names like `finance.place_order`, but Anthropic / OpenAI tool calling APIs are stricter on naming (no dots in some providers), and LLM prompt clarity favors short unqualified names. Finance harness ended up using `place_order` directly, conflicting with the docs.

**Fix:**
- `ToolDef` gains an `internal_name: String` field (was just `name`).
- `name` (public to LLM) stays unqualified: `place_order`.
- `internal_name` is namespaced for collision detection across stacked harnesses: `finance.place_order`.
- Harness composition uses `internal_name` for uniqueness check; loop uses `name` for LLM tool definitions and dispatch.

**Migration:** Default `internal_name = name` if not set — existing tools work unchanged. Finance tools add `internal_name: "finance.{tool}"`. ~20 lines across tool impls.

**Tradeoff considered:** Could also do it via `ToolAnnotations` or a separate `Tool::namespace()` method. Chose `ToolDef.internal_name` because it's part of the wire-level type already and matches MCP / Anthropic's `tool_metadata` pattern.

### D-M10-5. Versioning + downstream cascade

| Crate | Old | New | Reason |
|---|---|---|---|
| motosan-agent-primitives | 0.1.1 | **0.2.0** | breaking: HookCtx variants gain session_id; PostToolUseFailureCtx gains result; PermissionContext gains recent_messages |
| motosan-agent-tool | 0.4.0 | **0.5.0** | breaking: ToolDef gains internal_name |
| motosan-agent-loop | 0.25.1 | **0.26.0** | downstream of primitives + tool breaks; adds EngineBuilder::session_id + permission_context_window |
| motosan-ai (SDK) | 0.16.0 | **0.17.0** | downstream of tool break (ToolDef shape change) |
| motosan-agent-subagent | 0.4.0 | **0.5.0** | downstream of primitives + loop (HookCtx changes) |
| motosan-agent-harness | 0.1.2 | **0.2.0** | downstream of primitives (Hook trait surface) |
| motosan-agent-harness-finance | 0.1.0 | **0.2.0** | rewires against new primitives + harness; simplifies AuditLogHook |
| agemo | 0.1.3 | **0.2.0** | downstream of loop; demonstrates EngineBuilder::session_id wiring |
| motosan-sandbox | (patch) | patch | test-only fixup like M8.6 |

### D-M10-6. Primitives stays unfrozen until M11 acceptance

Unlike M8.6/M8.6.1 (which had a D-861-8 "don't touch primitives" invariant), M10 explicitly thaws primitives. After M10 + M11 pass, **0.2.0 → 1.0.0 refreeze**. The freezing decision happens in M11, not here.

### D-M10-7. `cargo build --locked` precondition

Process fix for AWK#2. Future chain-verify scripts and dispatch pre-conditions use `cargo build --locked` for read-only repos to catch accidental lockfile mutation. Not a code change.

### D-M10-8. Workflow + chat still deferred

Same pattern as M8/M8.6/M8.6.1. Workflow pins loop 0.8, chat-tool pins tool 0.2 — they're 4 major bumps behind even before M10's breaking changes. Not in scope.

## 4. Implementation phases

```
Phase A — Primitives 0.2.0 (PostToolUseFailureCtx + PermissionContext field adds) [1.5 days]
   └── scope: ONLY D-M10-2 + D-M10-3 (additive field changes). NOT D-M10-1 — that
       lives in loop. NOT D-M10-4 — that lives in tool.
   └── gate: cargo build + test green; doc-tests still pass (account for the macOS libiconv quirk)

Phase B — Tool 0.5.0 (ToolDef.internal_name)                                       [0.5 day]
   └── gate: cargo build + test green; backward-compat default keeps existing tools working

Phase C — Loop 0.26.0 (consume primitives 0.2.0 + tool 0.5.0; implement D-M10-1
         session_id flow; add EngineBuilder::session_id +
         permission_context_window setters)                                         [2 days]
   └── gate: cargo build + test --all-features green; all in-tree extensions compile;
       new test asserts session_id flows from Engine through HookInterceptorAdapter
       to a Hook impl (no more "default" literal)

Phase D — AI 0.17.0 (downstream of tool 0.5.0)                                      [0.5 day]
   └── gate: cargo build + test green

Phase E — Subagent 0.5.0 (Cargo.toml dep bumps + any test fixture migration)        [0.5 day]
   └── gate: cargo build + test --all-features green
   └── note: subagent's Hook/PermissionPolicy impls don't break (additive field
       changes); only test fixtures that construct ctx structs need migration

Phase F — Harness 0.2.0 + finance 0.2.0 (Cargo.toml dep bumps + simplify
         AuditLogHook to use ctx.result + finance ToolDef.internal_name)            [1 day]
   └── gate: cargo build + test green at workspace; finance/AWKWARDNESS.md
       items 3 + 4 marked resolved

Phase G — agemo 0.2.0 (consume loop 0.26.0; wire EngineBuilder::session_id from
         agemo's wire session_id)                                                   [0.5 day]
   └── gate: cargo build + test green; agemo --harness finance demo still works;
       audit log now carries the real wire session_id

Phase H — Chain checkpoint (commit + push all 8 repos)                              [0.5 day]  ⚠️ CHECKPOINT
   └── report: full chain test matrix; AWKWARDNESS.md items 1-5 closure status

Phase I — Sandbox patch (test-only fixup if any)                                    [0.25 day]

Phase J — Cross-cutting verification                                                [0.5 day]
   └── all 8 repos: cargo build --locked + cargo test green
```

**Total: ~7.75 days focused work + 1-2 days unknown-unknowns = 1.5-2 weeks calendar.** Down from initial 10-day estimate after the D-M10-1 reframing — most of the downstream "migration" is Cargo.toml dep version bumps + test-fixture updates, not Hook/Policy impl rewrites (those don't break because the primitives changes are additive at the trait level).

### Phase order rationale

Strict bottom-up dependency order. Primitives must publish first; tool can ship in parallel with primitives (different breaking surface); loop comes after both; ai needs only tool; subagent needs primitives + loop; harness needs primitives; finance + agemo last because they consume everything.

Phase H is the mandatory checkpoint after all 7 code-bearing repos push. Phase I is sandbox (test-only fixup if needed; might be empty). Phase J is the full chain-verify safety net.

## 5. Acceptance gates (final)

1. All 8 repos: `cargo build --locked` + `cargo test` green (subagent + loop with `--all-features`)
2. **Primitives 0.2.0** committed and pushed; all 4 D-M10 fixes verified by their respective tests
3. **Finance 0.2.0** simplifies `AuditLogHook` to use `ctx.result` instead of synthesizing from `ctx.failure` — `git diff` shows the simplification
4. **agemo `--harness finance` demo still works end-to-end** post-migration; transcript matches Phase 2c shape modulo the wire format additions (session_id should now appear in audit log)
5. **AWKWARDNESS.md items 3, 4 marked resolved** with reference to the M10 commits that fixed them; items 1, 2, 5 already closed
6. **M9_GATE_DIAGRAM.md §5 items 1, 4, 5 marked resolved**; items 2, 3 already resolved; item 6 explicitly deferred to M11+ with rationale
7. **New regression tests** in primitives + loop covering the 4 fixes
8. No workflow / chat touched
9. Chain-verify loop adopts `cargo build --locked` (D-M10-7)

## 6. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| `HookCtx` lifetime added (`&'a str` session_id) requires API rework if Hook impls hold ctx fields after the call | Medium | Pattern-check existing Hook impls before Phase A; if any retain ctx fields, design ctx as `Arc<str>` or `String` instead of `&'a str` |
| `PermissionContext.recent_messages` window growth breaks existing policies (e.g. AskOnce in agemo) | Low | Default empty slice if EngineBuilder doesn't set window; existing policies that ignore the field unaffected |
| `ToolDef.internal_name` migration breaks the architectural invariant test in loop (line ranges shift) | Medium | Phase C re-verifies the invariant test passes; if line ranges need widening, do it inline with the change |
| Cross-repo chain refactor (8 repos, 7 code-bearing) takes longer than M8 did because primitives is breaking | High | M8 chain was 25h estimate; M10 is similar scope. Honest estimate is 1.5-2 weeks calendar. If a repo cascades surprisingly, document and defer the downstream changes |
| macOS libiconv linker env still flaky for harness doc-tests | Out of scope | Already marked `ignore` in harness 0.1.2 post-Phase 1 |
| FinanceHarness behavior subtly changes due to PermissionContext widening (policy sees more context, might decide differently) | Low | Phase F gate runs the M9 demo scenarios; if behavior shifts, document as known M11 input |
| 2 process items (AWK#2 cargo --locked, Gate#2 event vocabulary docs) get forgotten | Medium | Phase J explicitly includes the `--locked` adoption; Phase A includes a short ARCHITECTURE.md note on event vocabulary |

## 7. Open questions to resolve before execution

1. **D-M10-1 ctx lifetime**: `&'a str` vs owned `String` for session_id on HookCtx variants. `&'a str` is more idiomatic but constrains some Hook impl patterns. **Recommendation: `&'a str`** — Hook impls that need to retain the id can `.to_string()` themselves; the trait shouldn't force allocation by default.

2. **D-M10-3 default window size**: 10 messages chosen heuristically. Could be 20 or configurable per-policy. **Recommendation: 10 with `permission_context_window(usize)` setter** — sensible default, advanced users can tune.

3. **D-M10-4 internal_name format**: dotted (`finance.place_order`), slashed (`finance/place_order`), or free-form? Anthropic API accepts dots in some places but not others. **Recommendation: free-form String** — let consumers pick; loop's collision check just uses byte-equality.

4. **Phase ordering: should AI 0.17.0 go before or after Loop 0.26.0?** AI doesn't depend on loop, but loop with `motosan-ai` feature does depend on ai. **Recommendation: Loop AFTER AI** — avoids loop having to rebuild with old ai version mid-stream. Updated Phase order in §4.

5. **Single CHANGELOG style vs per-crate**: with 7 crates bumping in the same window, do we want a top-level CHANGELOG.md tracking the M10 release across all crates? **Recommendation: per-crate** (existing pattern) + a brief M10 summary at the bottom of IMPLEMENTATION_PLAN.md noting "M10 shipped X-Y-Z dates, see individual CHANGELOGs."

## 8. What this plan does NOT cover

- Memory schema runtime enforcement (Gate#6) — deferred to M11+
- New Hook lifecycle methods for ask_user / final_answer (Gate#1 second half) — out of scope; D-M10-2 makes audit completable through existing methods
- Workflow + chat catch-up — still deferred
- M11 itself (rental harness) — separate plan
- crates.io publication — happens in M11

## 9. Effort estimate summary

| Phase | Days |
|---|---|
| A. Primitives 0.2.0 (D-M10-2 + D-M10-3 only) | 1.5 |
| B. Tool 0.5.0 (D-M10-4) | 0.5 |
| C. Loop 0.26.0 (D-M10-1 + primitives + tool consume) | 2 |
| D. AI 0.17.0 | 0.5 |
| E. Subagent 0.5.0 (mostly Cargo.toml + test fixtures) | 0.5 |
| F. Harness + finance 0.2.0 | 1 |
| G. agemo 0.2.0 | 0.5 |
| H. Chain checkpoint | 0.5 |
| I. Sandbox patch | 0.25 |
| J. Cross-cutting verify | 0.5 |
| **Total** | **7.75** |

Plus 1-2 days unknown-unknowns. Calendar: **1.5-2 weeks** focused. Re-estimate vs initial 9.75-day version reflects the D-M10-1 reframing (smaller scope, no struct migration) + recognition that subagent/harness/agemo migrations are mostly Cargo.toml bumps rather than impl rewrites.

## 10. After M10: M11 preview

M11 takes the M10-stabilized primitives 0.2.0 and validates with a **second vertical harness (rental)**. If rental needs no new primitives breaks → freeze API → 1.0.0 → publish to crates.io. If rental surfaces new breaks → primitives 0.3.0 → re-validate finance against 0.3.0 → then publish.

The decision to refreeze primitives (and at what version) happens in M11, not here. M10 deliberately leaves the door open.
