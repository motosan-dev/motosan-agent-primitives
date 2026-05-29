# Reviewer / Approval Seam — Design Spec

**Date:** 2026-05-29
**Status:** Draft — awaiting user review
**Related:** [CODEX_ARCHITECTURE_STUDY.md](../../CODEX_ARCHITECTURE_STUDY.md), [IMPLEMENTATION_PLAN.md](../../../IMPLEMENTATION_PLAN.md) §M11
**Scope position:** Design input for **post-M11 / post-1.0**. NOT a directive to implement before the rental harness validates the API and freezes 1.0. Captured now while the comparison context (Codex, pi) is fresh.

---

## 1. Problem

Motosan can already gate tool calls at the **decision** layer:

- `Hook::pre_tool_use` can `Skip` / `Abort` a call.
- `PermissionPolicy::check(ctx) -> Allow | Deny | AskUser`, composed most-restrictive-wins across stacked harnesses.

The loop **already has an answering mechanism** for the root engine (verified in `loop/src/core/engine.rs`, the `consult_policy` match): on `Permission::AskUser` it emits `AgentEvent::LoopInterceptor(ExtensionEvent::AskUser{..})`, registers the tool call in `deferred_calls` (`by_extension: "permission"`), and resolves it when an `AgentOp::AskUserAnswer` arrives (the AskUser-interceptor Defer/ResumeDeferred protocol). **This is already an SQ/EQ-style channel** — agemo is just one consumer of it (bridging the event to stdin and the answer back to the ops channel).

The confirmed gap is **not** "no channel" — it is that this channel is **(a) hard-wired, not swappable** (you cannot drop in a guardian-agent reviewer in place of the human prompt), and **(b) not reachable from a child engine**: `ChildSpec` carries no policy, and a sub-agent's `AskUser` event / answer op are not routed back to the parent session's deferred/ops channel. A child that returns `AskUser` today emits the event into the void and **stalls** (the deferred call never resolves) until timeout/cancellation.

Two reference points bracket the design space:

- **pi** (`earendil-works/pi`): a single async `beforeToolCall(ctx) -> { block?, reason? }`. Human approval is just an extension calling `confirm(): Promise<boolean>` *inside* the hook. No `AskUser` trichotomy, no separate reviewer, no sub-agents. Minimal.
- **Codex** (`openai/codex`): a decoupled SQ/EQ approval channel (`request_command_approval` → `oneshot` → `Op::ExecApproval`), a swappable **guardian** reviewer that is itself a sub-agent, and multi-agent where children **inherit** the parent's approval policy + sandbox. Heavy.

## 2. Goal

One framework that spans **both ends** without forcing the heavy machinery on the simple case:

- **pi end:** a user who never escalates pays nothing new — existing `pre_tool_use` / `Allow|Deny` policy is the whole story.
- **Codex end:** a user can plug in interactive human review, a guardian-agent reviewer, escalation pipelines, and sub-agent approval that flows back to one central answering point.
- Same contract across the spectrum; going remote later swaps an implementation, not the contract.

### Non-goals (YAGNI)

- No execpolicy-style auto-safe allowlist as a third axis (Motosan has `ToolAnnotations` to build one later if a real need appears).
- No decoupled SQ/EQ wire protocol now (only needed for a remote/IPC driver; see §9).
- No sandbox-per-engine inheritance (Motosan does not wire a per-engine sandbox into the approval path today).
- No change to the `PermissionPolicy` trait or its composition semantics.
- No "approve-with-amendment" (editing tool args before approving, like Codex's execpolicy/network amendments) (F8). `ReviewDecision` is `Approve | Deny` in v1; arg rewriting stays the job of `Hook::pre_tool_use`. Revisit only if a vertical needs it.

## 3. Core design: two layers + one async seam

Keep the **decision** layer; add a **resolution** layer under `AskUser`.

```rust
// Layer 1 — DECISION (already exists, unchanged)
PermissionPolicy::check(ctx) -> Allow | Deny | AskUser     // composable, most-restrictive-wins

// Layer 2 — RESOLUTION (new): resolves an escalation into a final verdict
#[async_trait]
pub trait Reviewer: Send + Sync {
    // BY VALUE (owned) — lets a reviewer move/queue/defer the request (F3)
    async fn review(&self, req: ApprovalRequest) -> ReviewDecision;
}

#[derive(Clone)]   // composites that fan out to >1 reviewer clone it
pub struct ApprovalRequest {
    pub tool_call: ToolCall,              // OWNED — see "owned, not borrowed" below
    pub annotations: ToolAnnotations,
    pub session_id: String,
    pub recent_messages: Vec<Message>,    // owned snapshot of the engine's window (D-M10-3)
    pub prompt: Option<String>,           // sourced directly from `Permission::AskUser { prompt }`
    pub cancellation_token: CancellationToken,  // review() MUST observe this
}

pub enum ReviewDecision { Approve, Deny }
```

**Rule:** when the composed policy returns `AskUser`, the loop/manager calls `session.reviewer.review(req).await` for the final verdict. When it returns `Allow`/`Deny`, the reviewer is never consulted.

**Default reviewer = `DenyReviewer`** (fail-safe): if a policy escalates but no reviewer is wired, the call is denied rather than silently allowed.

**Owned, not borrowed (F3).** Unlike `PermissionContext<'a>` (which is consumed synchronously inside `check`), an `ApprovalRequest` may be **retained**: a reviewer can queue it, move it to another task/thread, or hold it across a long guardian turn (cf. Codex's `spawn_approval_request_review`). Borrowed `&ToolCall` / `&[Message]` would forbid that and force the caller to keep everything alive for the whole `review()` duration. So the request owns its data (one clone of the tool call + the window snapshot per escalation — escalations are rare, so the cost is negligible). This matches Codex's deliberately-owned `request_command_approval` arguments. Consequently `review()` takes the request **by value**, and `ApprovalRequest` derives `Clone` so a fan-out composite reviewer can hand a copy to each child reviewer (an escalating composite that tries one then another needs no clone — it borrows, then moves).

**Cancellation (F1).** `review()` can block for a long time (human away, guardian slow). `ApprovalRequest` therefore carries a `CancellationToken` — consistent with every `*Ctx` struct in primitives — and a well-behaved reviewer races its wait against it, returning `Deny` (or a dedicated cancelled path) when the turn is cancelled. Mirrors Codex's `review_approval_request_with_cancel`.

**`prompt` source (F5).** `Permission::AskUser` already carries `prompt: Option<String>` today. `ApprovalRequest.prompt` is that value passed straight through — no new plumbing on the policy side.

**Naming caution (F6).** Do not conflate two similarly-named concepts: `Permission::AskUser` (this approval seam — "may this tool run?") and loop's existing `extensions/ask_user` (a tool-side capability for an agent to *ask the user a question* mid-turn, surfaced as `AskUserEvent` and bridged by agemo). They are unrelated. The new types are named around **approval / `Reviewer`** precisely to keep that distinction; implementers must not route one through the other.

### Why `Reviewer` is separate from `PermissionPolicy`

`PermissionPolicy` is **composed** (N stacked policies, most-restrictive-wins). You do not want N policies each blocking on human input. You want: many policies vote → at most one `AskUser` → **exactly one** session-level reviewer resolves it. So `Reviewer` is inherently *singular, session-scoped, swappable*, whereas policy is *stackable decision logic*. Keeping them separate is precisely what lets the pi-simple case (no reviewer) and the Codex-heavy case (rich reviewer) coexist without bloating either.

### Why an async trait is enough (the pi lesson)

`review()` is `async`, so "get an answer" is just an `await` inside the impl — pi's `confirm(): Promise<bool>` equivalent. The trait does **not dictate** how the answer arrives: a **guardian** reviewer needs no channel at all (it awaits a sub-agent turn in-process); the **human** reviewer awaits the loop's existing event/defer/ops round-trip (§9); a **remote** reviewer (future) awaits the same round-trip over the wire. One async trait accommodates all three without changing — that is the whole point of making it the seam.

## 4. Usage across the spectrum

### Tier 1 — pi-minimal (zero new code)

```rust
let engine = Engine::builder()
    .tools(harness.tools())
    .permission_policy(policy)   // returns only Allow/Deny
    .build();                    // reviewer never invoked
```

Or no policy at all — use `Hook::pre_tool_use` returning `Abort`. Identical to pi's `beforeToolCall → { block }`.

### Tier 2 — interactive human review (the common case; generalizes agemo)

```rust
struct StdinReviewer { /* channel to the host UI */ }
#[async_trait]
impl Reviewer for StdinReviewer {
    async fn review(&self, req: ApprovalRequest) -> ReviewDecision {
        emit_wire_event(AskUser { tool: req.tool_call.name.clone(), .. });
        match read_answer().await {           // pi's confirm() equivalent — one await
            Answer::Yes => ReviewDecision::Approve,
            _           => ReviewDecision::Deny,
        }
    }
}

let engine = Engine::builder()
    .permission_policy(policy)               // e.g. FinanceApprovalPolicy → AskUser on place_order
    .reviewer(Arc::new(StdinReviewer::new(..)))   // the one new line
    .session_id(sid)
    .build();
```

### Tier 3 — guardian (Codex-style: a reviewer agent decides)

Swap the impl; the call site is unchanged.

```rust
struct GuardianReviewer { guardian: Arc<Engine>, llm: Arc<dyn LlmClient> }
#[async_trait]
impl Reviewer for GuardianReviewer {
    async fn review(&self, req: ApprovalRequest) -> ReviewDecision {
        let verdict = self.guardian.run_once(prompt_from(&req), &self.llm).await;
        if verdict.approved { ReviewDecision::Approve } else { ReviewDecision::Deny }
    }
}
```

Escalation pipeline (auto first, human on uncertainty) is just a composite `Reviewer` that wraps two others.

**Guardian recursion guard (F4).** A `GuardianReviewer` runs another `Engine`. That guardian engine MUST be built with a **non-escalating reviewer** (`AutoReviewer`/`DenyReviewer`) so its own tool calls never re-enter the human/guardian path — otherwise an approval review could recurse or deadlock. (Codex enforces this by tagging the guardian session as a distinct `SubAgent` source and gating `routes_approval_to_guardian`.) The `inherit_approval_from` sugar in §4/§8 must therefore **not** be used when constructing a guardian's own engine.

### Tier 4 — multi-agent (closes the gap)

The sub-agent factory inherits the parent policy and shares the central reviewer:

```rust
let central: Arc<dyn Reviewer> = Arc::new(StdinReviewer::new(..));   // one per session

let factory: ChildEngineFactory = {
    let central = central.clone();
    let parent_policy = parent_policy.clone();
    Arc::new(move |spec| {
        let engine = Engine::builder()
            .tools(child_harness.tools())
            .permission_policy(parent_policy.clone())   // inherit parent policy (default)
            .reviewer(central.clone())                  // child AskUser routes to the same answerer
            .session_id(spec.child_session_id())        // distinct → reviewer/UI can label "child X asks"
            .build();
        Ok((engine, llm.clone()))
    })
};
```

A child finance agent's `place_order` → `AskUser` → its reviewer is the central one → answered in one place (human or guardian). `req.session_id` differs, so the UI can show *which* agent is asking.

A `manager`-level convenience (sugar) is expected so this is the default, not hand-wired each time:

```rust
SubagentConfig::new(factory).inherit_approval_from(&parent_engine)  // policy + central reviewer
```

## 5. Harness vs host split

- **Harness brings the policy** — the *domain* decision of whether to escalate (`FinanceApprovalPolicy` knows `place_order` needs approval). Stays on the `Harness` trait (`permission_policy()`), unchanged.
- **Host brings the reviewer** — *how* to answer in this environment (stdin? a UI? a guardian agent?). Wired on `EngineBuilder::reviewer(..)`, not on `Harness`.

This separation is natural: "should we ask?" is domain knowledge; "ask whom, and how?" is runtime/environment. Therefore `Reviewer` does **not** go on the `Harness` trait.

## 6. Where the code lives

| Piece | Crate | Note |
|---|---|---|
| `Reviewer` trait, `ApprovalRequest`, `ReviewDecision` | **`motosan-agent-primitives`** | Contract types, same module/role as `PermissionPolicy`. Small, additive. |
| `EngineBuilder::reviewer(..)`; refactor the existing `Permission::AskUser` handling (emit event → `deferred_calls` → `AgentOp::AskUserAnswer`) to sit **behind** a `Reviewer` | `motosan-agent-loop` | Not a new channel — the existing machinery in `engine.rs`'s `consult_policy` match becomes the default reviewer impl. |
| `DenyReviewer` (default-when-none), `DeferredAskUserReviewer` (wraps today's event/defer/ops behavior), `GuardianReviewer`, composite | `motosan-agent-loop` (and host crates like agemo) | NOT in primitives. |
| Child policy inheritance + shared-reviewer sugar | `motosan-agent-subagent` | `SubagentConfig::inherit_approval_from`. |

Rationale for primitives placement: a `Reviewer` is referenced by loop, subagent, and potentially harness consumers — the same cross-cutting contract role `PermissionPolicy` already plays there. A dedicated `motosan-agent-approval` crate is premature for a single concept.

## 7. Composition & ordering semantics

- **Policies** compose exactly as today (most-restrictive-wins; any `Deny` → Deny, else any `AskUser` → AskUser, else Allow). Unchanged.
- **Exactly one `Reviewer` per engine/session.** It is only consulted when the composed result is `AskUser`. Its `Approve` maps to proceeding; `Deny` maps to the same blocked path a policy `Deny` takes (the model receives an error tool result).
- A `Reviewer` cannot *loosen* a policy `Deny` — it only resolves `AskUser`. There is no override of an outright `Deny`, mirroring the no-escape-hatch rule of policy composition.
- **Concurrent escalations serialize (F2).** Motosan runs tool calls in parallel batches (`tests/permission_parallel_batch.rs`). If several calls in one batch each resolve to `AskUser`, the engine MUST funnel their `review()` calls through a single serialization point (one at a time per session) rather than invoking the reviewer concurrently — a human cannot answer N prompts at once, and a guardian agent should see them in order. Implementation: a per-session approval mutex/queue (the lightweight analogue of Codex's keyed pending-approval map). A future `review_batch(&[ApprovalRequest])` may be added if a reviewer wants to present them together; out of scope for v1.

## 8. Multi-agent inheritance defaults (resolved decision)

- A spawned child **inherits the parent's `PermissionPolicy`** by default; it may stack its own (composite, most-restrictive-wins).
- A child **shares the parent session's `Reviewer`** by default (routes escalations to one answerer).
- Both are **opt-out** for a fully isolated child.
- **Sandbox is explicitly out of scope** — Motosan has no per-engine sandbox in the approval path; revisit only if a vertical needs it.

## 9. Relationship to the existing event/defer/ops channel (and remote)

The "emit `AskUser` event → defer the call → resolve on `AgentOp::AskUserAnswer`" machinery described in §1 **already exists in the loop**. This spec does **not** build a new channel — it puts that machinery **behind the `Reviewer` trait** so it becomes swappable and child-reachable:

- The **default interactive reviewer** *is* today's behavior (emit the AskUser event, defer, await the answer op), refactored to live behind `Reviewer`. Root-engine behavior is preserved.
- A **guardian** reviewer is a different impl of the same trait — no event/defer needed; it just awaits a sub-agent turn.
- **Remote / IPC** (if Motosan grows an app-server driver) is *also* already this shape: the event goes over the wire instead of in-process, the answer op comes back over the wire. The `Reviewer` trait and all callers/policies are untouched — only the event transport changes.

This is why the seam is an async trait: it abstracts an answering mechanism the loop already implements in one (in-process) form today, and leaves room for the remote form later without a contract change.

## 10. Testing strategy

- **Decision unchanged:** existing `PermissionPolicy` composition tests stay green.
- **Reviewer unit:** `DenyReviewer` denies; an `AlwaysApprove` test reviewer approves; a `RecordingReviewer` asserts `ApprovalRequest` carries the right `tool_call` / `session_id` / `recent_messages`.
- **Wiring:** policy `AskUser` + `AlwaysApprove` → tool runs; policy `AskUser` + default (no reviewer) → denied (fail-safe).
- **Multi-agent:** parent spawns child whose policy returns `AskUser`; assert the *central* reviewer receives the request with the *child's* `session_id`; assert no deadlock and the child proceeds/denies per the verdict.
- **pi parity:** policy returning only `Allow`/`Deny` never constructs an `ApprovalRequest` (reviewer untouched).

## 11. Versioning & migration impact

- **primitives:** purely additive — one new trait plus two new types, with **no changes to any existing item** (unlike D-M10-2, which added a field to an existing struct and forced literal constructors to update). This is a non-breaking minor bump; every existing `Hook` / `PermissionPolicy` impl compiles untouched.
- **loop:** new `EngineBuilder::reviewer()` setter (additive); **refactor** the existing `Permission::AskUser` handling in `engine.rs` (event → `deferred_calls` → `AgentOp::AskUserAnswer`) into a `DeferredAskUserReviewer` impl so it is swappable, behind the trait (root behavior preserved); add a per-session approval serialization point (F2). **F7 (resolved):** today an unbridged/unanswered `AskUser` *stalls* (deferred call never resolves until timeout/cancel). So `DenyReviewer` as the **default-when-no-reviewer-is-set** is a deliberate, safer behavior change for the no-reviewer case (e.g. a child that nobody wired) — call it out in the changelog. Hosts that wire `DeferredAskUserReviewer` (or agemo today) keep the current ask-the-human behavior.
- **subagent:** `SubagentConfig::inherit_approval_from` sugar (additive).
- **agemo:** migrate its existing stdin `AskUser` bridge into a `StdinReviewer` impl. This is behavior-preserving *for agemo specifically* (it already answers root-engine `AskUser`); it is independent of the F7 question about the framework-wide default when no reviewer is wired.

## 12. Decisions resolved in this spec

1. **Types live in `motosan-agent-primitives`** (alongside `PermissionPolicy`), not a new crate.
2. **`Reviewer` is separate from `PermissionPolicy`** (singular/session-scoped vs stackable).
3. **Child inherits policy + shares reviewer by default; sandbox deferred; opt-out provided.**
4. **Default reviewer is fail-safe `Deny`.**
5. **No execpolicy allowlist, no SQ/EQ protocol now** (both deferred, both addable without breaking the seam).
6. **`ApprovalRequest` is owned** (retainable/deferrable by a reviewer), **carries a `CancellationToken`**, and its `prompt` passes through from `Permission::AskUser { prompt }` (F1/F3/F5).
7. **Concurrent escalations serialize per session** (F2); no `Approve`-with-amendment in v1 (F8).
8. **A guardian's own engine uses a non-escalating reviewer** (F4).

## 13. Open questions (for review)

- Should `ReviewDecision` carry more than `Approve`/`Deny` (e.g. `ApproveForSession`, a remembered scope like Codex's `ApprovedForSession`)? Leaning **no** for v1 — add later if a vertical wants "don't ask again this session."
- *(F7 — resolved during code review: today an unanswered `AskUser` stalls via the deferred-call protocol; see §1, §9, §11. No longer open.)*

## 14. Sequencing

Do not implement before **M11** (rental harness → freeze 1.0). This is forward design. When a vertical genuinely needs interactive child-agent approval: (1) add the primitives types (additive); (2) refactor the loop's existing `Permission::AskUser` machinery (event → `deferred_calls` → `AgentOp::AskUserAnswer`) into a `DeferredAskUserReviewer` behind the trait, add the `reviewer()` setter and the `DenyReviewer` default, and the per-session serialization point; (3) add the subagent inheritance sugar so a child's `AskUser` routes to the parent session's reviewer/ops channel (the actual gap); (4) agemo keeps working through the `DeferredAskUserReviewer` (behavior-preserving for the root).
