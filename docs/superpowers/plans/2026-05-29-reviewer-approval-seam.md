# Reviewer / Approval Seam — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a swappable, async, session-scoped `Reviewer` seam that resolves `Permission::AskUser` escalations — spanning pi-minimal (never escalate) to Codex-heavy (guardian agent, multi-agent child approval) on one contract.

**Architecture:** Keep `PermissionPolicy` as the decision layer (`Allow | Deny | AskUser`, composed most-restrictive-wins). Add a new `Reviewer` trait in primitives that resolves an `AskUser` into `Approve | Deny { reason }`. In the loop, resolve `AskUser` through the session's reviewer — **chosen architecture is spec §9a: the reviewer owns its I/O and `review()` is an `await` inside each tool call's per-call future.** ⚠️ The sizing spike (spec §9d, 2026-05-30) found this is a **LARGE execution-pipeline refactor** (today's engine is two-phase — a slot pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)` — not per-call; non-blocking already exists there, so §9a re-achieves it through per-call futures to fully decouple the reviewer). §9a is chosen **knowing it is LARGE**, for the cleaner end state. It replaces the loop's current event/op approval path (existing approval tests rewritten — R1). The cheaper engine-mediated defer/resume alternative was considered and rejected (spec §9c). The interactive human reviewer lives with the host (agemo); the loop ships only a fail-safe `DenyReviewer` default. Children share one reviewer (Phase 3) to close the child-`AskUser` gap.

**Tech Stack:** Rust, `async-trait`, `tokio` / `tokio-util` (`CancellationToken`), `serde`. Crates: `motosan-agent-primitives` (0.3.0), `motosan-agent-loop`, `motosan-agent-subagent`, `agemo`.

**Source spec:** [docs/superpowers/specs/2026-05-29-reviewer-approval-seam-design.md](../specs/2026-05-29-reviewer-approval-seam-design.md)

---

## Scope & sequencing

This feature spans four crates with a strict dependency order. It is **one coherent feature**, not independent subsystems, so it is one plan with four phases. Each phase is independently committable and testable.

- **Phase 1 — primitives:** ✅ **DONE (2026-05-29, primitives 0.4.0).** New `Reviewer` trait + `ApprovalRequest` + `ReviewDecision`. Purely additive (no existing item changes). Fully detailed below.
- **Phase 2 — loop:** §9a spike (size the refactor + migration surface), `DenyReviewer`, `EngineBuilder::reviewer()`, resolve `AskUser` through the reviewer (§9a: review inside the per-call future → non-blocking via `join`), and rewrite the displaced event/op approval tests (R1). Interactive reviewer NOT here (host-owned, Phase 4).
- **Phase 3 — subagent:** `SubagentConfig::inherit_approval_from` sugar so a child's `AskUser` routes to the parent session's reviewer/ops channel (the actual gap).
- **Phase 4 — agemo:** agemo provides its own host-owned `Reviewer` (owns stdin I/O) and wires it via `.reviewer(..)`, restoring interactive approval after the loop default became `DenyReviewer`.

> **Granularity calibration (deliberate, not a placeholder):** Phase 1 is fine-grained TDD. Phases 2–4 are written at task level (exact files, signatures, code sketches, test intent) because they touch `motosan-agent-loop` internals that are in active flux (primitives just moved 0.2.0→0.3.0 during design) and the spec gates them **post-M11**. When a phase is actually picked up, re-read the then-current `engine.rs` / `permission_runtime.rs` and expand its tasks into TDD micro-steps against the real code.

**Out of scope for this plan (P5):** `GuardianReviewer` and composite/escalating reviewers are **user-land impls** (a host or vertical writes them against the `Reviewer` trait — spec §4 Tier 3 shows them as usage examples, not framework deliverables). This plan ships only the framework pieces: the trait + types (Phase 1), the `DenyReviewer` default + reviewer wiring (Phase 2), inheritance sugar (Phase 3), and agemo's host-owned reviewer (Phase 4). If a built-in guardian is later wanted, it is a separate plan. The guardian-recursion guard (spec §4 F4) is still documented in Task 8 because the inheritance sugar must not be misused when someone *does* build a guardian.

**Do not start before M11** (rental harness → 1.0 freeze), per the spec §14.

---

## File structure

| File | Phase | Responsibility |
|---|---|---|
| `motosan-agent-primitives/src/approval.rs` (create) | 1 | `Reviewer` trait, `ApprovalRequest`, `ReviewDecision` |
| `motosan-agent-primitives/src/lib.rs` (modify) | 1 | `pub mod approval;` + re-exports |
| `motosan-agent-primitives/CHANGELOG.md` (modify) | 1 | 0.4.0 entry (additive) |
| `motosan-agent-loop/src/core/reviewer.rs` (create) | 2 | `DenyReviewer` (default-when-none). Interactive reviewer is host-owned (§9a), not here. |
| `motosan-agent-loop/src/core/engine.rs` (modify) | 2 | `EngineBuilder::reviewer()`; route `AskUser` → reviewer; per-session approval serialization |
| `motosan-agent-loop/src/core/permission_runtime.rs` (modify) | 2 | helper to build an `ApprovalRequest` from the consult inputs |
| `motosan-agent-subagent/src/subagent/config.rs` (modify) | 3 | `inherit_approval_from` |
| `agemo/src/main.rs` (modify) | 4 | construct/pass agemo's own host-owned `Reviewer` (owns stdin I/O) |

---

## Phase 1 — primitives: the `Reviewer` contract

> **✅ COMPLETE — 2026-05-29.** Landed as **primitives 0.4.0**, pushed to origin/main. Strictly additive (4 files: `src/approval.rs` +168, `src/lib.rs` +2, CHANGELOG, Cargo.toml — no existing item touched). Verified green: 46 unit + 8 fixtures + 10 doctests (doctests link via the home-config `rustdocflags` fix). Review passed with no findings.
> - [x] **Task 1** — `ReviewDecision { Approve, Deny { reason } }` (`a2f208f`)
> - [x] **Task 2** — owned, cancellable `ApprovalRequest` (`58a9825`)
> - [x] **Task 3** — `Reviewer` trait (by-value, object-safe) (`bde3187`)
> - [x] **Task 4** — doctest + version bump to 0.4.0 (`1ec8ca3`)
>
> Deviations from the as-written tasks (intentional, all applied): P1 compile-time `Send+'static` assertion instead of `tokio::spawn`; P2 clean re-export (no fail-then-narrow dance); the doctest additionally covers the cancellation fail-closed path.

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

## Phase 2 — loop: reviewer wiring (§9a / A+B, decided)

> **Architecture (decided):** **§9a (A+B)** per spec §9 — the reviewer owns its I/O and `review()` is an `await` inside each tool call's per-call future. **⚠️ The §9d sizing spike (2026-05-30) graded this LARGE** — today's engine is two-phase (slot pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)`), so §9a means rebuilding the execution pipeline across ~8 dispatch call sites + the streaming-eager path. Non-blocking already exists in that join; §9a re-achieves it via per-call futures to fully decouple the reviewer. **Chosen knowing it is LARGE.** The engine-mediated defer/resume alternative is **rejected** (spec §9c). §9a **replaces** the loop's current `ExtensionEvent::AskUser`/`AgentOp::AskUserAnswer` approval path *for permission only* (the `ask_user` extension keeps it, F6); existing permission-approval tests are rewritten and agemo's bridge moves into a host-owned reviewer (R1). Interactive reviewer is host-owned (Phase 4), not the loop — the loop ships only `DenyReviewer`. Approval **timeout** → reviewer (R2). Preserve event ordering (R4). **The §9d spike's restructure surface IS this phase's task breakdown.**

Grounding (RE-VERIFY at pickup — loop is in active flux; line numbers may have drifted):
- `src/core/engine.rs`: the `consult_policy(...)` match — `Allow` (emit ToolStarted + dispatch), `Deny { reason }` (resolved error slot), `AskUser { prompt }` (today: emit `ExtensionEvent::AskUser` + `deferred_calls` + resolve via `AgentOp::AskUserAnswer`). Find how per-call futures are composed and joined (`execute_tools_parallel` / `resolve_deferred_slots` / `join!`).
- `EngineBuilder` setters (`permission_policy`, `session_id`, …) and `Engine`/`build()`.
- `src/core/permission_runtime.rs`: `consult_policy(...) -> Permission`; `default_prompt(name, args)`.

### Task 5: SPIKE — size the §9a refactor ✅ DONE (2026-05-30, loop@main)

**Verdict: LARGE** (not structurally impossible). Full result recorded in **spec §9d** — read it before Task 6/7. Headlines:
- Today's engine is **two-phase** (slot pre-pass + `join!(resolve_deferred_slots, execute_tools_parallel)`); permission is NOT inside a per-call future. Non-blocking already exists in that join.
- §9a's per-call fold touches: engine/builder, `consult_policy`, and the whole execution pipeline (`dispatch_tool_call_to_slot` ×~8 sites, `execute_tools_with_policy`, `resolve_and_execute_intercepted_slots`, `execute_tools_parallel`, the streaming-eager `resolve_and_combine_preexecuted_slots`), plus removing the permission-specific `InterceptedSlot::DeferredPermission` / `deferred_calls` / `approval_from_answer`.
- **Hard constraints:** the `ask_user` *extension* reuses the same `ExtensionEvent::AskUser` + `AgentOp::AskUserAnswer` — those STAY (F6); only the *permission* use is removed. The streaming-eager path uses `tokio::spawn` (`src/streaming_executor.rs`) — §9a must handle it (the pure-`join` story doesn't cover it).
- Migration list + agemo bridge: see spec §9d.

### Task 6: `DenyReviewer` + `EngineBuilder::reviewer()` ✅ DONE (loop@`ee9fec4`)

Landed and reviewed clean: additive only (the AskUser/pipeline path untouched, all existing approval tests pass unchanged); `DenyReviewer` + `EngineBuilder::reviewer()` setter/field/default/accessor; dormant until Task 7.

**Files:** Create `motosan-agent-loop/src/core/reviewer.rs` (`mod reviewer;` in `src/core/mod.rs`); modify `engine.rs`.

- Implement `DenyReviewer` → `ReviewDecision::Deny { reason: "no reviewer configured".into() }`. No shared state, no lock.
- Add `reviewer: Option<Arc<dyn Reviewer>>` to `EngineBuilder` + `Engine`; setter `pub fn reviewer(mut self, r: Arc<dyn Reviewer>) -> Self` next to `permission_policy`; `build()` defaults to `Arc::new(DenyReviewer)`; add a `reviewer()` accessor.
- **Do NOT build an interactive reviewer here** — under §9a that reviewer owns stdin/UI I/O and belongs to the host (agemo, Phase 4). The loop ships only `DenyReviewer`.
- **Tests:** `DenyReviewer` returns `Deny`; builder stores the reviewer; unset → `DenyReviewer`.
- **Commit:** `feat(loop): DenyReviewer + EngineBuilder::reviewer() (default DenyReviewer)`.

### Task 7: resolve `AskUser` through the reviewer — ⚠️ v1 BOTCHED & REVERTED; redo design-first

**v1 attempt (2026-05-30) failed:** it patched the shared two-phase pipeline incrementally; each blocking-point fix exposed another entangled wait (permission review vs `ask_user` extension defer vs streaming chunks vs sibling resume). Tests went green but with latent concurrency bugs ("green but wrong"). It was **reverted** — loop `main` is back at clean Task 6 (`ee9fec4`); the dead-end is preserved at branch `wip/task7-v1-botched`.

**Redo is DESIGN-FIRST and specified separately:** see **[2026-05-30-task7-v2-execution-model.md](2026-05-30-task7-v2-execution-model.md)**. The lesson: a LARGE concurrency rebuild must (1) draw the per-call execution model, (2) get it reviewed, (3) land the *structural* rewrite with zero behavior change and the full suite green, and only THEN add the reviewer — never incrementally patch the shared batch resolver.

The v2 doc carries the full plan; in summary it: makes each tool call **one self-contained future** (permission check/review → interceptor dispatch incl. ask_user defer → execute), has the batch only `join_all` them, replaces the batch-level `resolve_deferred_slots` with an **ops router** that wakes the specific waiting call, unifies the streaming path onto the same future, keeps the `ask_user` extension on `AgentOp::AskUserAnswer` (only the permission use is removed, F6), and makes `architectural_invariants.rs` symbolic. Hard constraints, migration list (R1), and the P3/P4/event-ordering tests are in that doc.

**Phase 2 acceptance:** `cargo build/test --all-features` green in loop; the §9a scoping note + migration list (Task 5) recorded; the displaced permission-approval tests rewritten to drive a `Reviewer` (R1); remaining permission/parallel-batch/`ask_user`-extension tests pass; event ordering unchanged (R4).

---

## Phase 3 — subagent: child inheritance

Grounding: `motosan-agent-subagent/src/subagent/config.rs` — `SubagentConfig { catalog, factory, ..., parent_session_id, ... }`; `ChildEngineFactory = Arc<dyn Fn(ChildSpec) -> Result<(Engine, Arc<dyn LlmClient>), _>>`; `ChildSpec` carries no policy/reviewer.

### Task 8: `SubagentConfig::inherit_approval_from`

**Files:** Modify `src/subagent/config.rs` (and `spec.rs` if the factory wrapper lives there).

- Add a builder method that wraps the user's `factory` so each child `Engine` is built with `.permission_policy(parent_policy.clone())` and `.reviewer(parent_reviewer.clone())` unless the child's own factory already set them. Signature sketch: `pub fn inherit_approval_from(self, policy: Arc<dyn PermissionPolicy>, reviewer: Arc<dyn Reviewer>) -> Self`.
- The shared `reviewer` is the parent session's reviewer (e.g. agemo's host-owned reviewer), so a child's `AskUser` routes to the **same** answerer — closing the gap. `ApprovalRequest.session_id` carries the child's id so the UI can label which agent asks.
- **Guardian guard (spec §4 F4):** document that `inherit_approval_from` must NOT be used when building a guardian's own engine (give it a non-escalating reviewer instead).
- **Tests:** a parent spawns a child whose policy returns `AskUser`; assert the parent-provided recording reviewer receives an `ApprovalRequest` with the child's `session_id`; assert no deadlock; child proceeds/denies per the verdict. Use `MockChildEngineFactory` / `MockLlmClient` from `src/testing/`.
- **Commit:** `feat(subagent): inherit_approval_from — child AskUser routes to parent reviewer`.

**Phase 3 acceptance:** `cargo test --workspace --all-features` green in subagent (note: subagent is a single crate, but use `--all-features`); new multi-agent approval test passes.

---

## Phase 4 — agemo: migrate the bridge

Grounding: `agemo/src/main.rs` already bridges the root engine's `AskUser` event to stdin and answers via the ops channel (`AgentOp::AskUserAnswer`). It builds the engine at `Engine::builder()...session_id(sid)`.

### Task 9: agemo's host-owned reviewer (§9a)

**Files:** Modify `agemo/src/main.rs`.

- Implement an agemo-local `Reviewer` (e.g. `StdinReviewer`) whose `review(req)` does what agemo's bridge does today — prompt the user (its own stdout/stdin or its existing wire-event emit + answer read) and map the answer to `Approve`/`Deny { reason }`, racing `req.cancellation_token`. It **owns its I/O** (does not depend on engine internals). Pass it via `.reviewer(Arc::new(...))` on the builder, restoring interactive approval after the loop default became `DenyReviewer`.
- Move agemo's current `permission_timeout_secs` into this reviewer (R2): `review()` races a timeout against the answer and returns `Deny` on expiry.
- Confirm the existing finance smoke test (`AGEMO_STUB_PROVIDER=1 cargo run -- --harness finance --prompt "..."`) still completes and the audit `session_id` correlation still holds.
- **Test/verify:** run the smoke test; assert an `AskUser`-triggering tool still prompts and resolves.
- **Commit:** `feat(agemo): host-owned Reviewer for interactive approval`.

**Phase 4 acceptance:** agemo builds/tests green; finance demo behavior unchanged for the root agent.

---

## Cross-cutting acceptance (after all phases)

- Full chain `cargo build/test --all-features` (harness with `--workspace`) green across primitives, loop, subagent, agemo.
- pi-parity check: a policy returning only `Allow`/`Deny` never constructs an `ApprovalRequest` (add a loop test with a recording reviewer asserting `review` is never called).
- CHANGELOGs updated; primitives 0.4.0; loop/subagent/agemo bumped per cascade.
- Spec §3 sketch reconciled with the implemented `ReviewDecision::Deny { reason }` shape.

## Self-review notes (gaps flagged during planning)

- **Spec reconciled (P6):** spec §3 now uses `ReviewDecision::Deny { reason }` (richer, matches `Permission::Deny`), aligned with this plan. No remaining drift.
- **Default choice:** default is `DenyReviewer` (fail-safe); hosts opt into their own reviewer (e.g. agemo's host-owned stdin reviewer, §9a). Documented as a behavior change vs today's stall (spec F7).
- **Integration route (decided):** §9a — review inside the per-call future (spec §9a). The defer/resume alternative is rejected (§9c); Phase 2's spike only *scopes* §9a + its migration. §9a replaces the event/op approval path (R1: rewrite those tests), moves approval timeout into the reviewer (R2), and must preserve event ordering (R4).
- **Moving target:** Phases 2–4 reference current `engine.rs` line numbers/handles; re-verify against the then-current code at pickup (loop is in active flux).
