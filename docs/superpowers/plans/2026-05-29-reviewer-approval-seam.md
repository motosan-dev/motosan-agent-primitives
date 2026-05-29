# Reviewer / Approval Seam — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a swappable, async, session-scoped `Reviewer` seam that resolves `Permission::AskUser` escalations — spanning pi-minimal (never escalate) to Codex-heavy (guardian agent, multi-agent child approval) on one contract.

**Architecture:** Keep `PermissionPolicy` as the decision layer (`Allow | Deny | AskUser`, composed most-restrictive-wins). Add a new `Reviewer` trait in primitives that resolves an `AskUser` into `Approve | Deny`. In the loop, the **existing** `Permission::AskUser` machinery (emit `AskUser` event → `deferred_calls` → `AgentOp::AskUserAnswer`) is refactored to live *behind* a `DeferredAskUserReviewer`, making it swappable and reachable from child engines. Default-when-none is a fail-safe `DenyReviewer`.

**Tech Stack:** Rust, `async-trait`, `tokio` / `tokio-util` (`CancellationToken`), `serde`. Crates: `motosan-agent-primitives` (0.3.0), `motosan-agent-loop`, `motosan-agent-subagent`, `agemo`.

**Source spec:** [docs/superpowers/specs/2026-05-29-reviewer-approval-seam-design.md](../specs/2026-05-29-reviewer-approval-seam-design.md)

---

## Scope & sequencing

This feature spans four crates with a strict dependency order. It is **one coherent feature**, not independent subsystems, so it is one plan with four phases. Each phase is independently committable and testable.

- **Phase 1 — primitives:** new `Reviewer` trait + `ApprovalRequest` + `ReviewDecision`. Purely additive (no existing item changes). Fully detailed below.
- **Phase 2 — loop:** `DenyReviewer`, `DeferredAskUserReviewer` (wraps today's behavior), `EngineBuilder::reviewer()`, route `consult_policy` `AskUser` → reviewer, per-session serialization.
- **Phase 3 — subagent:** `SubagentConfig::inherit_approval_from` sugar so a child's `AskUser` routes to the parent session's reviewer/ops channel (the actual gap).
- **Phase 4 — agemo:** migrate the existing stdin bridge onto `DeferredAskUserReviewer` (behavior-preserving).

> **Granularity calibration (deliberate, not a placeholder):** Phase 1 is fine-grained TDD. Phases 2–4 are written at task level (exact files, signatures, code sketches, test intent) because they touch `motosan-agent-loop` internals that are in active flux (primitives just moved 0.2.0→0.3.0 during design) and the spec gates them **post-M11**. When a phase is actually picked up, re-read the then-current `engine.rs` / `permission_runtime.rs` and expand its tasks into TDD micro-steps against the real code.

**Out of scope for this plan (P5):** `GuardianReviewer` and composite/escalating reviewers are **user-land impls** (a host or vertical writes them against the `Reviewer` trait — spec §4 Tier 3 shows them as usage examples, not framework deliverables). This plan ships only the framework pieces: the trait + types (Phase 1), the `DenyReviewer` default and `DeferredAskUserReviewer` (Phase 2), inheritance sugar (Phase 3), and the agemo migration (Phase 4). If a built-in guardian is later wanted, it is a separate plan. The guardian-recursion guard (spec §4 F4) is still documented in Task 8 because the inheritance sugar must not be misused when someone *does* build a guardian.

**Do not start before M11** (rental harness → 1.0 freeze), per the spec §14.

---

## File structure

| File | Phase | Responsibility |
|---|---|---|
| `motosan-agent-primitives/src/approval.rs` (create) | 1 | `Reviewer` trait, `ApprovalRequest`, `ReviewDecision` |
| `motosan-agent-primitives/src/lib.rs` (modify) | 1 | `pub mod approval;` + re-exports |
| `motosan-agent-primitives/CHANGELOG.md` (modify) | 1 | 0.4.0 entry (additive) |
| `motosan-agent-loop/src/core/reviewer.rs` (create) | 2 | `DenyReviewer`, `DeferredAskUserReviewer` |
| `motosan-agent-loop/src/core/engine.rs` (modify) | 2 | `EngineBuilder::reviewer()`; route `AskUser` → reviewer; per-session approval serialization |
| `motosan-agent-loop/src/core/permission_runtime.rs` (modify) | 2 | helper to build an `ApprovalRequest` from the consult inputs |
| `motosan-agent-subagent/src/subagent/config.rs` (modify) | 3 | `inherit_approval_from` |
| `agemo/src/main.rs` (modify) | 4 | construct/pass `DeferredAskUserReviewer` |

---

## Phase 1 — primitives: the `Reviewer` contract

Grounding (verified in current code, primitives 0.3.0):
- `src/permission.rs`: `Permission::{Allow, Deny { reason }, AskUser { prompt: Option<String> }}`, `PermissionContext<'a>`, `PermissionPolicy::check`.
- `src/tool.rs`: `ToolCall` (L71), `ToolAnnotations` (L152) — unchanged by the ToolSchema (#1) addition.
- `src/message.rs`: `Message`, `Role`.
- Crate already depends on `tokio-util` for `CancellationToken` (used by hook `*Ctx` structs).

### Task 1: `ReviewDecision` enum

**Files:**
- Create: `motosan-agent-primitives/src/approval.rs`
- Modify: `motosan-agent-primitives/src/lib.rs`

- [ ] **Step 1: Write the failing test** (in `src/approval.rs`)

```rust
//! Approval resolution contract — the answering half of `Permission::AskUser`.
//!
//! [`PermissionPolicy`](crate::permission::PermissionPolicy) *decides*
//! (`Allow | Deny | AskUser`). When the composed decision is `AskUser`, the
//! framework consults the session's single [`Reviewer`] to *resolve* it into a
//! final [`ReviewDecision`]. See the design spec for the full rationale.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::message::Message;
use crate::tool::{ToolAnnotations, ToolCall};

/// Final verdict produced by a [`Reviewer`] for an escalated tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewDecision {
    /// Proceed with the tool call.
    Approve,
    /// Block the tool call; `reason` is shown to the model (same path as
    /// [`Permission::Deny`](crate::permission::Permission::Deny)).
    Deny {
        /// Human-readable explanation.
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_decision_round_trips() {
        for d in [
            ReviewDecision::Approve,
            ReviewDecision::Deny { reason: "no".into() },
        ] {
            let s = serde_json::to_string(&d).unwrap();
            let back: ReviewDecision = serde_json::from_str(&s).unwrap();
            assert_eq!(d, back);
        }
    }
}
```

> Note: `ReviewDecision::Deny { reason }` (not a bare `Deny`) so a reviewer can explain itself to the model, matching `Permission::Deny`. The spec sketched `Deny` bare; this is the resolved, richer shape — update the spec's §3 sketch when implementing.

- [ ] **Step 2: Wire the module.** In `src/lib.rs` add `pub mod approval;` and `pub use approval::ReviewDecision;` only (Tasks 2–3 extend the re-export to add `ApprovalRequest` then `Reviewer`). Match the existing `pub use permission::{...}` style. (Do not re-export not-yet-defined items — no failing-then-narrowing dance.)

- [ ] **Step 3: Run the test, expect PASS**

Run: `cargo test -p motosan-agent-primitives approval::tests::review_decision_round_trips`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add src/approval.rs src/lib.rs
git commit -m "feat: add ReviewDecision to primitives approval module"
```

### Task 2: `ApprovalRequest` (owned, cancellable)

**Files:**
- Modify: `motosan-agent-primitives/src/approval.rs`

- [ ] **Step 1: Write the failing test** (append to `src/approval.rs`)

```rust
#[cfg(test)]
mod request_tests {
    use super::*;
    use crate::message::Role;
    use crate::tool::ToolCall;

    fn sample_request() -> ApprovalRequest {
        ApprovalRequest {
            tool_call: ToolCall {
                id: "call-1".into(),
                name: "place_order".into(),
                input: serde_json::json!({ "symbol": "AAPL", "qty": 10 }),
            },
            annotations: ToolAnnotations { destructive: true, ..Default::default() },
            session_id: "sess-1".into(),
            recent_messages: vec![Message::text(Role::User, "buy 10 AAPL")],
            prompt: Some("Approve buying 10 AAPL?".into()),
            cancellation_token: CancellationToken::new(),
        }
    }

    #[test] // no runtime needed — pure ownership/clone checks
    fn approval_request_is_cloneable_and_retains_data() {
        let req = sample_request();
        let copy = req.clone();                          // fan-out composites clone it
        assert_eq!(copy.tool_call.name, "place_order");
        assert_eq!(req.recent_messages.len(), 1);        // original still usable after clone
        assert_eq!(req.prompt.as_deref(), Some("Approve buying 10 AAPL?"));
        // owned → Send + 'static (compile-time proof, NO tokio runtime)
        fn _assert_send_static<T: Send + 'static>() {}
        _assert_send_static::<ApprovalRequest>();
    }
}
```

- [ ] **Step 2: Implement `ApprovalRequest`** (add to `src/approval.rs`, before the test modules). Verify `ToolCall` field names against current `src/tool.rs` (`id`, `name`, `input`) before writing the test fixture.

```rust
/// Everything a [`Reviewer`] needs to resolve one escalated tool call.
///
/// **Owned, not borrowed** (cf. `PermissionContext<'a>`): a reviewer may
/// queue this, move it to another task/thread, or hold it across a long
/// guardian turn, so it cannot borrow from the engine's transient state.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    /// The tool call awaiting approval.
    pub tool_call: ToolCall,
    /// Annotations declared by the target tool.
    pub annotations: ToolAnnotations,
    /// Session that raised the escalation (distinct per child agent).
    pub session_id: String,
    /// Snapshot of the engine's recent-message window (may be empty).
    pub recent_messages: Vec<Message>,
    /// Prompt passed straight through from `Permission::AskUser { prompt }`.
    pub prompt: Option<String>,
    /// Observe this; return `Deny` (or abort) when the turn is cancelled.
    pub cancellation_token: CancellationToken,
}
```

- [ ] **Step 3: Run the test, expect PASS**

Run: `cargo test -p motosan-agent-primitives approval::request_tests`
Expected: PASS (requires `tokio` dev-dep with `rt`/`macros`, already present per `Cargo.toml`).

- [ ] **Step 4: Commit**

```bash
git add src/approval.rs
git commit -m "feat: add owned cancellable ApprovalRequest"
```

### Task 3: `Reviewer` trait (object-safe, by-value)

**Files:**
- Modify: `motosan-agent-primitives/src/approval.rs`, `motosan-agent-primitives/src/lib.rs`

- [ ] **Step 1: Write the failing test** (append to `src/approval.rs`)

```rust
#[cfg(test)]
mod reviewer_tests {
    use super::*;
    use std::sync::Arc;

    struct AlwaysApprove;
    #[async_trait]
    impl Reviewer for AlwaysApprove {
        async fn review(&self, _req: ApprovalRequest) -> ReviewDecision {
            ReviewDecision::Approve
        }
    }

    /// Object-safety smoke test: must compile.
    #[allow(dead_code)]
    fn assert_object_safe(_r: Arc<dyn Reviewer>) {}

    #[tokio::test]
    async fn reviewer_resolves_by_value() {
        let r: Arc<dyn Reviewer> = Arc::new(AlwaysApprove);
        let req = super::request_tests::sample_request(); // reuse fixture (make it pub(crate))
        assert_eq!(r.review(req).await, ReviewDecision::Approve);
    }
}
```

- [ ] **Step 2: Implement the trait** (add to `src/approval.rs`)

```rust
/// Resolves an escalated tool call into a final [`ReviewDecision`].
///
/// Exactly one per session; consulted **only** when the composed
/// [`PermissionPolicy`](crate::permission::PermissionPolicy) decision is
/// [`Permission::AskUser`](crate::permission::Permission::AskUser). Takes the
/// request **by value** so the reviewer may move/queue/defer it. Object-safe:
/// usable as `Arc<dyn Reviewer>`.
#[async_trait]
pub trait Reviewer: Send + Sync {
    /// Resolve `req`. Long waits MUST race `req.cancellation_token`.
    async fn review(&self, req: ApprovalRequest) -> ReviewDecision;
}
```

Make `request_tests::sample_request` `pub(crate)` so the reviewer test can reuse it. Restore the full re-export in `lib.rs`: `pub use approval::{ApprovalRequest, ReviewDecision, Reviewer};`.

- [ ] **Step 3: Run the tests, expect PASS**

Run: `cargo test -p motosan-agent-primitives approval::`
Expected: PASS (all three modules).

- [ ] **Step 4: Object-safety + full build**

Run: `cargo build -p motosan-agent-primitives --all-features`
Expected: clean (the `assert_object_safe` proves `dyn Reviewer`).

- [ ] **Step 5: CHANGELOG + commit**

Add a `0.4.0` entry to `CHANGELOG.md`: "ADDED: `Reviewer` trait, `ApprovalRequest`, `ReviewDecision` (approval-resolution seam under `Permission::AskUser`). Additive — no existing item changed."

```bash
git add src/approval.rs src/lib.rs CHANGELOG.md
git commit -m "feat: add Reviewer trait (approval-resolution seam)"
```

### Task 4: doctest + version bump

- [ ] **Step 1:** Add a crate-level doctest to `approval.rs` showing a minimal `Reviewer` impl (mark `no_run` only if it needs an engine; a pure `AlwaysApprove` example runs fine). Cover the three paths in the rustdoc like `permission.rs` does.
- [ ] **Step 2:** Bump `Cargo.toml` version `0.3.x` → `0.4.0`.
- [ ] **Step 3:** Run `cargo test -p motosan-agent-primitives --all-features` (incl. doctests — requires the home-config `rustdocflags` fix already in place). Expected: green.
- [ ] **Step 4:** Commit `chore: primitives 0.4.0 — Reviewer seam`.

**Phase 1 acceptance:** `cargo build/test -p motosan-agent-primitives --all-features` green; every existing `PermissionPolicy`/`Hook` impl untouched; `Arc<dyn Reviewer>` compiles.

---

## Phase 2 — loop: default reviewers + wiring

Grounding (verified in current `motosan-agent-loop`):
- `src/core/engine.rs` ~L3526: `match consult_policy(...)` with `Allow` (emit ToolStarted + dispatch), `Deny { reason }` (resolved error slot), `AskUser { prompt }` (emit `ExtensionEvent::AskUser`, insert into `deferred_calls` keyed by `item.id`, resolved later via `AgentOp::AskUserAnswer`).
- `EngineBuilder` setters in `engine.rs`: `tools`@468, `system_prompt`@478, `hooks`@635, `permission_policy`@641, `memory_schema`@647, `session_id`@660.
- `src/core/permission_runtime.rs`: `consult_policy(policy, session_id, tool_use_id, tool_name, tool_input, annotations, recent_messages) -> Permission`; `default_prompt(name, args)`.

### Task 5: `DenyReviewer` + `DeferredAskUserReviewer`

**Files:** Create `motosan-agent-loop/src/core/reviewer.rs`; register `mod reviewer;` in `src/core/mod.rs`.

- Implement `DenyReviewer` (returns `ReviewDecision::Deny { reason: "no reviewer configured".into() }`) — the default-when-none.
- Implement `DeferredAskUserReviewer`: holds the handles the current `AskUser` arm uses (the `on_event` emitter + `deferred_calls`/ops resolution path, or whatever the then-current engine exposes). Its `review(req)` reproduces today's behavior: emit the `ExtensionEvent::AskUser` question (using `req.prompt` or `default_prompt`), then await the matching `AgentOp::AskUserAnswer` (racing `req.cancellation_token`), mapping allow→`Approve`, deny→`Deny`.
- **Tests:** unit test `DenyReviewer` returns Deny; for `DeferredAskUserReviewer`, a test that feeds a synthetic answer op and asserts the mapped decision, plus a cancellation test (cancel the token → resolves to `Deny`). Model these on the existing AskUser-interceptor tests in the loop test suite.
- **Commit:** `feat(loop): add DenyReviewer + DeferredAskUserReviewer`.

### Task 6: `EngineBuilder::reviewer()` + Engine field

**Files:** Modify `engine.rs` (builder + `Engine` struct + `build()`).

- Add `reviewer: Option<Arc<dyn Reviewer>>` to `EngineBuilder` and `Engine`; setter `pub fn reviewer(mut self, r: Arc<dyn Reviewer>) -> Self` next to `permission_policy` (@641).
- In `build()`, default to `Arc::new(DenyReviewer)` when unset. (Decision: default is `DenyReviewer`, NOT `DeferredAskUserReviewer` — a host that wants interactive approval wires the deferred one explicitly; this makes "child nobody wired" fail safe. Call this out in the loop CHANGELOG as a behavior change vs today's stall.)
- Add accessor `pub fn reviewer(&self) -> &Arc<dyn Reviewer>`.
- **Test:** builder stores the reviewer; unset → `DenyReviewer`.
- **Commit:** `feat(loop): EngineBuilder::reviewer() + DenyReviewer default`.

### Task 7: route `AskUser` → reviewer + serialize

**Files:** Modify `engine.rs` (the `consult_policy` match ~L3557) and add a per-session approval mutex.

- Replace the inline `AskUser { prompt }` arm's event/defer logic with: build an `ApprovalRequest` (via a new `permission_runtime::approval_request_from(...)` helper that owns/clones `tool_call`, `annotations`, `session_id`, the `recent_messages_owned` already computed above, and `prompt`, plus the engine's `cancellation_token`), then `self.reviewer.review(req).await`, mapping `Approve` → the same path as `Allow` (emit ToolStarted + dispatch) and `Deny { reason }` → the same resolved-error slot as the `Deny` arm.
- **Serialization lives in the shared resource, NOT the Engine (P3).** The approval mutex must guard the *answering channel*, so it belongs **inside the reviewer** (e.g. a `tokio::sync::Mutex` held by `DeferredAskUserReviewer` around its emit-event/await-answer critical section), not on each `Engine`. Reason: in multi-agent (Phase 3) a parent and its children **share one reviewer instance**; a per-Engine mutex would let parent + child escalations hit the same event/ops channel concurrently and race. Putting the lock in the shared reviewer serializes across all engines that share it. `DenyReviewer` needs no lock (no shared channel).
- **Preserve the non-blocking-batch semantic (P4 — the deepest risk).** Today the `AskUser` arm *defers* the call (`deferred_calls` + resume) so the rest of a parallel batch keeps running; this refactor routes through `reviewer.review().await`, which is an inline await. The refactor MUST keep an `Allow`/`Deny` sibling in the same batch from being blocked by another sibling's pending `AskUser`. Two ways: (i) keep the defer/resume machinery underneath and have `DeferredAskUserReviewer::review()` drive it (preferred — least behavior change), or (ii) spawn each slot's resolution so awaits don't serialize the batch. Pick (i) unless it proves infeasible against the then-current engine; document whichever in the commit.
- **Tests:** (a) policy `AskUser` + `AlwaysApprove` reviewer → tool runs; (b) `AskUser` + default `DenyReviewer` → blocked; (c) **non-blocking batch (P4):** a parallel batch with one `Allow` call and one `AskUser` call whose reviewer never answers (use a reviewer that awaits a token you never fire) → assert the `Allow` call still completes (is NOT blocked by the pending escalation), then cancel to unwind; (d) **shared-reviewer serialization (P3):** two engines sharing one `DeferredAskUserReviewer`, both escalate → a recording reviewer asserts its critical section is entered serially (no overlap). Reuse `tests/permission_parallel_batch.rs` patterns.
- **Verify F7:** confirm the refactor preserves agemo's interactive behavior (Task 9) and that the new default (`DenyReviewer`) is the documented change.
- **Commit:** `feat!(loop): resolve AskUser via Reviewer; serialize concurrent escalations`.

**Phase 2 acceptance:** `cargo build/test --all-features` green in loop; existing permission/parallel-batch/ask_user tests pass (adjusted for the new default where they relied on stall behavior).

---

## Phase 3 — subagent: child inheritance

Grounding: `motosan-agent-subagent/src/subagent/config.rs` — `SubagentConfig { catalog, factory, ..., parent_session_id, ... }`; `ChildEngineFactory = Arc<dyn Fn(ChildSpec) -> Result<(Engine, Arc<dyn LlmClient>), _>>`; `ChildSpec` carries no policy/reviewer.

### Task 8: `SubagentConfig::inherit_approval_from`

**Files:** Modify `src/subagent/config.rs` (and `spec.rs` if the factory wrapper lives there).

- Add a builder method that wraps the user's `factory` so each child `Engine` is built with `.permission_policy(parent_policy.clone())` and `.reviewer(parent_reviewer.clone())` unless the child's own factory already set them. Signature sketch: `pub fn inherit_approval_from(self, policy: Arc<dyn PermissionPolicy>, reviewer: Arc<dyn Reviewer>) -> Self`.
- The shared `reviewer` is the parent session's reviewer (e.g. the parent's `DeferredAskUserReviewer`), so a child's `AskUser` routes to the **same** answering channel/ops — closing the gap. `ApprovalRequest.session_id` carries the child's id so the UI can label which agent asks.
- **Guardian guard (spec §4 F4):** document that `inherit_approval_from` must NOT be used when building a guardian's own engine (give it a non-escalating reviewer instead).
- **Tests:** a parent spawns a child whose policy returns `AskUser`; assert the parent-provided recording reviewer receives an `ApprovalRequest` with the child's `session_id`; assert no deadlock; child proceeds/denies per the verdict. Use `MockChildEngineFactory` / `MockLlmClient` from `src/testing/`.
- **Commit:** `feat(subagent): inherit_approval_from — child AskUser routes to parent reviewer`.

**Phase 3 acceptance:** `cargo test --workspace --all-features` green in subagent (note: subagent is a single crate, but use `--all-features`); new multi-agent approval test passes.

---

## Phase 4 — agemo: migrate the bridge

Grounding: `agemo/src/main.rs` already bridges the root engine's `AskUser` event to stdin and answers via the ops channel (`AgentOp::AskUserAnswer`). It builds the engine at `Engine::builder()...session_id(sid)`.

### Task 9: wire `DeferredAskUserReviewer`

**Files:** Modify `agemo/src/main.rs`.

- Construct a `DeferredAskUserReviewer` (or agemo's own `Reviewer` impl wrapping its existing stdin emit/read) and pass it via `.reviewer(...)` on the builder, so agemo keeps interactive approval after the loop default became `DenyReviewer`.
- Confirm the existing finance smoke test (`AGEMO_STUB_PROVIDER=1 cargo run -- --harness finance --prompt "..."`) still completes and the audit `session_id` correlation still holds.
- **Test/verify:** run the smoke test; assert an `AskUser`-triggering tool still prompts and resolves.
- **Commit:** `feat(agemo): use DeferredAskUserReviewer for interactive approval`.

**Phase 4 acceptance:** agemo builds/tests green; finance demo behavior unchanged for the root agent.

---

## Cross-cutting acceptance (after all phases)

- Full chain `cargo build/test --all-features` (harness with `--workspace`) green across primitives, loop, subagent, agemo.
- pi-parity check: a policy returning only `Allow`/`Deny` never constructs an `ApprovalRequest` (add a loop test with a recording reviewer asserting `review` is never called).
- CHANGELOGs updated; primitives 0.4.0; loop/subagent/agemo bumped per cascade.
- Spec §3 sketch reconciled with the implemented `ReviewDecision::Deny { reason }` shape.

## Self-review notes (gaps flagged during planning)

- **Spec reconciled (P6):** spec §3 now uses `ReviewDecision::Deny { reason }` (richer, matches `Permission::Deny`), aligned with this plan. No remaining drift.
- **Default choice:** Task 6 resolves the spec's slight ambiguity — default is `DenyReviewer` (fail-safe), and hosts opt into `DeferredAskUserReviewer`. Documented as a behavior change vs today's stall (spec F7).
- **Moving target:** Phases 2–4 reference current `engine.rs` line numbers/handles; re-verify against the then-current code at pickup (loop is in active flux).
