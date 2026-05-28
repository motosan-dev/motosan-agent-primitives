# M8.6.1 — Tool execution path unification + middleware completeness

**Date:** 2026-05-28
**Repos touched:** `motosan-agent-loop` (0.24.0 → 0.25.0), `motosan-agent-subagent` (0.3.0 → 0.3.1), `agemo` (no version change unless needed for new tests)
**Parent milestone:** [M8.6](M8_6_LOOPINTERCEPTOR_REFACTOR_PLAN.md) — this is the structural completion of what M8.6 only wired at one entry point
**Estimated effort:** 6-8 days (1-1.5 weeks)
**Status:** Awaiting approval

---

## 0. Origin

M8.6 wired `PermissionPolicy`, `Hook` adapter, and `AskUser` routing into one tool-dispatch entry point (`execute_tools_with_policy`). All planned tests passed. **But:** there are other tool-dispatch paths in the engine that bypass this entry entirely. Streaming is the production-critical one — agemo emits JSONL via the streaming path, so the M8.6 middleware effectively never fires in real use.

Wade's review surfaced 5 gaps post-M8.6 (2026-05-28):

| # | Gap | Filed elsewhere? |
|---|---|---|
| 1 | Permission policy not applied to all dispatch paths | Subsumed by this plan |
| 2 | AskUser blocks the entire tool batch (inline `rx.recv` serialization) | [motosan-agent-loop#195](https://github.com/motosan-dev/motosan-agent-loop/issues/195) |
| 3 | Hook chain stops after first rewrite — later hooks don't see updated_input | NEW; this plan |
| 4 | Streaming eager tool execution bypasses permission / interceptors / hooks | Subsumed by #1 (same root: unified dispatch) |
| 5 | `on_subagent_stop` only fires on explicit close, 6 of 7 termination paths miss it | [motosan-agent-subagent#1](https://github.com/motosan-dev/motosan-agent-subagent/issues/1) |

This plan addresses all 5. The 2 issues already filed (#195, subagent#1) are absorbed as work items; their issue bodies stay as the design refs.

## 1. Purpose

Make M8.6's wiring **structurally complete** rather than entry-point-complete. Every tool invocation in the engine must reach the same pipeline:

```
ENTRY → permission → interceptor chain (with proper rewrite propagation)
      → deferral resolution → tool.call() → result rewrite chain → wire conversion
```

After this milestone, M9's FinanceHarness can rely on PermissionPolicy + AuditLogHook actually firing in production (streaming) use.

## 2. Scope

### IN
- Audit + identify every `tool.call()` invocation reachable from production code paths
- Refactor `StreamingToolExecutor::submit` to go through the unified pipeline (or replace its role with a streaming-aware variant of the unified pipeline)
- Fix `LoopInterceptor::intercept_tool_call` chain dispatch: propagate `updated_input` through the chain instead of stopping at first non-Proceed
- Replace inline `rx.recv` permission AskUser with `ToolDecision::Defer` so batch tools aren't serialized (#195)
- Route all 6 unguarded subagent termination paths through `on_subagent_stop` (subagent#1)
- Add **architectural invariant tests** that grep-assert no `tool.call()` bypass exists post-fix (so this never regresses)
- Add **multi-hook chain test** that proves rewrite propagation
- Add **streaming permission test** that proves policy fires on the streaming path
- Add **parallel batch test** that proves one AskUser-deferred tool doesn't block other tools in the batch

### OUT — deferred
- Workflow + chat repos still on old loop versions (continued M8 deferral)
- Multi-stacked harness composition
- `MemorySchema` runtime enforcement

**Note on PlanningTool:** previously deferred; **now in scope** after self-review confirmed it IS a bypass (`planning.rs:478` calls `tool.call()` on sub-tools synchronously via `block_on`, skipping the unified dispatch). M9's FinanceHarness can plausibly use PlanningTool to orchestrate `get_quote → check_price → place_order`, and FinanceApprovalPolicy gating `place_order` must fire on the inner dispatch. Folded into Phase A/B work.

### Out — never belongs here
- Primitives changes (D-M86-10 invariant from M8.6 continues to apply)
- New Hook trait methods (the 9 are stable; this plan plumbs the existing ones correctly)

## 3. Verified audit baseline (2026-05-28)

Every `tool.call()` site in motosan-agent-loop's src tree:

| Site | Goes through pipeline? |
|---|---|
| `src/core/engine.rs:3879` (inside `execute_tools_parallel`) | ✅ when reached via `execute_tools_with_policy` |
| `src/mcp/adapter.rs:149, 185` (McpToolAdapter internals) | ✅ not a user-facing dispatch; adapter calls underlying MCP transport |
| `src/planning.rs:478` (PlanningTool inner) | ❌ **confirmed bypass** — PlanningTool synchronously calls sub-tools via `block_on(tool.call(...))`, skipping policy/interceptors. Folded into Phase B scope. |
| **`src/streaming_executor.rs:53`** | ❌ **bypasses everything** |

External callers of `execute_tools_with_policy`: only 2 — `engine.rs:1732, 1977`. Both are non-streaming batch paths.

`StreamingToolExecutor` is re-exported as public API at `lib.rs:94`. It's the consumed-by-streaming-response-handler path. Production agemo uses it.

Interceptor chain bug (gap #3) verified at `src/core/interceptor_set.rs:11`:
> "intercept_tool_call: iterate in order, **stop at first non-`Proceed(orig)`**"

This is incorrect per primitives D2-revised spec (each hook sees `updated_input` from previous). The chain breaks early on any rewrite.

## 4. Shared design decisions

### D-861-1. Single unified pipeline entry point

Define a single function in `motosan-agent-loop/src/core/dispatch.rs` (new file):

```rust
pub(crate) async fn dispatch_tool_call(
    item: ToolCallItem,
    tool_map: &HashMap<String, Arc<dyn Tool>>,
    interceptors: &mut InterceptorSet,
    policy: Option<&Arc<dyn PermissionPolicy>>,
    ctx: &mut HookCtx<'_>,
    on_event: &EventSink,
    ops_rx: &mut Option<mpsc::Receiver<AgentOp>>,
    /* ... timing + cancellation hooks ... */
) -> ToolOutput
```

This function is the ONLY way the engine invokes a user-visible tool. It encapsulates: permission check → interceptor chain (with rewrite propagation) → deferral resolution → `tool.call()` → result-rewrite chain.

Both `execute_tools_parallel` (batch path) and the streaming path become consumers of `dispatch_tool_call`.

### D-861-2. Streaming path refactor

`StreamingToolExecutor::submit` currently calls `tool.call(args, &ctx)` directly inside a spawned task. Change to:

```rust
pub fn submit(&mut self, item: ToolCallItem, dispatch_ctx: DispatchCtx) {
    let handle = tokio::spawn(async move {
        dispatch_tool_call(item, /* ... from dispatch_ctx ... */).await
    });
    self.pending.push((item, handle));
}
```

`DispatchCtx` is a new struct that bundles the references `dispatch_tool_call` needs (interceptors, policy, on_event, etc.). Each `submit` clones what it needs.

**Concurrency consideration:** the interceptor chain mutates state via `&mut`. Spawning multiple tools concurrently means multiple tasks need mutable access to `&mut InterceptorSet`. Options:
- **(a)** Take a snapshot of the chain at submit time, run it inside the task. Requires `Clone` on interceptors (none are today).
- **(b)** Serialize interceptor dispatch on a Mutex/RwLock per `InterceptorSet`. Tool execution still runs in parallel after the chain returns Proceed.
- **(c)** Restructure so the chain runs at submit() time (synchronous within the streaming handler thread), then only the actual `tool.call()` runs in the spawned task.

**Recommendation: (c).** The chain is fast (it's middleware); doing it inline at submit time keeps the spawned task purely about tool execution. No locks needed.

### D-861-3. Hook rewrite propagation — fix is in the adapter, not the chain

**Initial diagnosis was wrong.** Re-reading `interceptor_set.rs:218-247` shows the chain code IS correct — it propagates `current` through the loop so each iteration sees the latest `Proceed(updated)`:

```rust
// interceptor_set.rs (existing, correct)
let mut current = ToolDecision::Proceed(call);
for (ext, _policy) in &mut self.extensions {
    let original_call = match &current {
        ToolDecision::Proceed(c) => c.clone(),  // ← reads latest Proceed from prior iter
        _ => return Ok(current),
    };
    match ext.intercept_tool_call(original_call.clone(), ...).await {
        Ok(decision) => current = decision,
        ...
    }
}
```

**The real bug is in `HookInterceptorAdapter::map_hook_result`** (`src/core/hook_adapter.rs`), which maps `HookResult::Continue { updated_input: Some(_) }` to `ToolDecision::Replace`, which is a TERMINAL decision that short-circuits the chain. So any rewriting Hook breaks the chain for all subsequent Hooks — matching Wade's symptom exactly.

**Fix is 1 line:**

```rust
// hook_adapter.rs — buggy version
HookResult::Continue { updated_input: Some(input) } => {
    let mut replaced = item.clone();
    replaced.args = input;
    ToolDecision::Replace(replaced)   // ← terminal, breaks chain
}

// hook_adapter.rs — fixed
HookResult::Continue { updated_input: Some(input) } => {
    let mut replaced = item.clone();
    replaced.args = input;
    ToolDecision::Proceed(replaced)   // ← propagates through chain
}
```

**Semantic rationale:** `Continue { updated_input }` is "same tool, rewritten args" — that's Proceed-with-rewrite. `Replace` is for "substitute a totally different call instead" (different tool, different args, prior hooks should have stopped). Conflating them was the bug.

**No chain refactor needed.** The interceptor_set chain is already correct.

### D-861-4. AskUser routing via ToolDecision::Defer (closes #195)

Per #195's recommended fix. Replace the inline `rx.recv().await` loop at `engine.rs:3331-3349` with:

```rust
Permission::AskUser { prompt } => {
    let question = prompt.unwrap_or_else(|| default_prompt(&item.name, &item.args));
    on_event(AgentEvent::LoopInterceptor(ExtensionEvent::AskUser(...)));
    return ToolDecision::Defer {
        call_id: item.id.clone(),
        reason: "awaiting permission approval".into(),
    };
}
```

The engine's existing deferral pipeline + `AskUserExtension`-style FIFO consumes `AgentOp::AskUserAnswer` and resumes. Other tools in the same batch are NOT blocked.

### D-861-5. Subagent stop unification (closes subagent#1)

Per subagent#1's Option A: introduce a single status-transition gate in `motosan-agent-subagent/src/subagent/manager.rs` that wraps every terminal-status write. The gate:
1. Updates `SubagentStatus`
2. Constructs a `SubagentResult` with the appropriate `StopReason` (per the mapping table in subagent#1)
3. Invokes `on_subagent_stop` via the interceptor dispatcher
4. Is **idempotent**: double-write doesn't fire double notifications

Migrate all 6 terminal-status writes (manager.rs:359-362, 790-796; driver.rs:67, 81-83; handle.rs:32) to call the gate instead of writing status directly.

### D-861-6. Architectural invariant tests

Add tests that fail if a future change reintroduces a bypass:

```rust
// tests/architectural_invariants.rs
#[test]
fn no_direct_tool_call_outside_dispatch() {
    let bypass_sites = find_tool_call_sites_outside_dispatch_module();
    let allowlist = [
        "src/mcp/adapter.rs",        // MCP internals — calls underlying transport, not user tools
        "src/planning.rs",           // documented special case (out of scope per §2)
        "src/core/dispatch.rs",      // the unified path itself
    ];
    let unexplained: Vec<_> = bypass_sites.into_iter()
        .filter(|s| !allowlist.iter().any(|a| s.starts_with(a)))
        .collect();
    assert!(unexplained.is_empty(), "tool.call() bypass found: {unexplained:#?}");
}
```

Same pattern for "every `submit()` in streaming goes through `dispatch_tool_call`."

This is the M8.6 lesson: catch architectural drift via tests, not just per-feature tests.

### D-861-7. Versioning

| Crate | Old | New | Reason |
|---|---|---|---|
| motosan-agent-loop | 0.24.0 | **0.25.0** | streaming refactor + interceptor chain semantics change (minor — old behavior is technically a bug but downstream might depend on it) |
| motosan-agent-subagent | 0.3.0 | **0.3.1** | additive: all termination paths now notify (patch — no API change) |
| agemo | 0.1.1 | (likely unchanged) | only test additions, if any |
| primitives, tool, ai, sandbox, harness | unchanged | — | not touched |

### D-861-8. Workflow + chat still deferred

Same as M8.6 D-M86-11. Sticking with the punt.

## 5. Implementation phases

```
Phase A — Architectural audit + unified dispatch design                   [1 day]
   └── gate: dispatch_tool_call signature stabilized + DispatchCtx defined

Phase B — Implement dispatch_tool_call + migrate execute_tools_parallel   [1.5 days]
   └── gate: batch path goes through dispatch_tool_call; existing tests still green

Phase C — Streaming path migration (D-861-2 option c)                     [1.5 days]
   └── gate: StreamingToolExecutor uses dispatch_tool_call; new streaming permission test green

Phase D — Interceptor chain rewrite propagation (D-861-3)                 [0.5 day]
   └── gate: new multi-hook chain test green; existing tests still pass

Phase E — AskUser ToolDecision::Defer migration (D-861-4 / closes #195)   [1 day]
   └── gate: parallel-batch test green; wildcard interference test green;
            permission_gating.rs (4 existing tests) still green

Phase F — Loop checkpoint (commit + push 0.25.0)                          [0.5 day]  ⚠️ CHECKPOINT

Phase G — Subagent stop unification (D-861-5 / closes subagent#1)         [1 day]
   └── gate: 3 new termination-path tests green + existing test still green

Phase H — Subagent commit + push 0.3.1                                    [0.5 day]

Phase I — Architectural invariant tests + cross-repo verify               [0.5 day]
   └── gate: invariant tests green; all 8 repos build + test green
```

**Total: 7.5 focused days + 1-2 days unknown-unknowns = 8-9 days. Calendar: 1.5-2 weeks.**

### Phase A — Architectural audit (1 day)

1. Enumerate every `tool.call()` site in loop (done — see §3 baseline). Re-verify no new sites since baseline.
2. Design `DispatchCtx` struct: what state does the unified pipeline need to thread?
3. Design `dispatch_tool_call` signature. Specifically resolve:
   - Sync vs async boundaries for the streaming path
   - How `ops_rx` is shared/cloned across spawned tasks
   - Cancellation token integration
4. Write Phase A design notes inline in `src/core/dispatch.rs` (just the struct + signature + doc comments — no implementation yet).
5. **Gate:** `cargo check` passes with the new file present; signature reviewed by Wade.

### Phase B — Batch path + PlanningTool migration (2 days)

1. Implement `dispatch_tool_call` body: extract the per-tool logic currently in `execute_tools_with_policy`'s loop (lines 3290-3375) into the unified function.
2. Refactor `execute_tools_with_policy` to call `dispatch_tool_call` per item.
3. **PlanningTool migration (per resolved Q1):** rewrite `planning.rs:478` to `block_on(dispatch_tool_call(item, ...))` instead of `block_on(tool.call(args, &ctx))`. PlanningTool needs access to interceptors + policy at construction time — add a constructor variant `PlanningTool::with_dispatch_ctx(...)` or pass a thread-safe handle on each invocation.
4. **New test** `tests/planning_subtools_gated.rs`: PlanningTool that calls a `place_order` sub-tool + a Deny-everything PermissionPolicy → assert the inner `place_order` was blocked.
5. **Gate:** all existing tests still green; `permission_gating.rs` (4 tests) still green; `hook_lifecycle.rs` + `hook_interceptor_parity.rs` still green; new `planning_subtools_gated` test green.

### Phase C — Streaming path migration (1.5 days)

1. Refactor `StreamingToolExecutor::submit` per D-861-2 option (c): chain runs inline at submit time, only `tool.call()` runs in the spawned task.
2. Update `StreamingToolExecutor`'s public API (this might be a breaking change — note in CHANGELOG).
3. Update any in-tree caller (search for `StreamingToolExecutor::submit(`).
4. **New test** `tests/streaming_permission.rs`: a streaming response with tool_use chunks + a Deny-everything PermissionPolicy → assert NO tool was actually invoked.
5. **Gate:** new streaming_permission test green; existing streaming_executor tests still green.

### Phase D — Hook adapter rewrite-propagation fix (0.25 day)

1. Modify `HookInterceptorAdapter::map_hook_result` at `src/core/hook_adapter.rs` per D-861-3 — 1-line change: `ToolDecision::Replace(replaced)` → `ToolDecision::Proceed(replaced)`.
2. **New test** `tests/hook_chain_rewrite.rs`: register 3 Hooks, each appends a key to the tool args via `HookResult::Continue { updated_input: Some(...) }`. Run a tool call. Assert the final tool sees all 3 keys composed — proves the chain propagates.
3. (No `interceptor_set.rs` change needed — chain code is correct as-is.)
4. **Gate:** new test green; existing tests still green.

### Phase E — AskUser via Defer (1 day)

1. Remove the inline `rx.recv().await` loop at `engine.rs:3331-3349`.
2. Replace with `ToolDecision::Defer { call_id, reason }` return.
3. Add a small handler that consumes `AgentOp::AskUserAnswer` and converts to `OpDecision::ResumeDeferred { call_id, result: ToolOutput }` — likely a new internal interceptor or extension of permission_runtime.
4. **New test** `tests/permission_parallel_batch.rs`: a batch of 3 tools where 1 requires AskUser, 2 are auto-allow → assert the 2 run to completion BEFORE the answer arrives; then send answer → assert the 3rd resumes.
5. **New test** `tests/permission_wildcard_isolation.rs`: a `PermissionPolicy::AskUser` deferral + a parallel `ask_user` tool call → send a wildcard answer → assert it goes to the FIFO-oldest deferral (per #195 acceptance).
6. **Gate:** both new tests green; original 4 `permission_gating.rs` tests still green.

### Phase F — Loop checkpoint (0.5 day) ⚠️ MANDATORY

- CHANGELOG 0.25.0 entry
- Commit: `feat: motosan-agent-loop 0.25.0 — unified tool dispatch + middleware completeness`
- Push to motosan-dev/motosan-agent-loop
- **Report back** with: commit SHA, test count delta, the 4 new test files passing, any deviations
- Wait for Wade approval before Phase G

### Phase G — Subagent stop unification (1 day)

1. Add `manager::record_termination(status, reason)` (or similar) that becomes the only path to a terminal `SubagentStatus`. Per D-861-5 + subagent#1 Option A.
2. Migrate 6 terminal-status writes to call through the gate.
3. **New test** `subagent_stop_fires_on_natural_completion`
4. **New test** `subagent_stop_fires_on_child_failure`
5. **New test** `subagent_stop_fires_on_parent_cancellation`
6. CHANGELOG 0.3.1 entry
7. **Gate:** 3 new tests + existing explicit-close test all green.

### Phase H — Subagent commit + push (0.5 day)

- Commit + push subagent 0.3.1

### Phase I — Architectural invariants + cross-repo verify (0.5 day)

1. Implement `tests/architectural_invariants.rs` per D-861-6.
2. Cross-cutting verify loop (same as M8.6's §J, updated):

```bash
for repo in motosan-agent-tool motosan-agent-loop motosan-ai/sdks/rust \
            motosan-agent-subagent motosan-sandbox motosan-agent-harness agemo; do
  cd /Users/daiwanwei/Projects/wade/$repo
  case "$repo" in
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

3. Verify primitives received zero commits this cycle.

## 6. Acceptance gates (final)

1. `cargo build && cargo test --all-features` green in loop 0.25.0 and subagent 0.3.1
2. `cargo build && cargo test` green in all other repos (unchanged)
3. **Architectural invariant test green**: no `tool.call()` outside the allowlisted modules
4. **Streaming permission test green**: streaming path applies PermissionPolicy
5. **Parallel batch test green**: AskUser-deferred tool doesn't block siblings in the same batch
6. **Interceptor chain rewrite test green**: 3 sequential interceptor rewrites compose correctly
7. **Subagent termination tests green**: natural completion + child failure + parent cancellation all fire `on_subagent_stop`
8. **#195 closed**: AskUser routes through ToolDecision::Defer; wildcard interference test green
9. **subagent#1 closed**: all 6 termination paths route through the gate
10. **No primitives commit** during this cycle
11. **agemo unchanged** unless a new test demonstrates the streaming permission fix end-to-end

## 7. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| **Streaming refactor breaks existing streaming tests** (timing, ordering) | High | Phase C gate runs full streaming suite. If a timing assertion fails, investigate whether the test was asserting on bypass behavior (test is wrong) vs. real ordering change (refactor is wrong) |
| Hook adapter 1-line change (D-861-3) breaks something subtle in pre_tool_use rewriting | Low | Single-line semantic-clarification fix; multi-hook test (Phase D step 2) verifies the desired behavior. No interceptor chain change means existing in-tree interceptors are unaffected. |
| `DispatchCtx` over- or under-bundles state, causing API churn mid-implementation | Medium | Phase A gate is "signature stabilized by Wade." Iterate on paper before code. |
| `dispatch_tool_call` mutable state needs (interceptor `&mut self`) cause borrow-checker pain in streaming refactor | Medium-High | Recommendation is option (c) — chain runs inline at submit time. If that doesn't fly, fall back to option (a) snapshot or accept a serialization point |
| AskUser ToolDecision::Defer requires deferred-call infrastructure that doesn't quite fit (e.g., the existing AskUserExtension's FIFO doesn't accept policy-driven deferrals) | Medium | Phase E may need a small new internal "permission_defer" handler that participates in the same FIFO. Plan §D-861-4 sketches this; concrete shape decided at implementation time |
| Phase F checkpoint reveals streaming refactor wasn't right and Phase C needs redo | Medium | Mandatory checkpoint exists for this. Cost of rework bounded to Phase C + D (~2 days). |
| Subagent gate refactor (D-861-5) touches manager.rs which is 28KB — risk of breaking existing subagent behavior | Medium | Existing 32 subagent tests are the safety net. If any regress, halt and investigate before pushing |
| Architectural invariant test (D-861-6) is brittle (grep-based) and breaks on cosmetic changes | Low | Keep the test focused: only check that `tool.call(` outside the dispatch module appears in the allowlist. Cosmetic changes elsewhere don't trigger it |

## 8. Open questions

### Q1. Does `PlanningTool::call` at `planning.rs:478` actually bypass policy?

**Resolved (2026-05-28):** Yes, confirmed bypass. PlanningTool synchronously calls sub-tools via `block_on(tool.call(...))`, skipping the unified dispatch entirely. M9's FinanceHarness can use PlanningTool to compose `place_order` etc., and the gating policy must fire on those inner calls.

**Decision:** fold into Phase B scope. PlanningTool's `block_on(tool.call(...))` becomes `block_on(dispatch_tool_call(...))`. Adds ~0.5 day to Phase B. Phase B updated below.

### Q2. Should `StreamingToolExecutor`'s public API break, or do we keep a back-compat wrapper?

The refactor changes `submit()`'s signature (needs `DispatchCtx`, not just tool_map + ctx). External consumers (if any) will break.

**Recommendation:** break it. The crate is internal-use; the only consumer is the engine. Document in CHANGELOG. If a real out-of-tree consumer surfaces post-release, add a 0.25.1 with a back-compat shim.

### Q3. Does the new `dispatch_tool_call` live in `core::dispatch` or merged into `core::engine`?

Style preference. New module is cleaner; merging keeps grep-locality.

**Recommendation:** new module `src/core/dispatch.rs`. The unified pipeline is a coherent concern that deserves its own namespace.

## 9. What this plan does NOT cover

- M9 itself (still blocked until M8.6.1 lands)
- Workflow + chat repos
- `PlanningTool::call` deep refactor (see Q1)
- Re-architecting the streaming response model itself (we keep eager execution semantics; just route them through the unified pipeline)

## 10. Effort estimate

| Phase | Days | Notes |
|---|---|---|
| A. Architectural audit + design | 1 | Mostly design work; small file added |
| B. Batch path + PlanningTool migration | 2 | Refactor + PlanningTool inner dispatch (resolved Q1) |
| C. Streaming path migration | 1.5 | Hardest single phase; concurrency-sensitive |
| D. Hook adapter rewrite fix | 0.25 | 1-line change + multi-hook chain test |
| E. AskUser via Defer | 1 | Closes #195 |
| F. Loop checkpoint | 0.5 | Commit + push |
| G. Subagent stop unification | 1 | Closes subagent#1 |
| H. Subagent commit + push | 0.5 | |
| I. Invariants + cross-repo verify | 0.5 | The "never regress" net |
| **Total** | **8.25** | + 1-2 unknown-unknowns. PlanningTool (+0.5) + Hook adapter shrink (-0.25) = net +0.25 vs first draft |

**Calendar: 1.5-2 weeks** focused.
