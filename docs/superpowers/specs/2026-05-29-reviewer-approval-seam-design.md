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

pub enum ReviewDecision { Approve, Deny { reason: String } }  // Deny carries a reason (mirrors Permission::Deny)
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
| `EngineBuilder::reviewer(..)`; fold `review()` into the per-call future so `Permission::AskUser` resolves through the reviewer (§9a, chosen); replace the old event/op approval path + migrate its tests | `motosan-agent-loop` | **LARGE refactor** (§9d): rebuild the two-phase pipeline into per-call futures + streaming-eager path. End state has no permission-specific slot/select/spawn, but reaching it is not a local edit. |
| `DenyReviewer` (default-when-none) | `motosan-agent-loop` | NOT in primitives. |
| Interactive reviewer (owns stdin/UI I/O), `GuardianReviewer`, composite, `ChannelReviewer` | host crates (e.g. agemo) / user-land | Under §9a the human reviewer owns its I/O, so it lives with the host, not the engine. |
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

## 9. Engine integration: reviewer-owned I/O (§9a — chosen)

`review()` is wired into the loop via **§9a (A+B)** below. §9c records the rejected alternative for posterity; the trait/types/usage are identical regardless, so this is purely an internal integration choice.

### 9a. The chosen design — "A+B": reviewer owns its I/O, review is an `await` inside the per-call future

- **A — the reviewer owns its I/O.** `review(req)` is a fully self-contained async call. The interactive (human) reviewer is provided by the **host** (e.g. agemo) wired to the host's own input channel; it does **not** go through the engine's `AgentOp::AskUserAnswer` ops channel. The engine never forwards answers, holds no oneshot registry.
- **B — review is an `await` inside each tool call's future.** The loop already runs tool calls as per-call async futures joined concurrently. Fold the review into that future:

```rust
async fn run_one_call(call) -> ToolOutput {
    match policy.check(&call).await {
        Allow            => execute(call).await,
        Deny { reason }  => error(reason),
        AskUser { prompt } => match reviewer.review(req_from(call, prompt)).await {
            Approve         => execute(call).await,
            Deny { reason } => error(reason),
        }
    }
}
// batch = join_all(calls.map(run_one_call))
```

**Why this is the chosen end state:** once tool calls run as per-call futures of the form above, `join` cooperatively multiplexes them on one task — a `review()` that suspends (awaiting a human) returns `Pending`, so the executor advances the sibling calls. The result has no permission-specific deferred-slot variant, no `select!` over ops, no oneshot registry, no answer-op forwarding; the `Reviewer` trait stays truly decoupled (even a host's interactive reviewer owns its own channel, nothing engine-internal leaks in). Cancellation = dropping the joined future (and `review()` races `req.cancellation_token`).

> **⚠️ Cost — this is a LARGE refactor, not a cheap one (spike 2026-05-30, §9d).** The engine today does **not** run per-call futures with the permission decision inside them — it is a two-phase pipeline (a slot pre-pass, then `join!(resolve_deferred_slots, execute_tools_parallel)`). Reaching the shape above means **rebuilding that pipeline** across ~8 dispatch call sites plus the streaming-eager path (which uses `tokio::spawn`). And the non-blocking property is **not new** — the existing resolver/executor `join!` already provides it; §9a re-achieves the same outcome through per-call futures **in order to fully decouple the reviewer**. We accept the LARGE cost deliberately for that cleaner end state; we are not getting non-blocking "for free" (the engine already had it). The cheaper engine-mediated route that reuses today's defer/resume is recorded as the rejected alternative in §9c.

- **P4 (non-blocking)** is satisfied by `join`, not by spawning.
- **P3 (serialization across a shared reviewer)** still lives inside the reviewer (an internal mutex around its critical section), unchanged.

### 9b. Decision: §9a is the chosen architecture

**§9a is the design** — the reviewer owns its I/O; `review()` is an `await` inside the per-call future. Accepted consequences (the costs are taken on purpose, in exchange for the cleaner end state):

- **Replaces the existing approval protocol (R1).** The loop's current `ExtensionEvent::AskUser` → `deferred_calls` → `AgentOp::AskUserAnswer` round-trip is **no longer used for permission approval** — the reviewer handles its own I/O. So existing permission-approval tests that assert that event/op round-trip must be **rewritten** (not merely retuned), and agemo's wire-based approval bridge moves into a host-owned reviewer. This migration is accepted, not avoided.
- **Keep the `ask_user` extension separate (F6).** The `ask_user` *extension* (an agent asking the user a mid-turn question) still uses the event/op machinery; only the *permission* path stops using it. Do not entangle them.
- **Timeout becomes the reviewer's job (R2).** agemo's `permission_timeout_secs` moves into its reviewer, which races a timeout against the answer (and honours `req.cancellation_token`).
- **Non-blocking is preserved, not newly gained (R3).** The engine already keeps allowed siblings running while an approval waits (via `join!(resolve_deferred_slots, execute_tools_parallel)`). §9a re-achieves the same property through per-call futures as a by-product of decoupling the reviewer — it is not a free new benefit. Holds once check→review→execute compose into one joined per-call future (the LARGE refactor in §9d).
- **Verify event ordering (R4).** ToolStarted and friends must fire in the same order as today (Approve → the existing Allow path).

### 9c. Rejected alternative — engine-mediated (defer/resume)

Keeping today's defer/resume and driving `review()` from it (spawn + a `DeferredReview` slot + `select!` + forwarding answer ops to the reviewer) was considered. It is a *smaller diff* and *preserves* the event/op protocol — but it permanently grows concurrency machinery in `resolve_deferred_slots` and leaves the default reviewer engine-coupled. **Rejected** in favour of §9a's cleaner end state. Recorded here so the choice is not re-litigated.

### 9d. Sizing spike — RESULT: LARGE (run 2026-05-30, loop@main)

A read-only spike on `motosan-agent-loop` sized the §9a refactor. **Verdict: LARGE** (not structurally impossible — a real execution-pipeline rebuild). Recorded so the cost is not under-estimated again.

**Why LARGE — today's flow is two-phase, not per-call:** permission is a slot *pre-pass* (`dispatch_tool_call_to_slot` → `consult_policy` → `InterceptedSlot`), and non-blocking comes from `join!(resolve_deferred_slots, execute_tools_parallel)` — *not* from review-inside-a-per-call-future. `AskUser` today emits `ExtensionEvent::AskUser`, inserts into `deferred_calls`, returns `InterceptedSlot::DeferredPermission`, and is resolved later from the shared `ops_rx` via `AgentOp::AskUserAnswer`.

**Restructure surface (the per-call fold touches all of these):**
- Engine/builder: add `reviewer` field + `DenyReviewer` default (`EngineBuilder` ~`engine.rs:424`, `Engine` ~`:857`).
- Permission path: `permission_runtime::consult_policy` (`:19`) + a new owned-`ApprovalRequest` builder.
- Execution pipeline (the bulk): `dispatch_tool_call_to_slot`, `execute_tools_with_policy`, `resolve_and_execute_intercepted_slots`, `execute_tools_parallel`, the streaming-eager path `resolve_and_combine_preexecuted_slots`, and ~8 `dispatch_tool_call_to_slot` call sites.
- Remove the *permission-specific* defer/op machinery (`InterceptedSlot::DeferredPermission`, the `deferred_calls` insert, the permission branch of the `AgentOp::AskUserAnswer` handler, `permission_runtime::approval_from_answer`).

**Hard constraints surfaced (do not break these):**
- The `ask_user` **extension** (agent asks a mid-turn question) **reuses the same `ExtensionEvent::AskUser` event type** and the `AgentOp::AskUserAnswer` op. §9a removes that path for *permission* only — `AgentOp::AskUserAnswer` and the event **must stay** for the ask_user extension + planning/defer protocols (F6). Migrating permission must not entangle them.
- The **streaming-eager executor** uses `tokio::spawn` (`src/streaming_executor.rs`), so the "one task, pure `join`" non-blocking story holds for the ordinary batch but **not** the streaming path — §9a must handle that path explicitly.

**Migration list (rewrite to drive a test `Reviewer`, not feed answer ops):** `tests/permission_gating.rs` (ask_user approve/deny/timeout, ToolStarted ordering), `tests/permission_parallel_batch.rs` (sibling non-block), `tests/streaming_permission.rs`, `tests/permission_wildcard_isolation.rs` (split the permission side from the extension side). agemo's permission bridge (`agemo/src/main.rs` stdin↔`AgentOp::AskUserAnswer`, `permission_timeout_secs`) moves into a host-owned reviewer (Phase 4).

**Decision (made with this result in hand):** proceed with §9a anyway — the cleaner, fully-decoupled end state is worth the LARGE refactor. The spike's surfaces above become Phase 2's task breakdown.

### 9e. Remote / IPC is just another reviewer

If Motosan later grows an app-server / remote driver, the structured request/response protocol (Codex's SQ/EQ) lives inside a **`ChannelReviewer`** impl: its `review()` emits an event over the wire and awaits a response the client resolves. The trait, engine, and policies are untouched — the wire protocol is one reviewer's concern, not an engine guarantee. Sharing one `ChannelReviewer` across all engines/children gives the uniform, can't-bypass protocol Codex enforces at the engine level — by convention here, which suits a multi-vertical framework.

### 9f. Spectrum coverage: pi and Codex both map onto this architecture

The whole point is one contract spanning pi-minimal → Codex-heavy. Verified mapping:

| Capability | pi | Codex | How it lands on this design |
|---|---|---|---|
| block / allow a call | `beforeToolCall → {block}` | execpolicy / sandbox | `Hook::pre_tool_use` (Abort) or policy `Deny` — no reviewer needed |
| ask a human | `await confirm()` in the hook | SQ/EQ event → `oneshot` → response op | policy `AskUser` + a reviewer whose `review()` awaits the host's channel (§9a) — **pi's `confirm()` is literally a `review()`** |
| reviewer is an agent | — | guardian sub-agent | `GuardianReviewer::review()` runs a sub-agent turn |
| central sink for many agents | — (no sub-agents) | one approval sink | children share one `Arc<dyn Reviewer>` (§8) |
| structured remote protocol | — | engine-enforced SQ/EQ | a shared `ChannelReviewer` (§9e) — convention, more flexible |
| "don't ask again this session" | — | `ApprovedForSession` | a stateful reviewer remembers internally |
| three-axis security | — | execpolicy + sandbox + AskForApproval | orthogonal: policy (decision) + reviewer (who answers) + sandbox (separate) |

**pi is the degenerate case** of this design (single agent, reviewer = `confirm()`). **Every Codex mechanism** maps to a reviewer impl plus the sharing convention, with the engine staying minimal. Nothing in either reference point is inexpressible here.

## 10. Testing strategy

- **Decision unchanged:** existing `PermissionPolicy` composition tests stay green.
- **Reviewer unit:** `DenyReviewer` denies; an `AlwaysApprove` test reviewer approves; a `RecordingReviewer` asserts `ApprovalRequest` carries the right `tool_call` / `session_id` / `recent_messages`.
- **Wiring:** policy `AskUser` + `AlwaysApprove` → tool runs; policy `AskUser` + default (no reviewer) → denied (fail-safe).
- **Multi-agent:** parent spawns child whose policy returns `AskUser`; assert the *central* reviewer receives the request with the *child's* `session_id`; assert no deadlock and the child proceeds/denies per the verdict.
- **pi parity:** policy returning only `Allow`/`Deny` never constructs an `ApprovalRequest` (reviewer untouched).

## 11. Versioning & migration impact

- **primitives:** purely additive — one new trait plus two new types, with **no changes to any existing item** (unlike D-M10-2, which added a field to an existing struct and forced literal constructors to update). This is a non-breaking minor bump; every existing `Hook` / `PermissionPolicy` impl compiles untouched.
- **loop:** new `EngineBuilder::reviewer()` setter (additive); resolve `Permission::AskUser` through the reviewer per §9a (fold review into the per-call future; reviewer owns its I/O). This **replaces** the current event/op approval path, so its existing approval tests get **rewritten** (R1) — the §9d spike (done; verdict LARGE) already lists them and the restructure surface. Default-when-none is `DenyReviewer`. **F7 (resolved):** today an unbridged/unanswered `AskUser` *stalls* (deferred call never resolves until timeout/cancel); `DenyReviewer` as the default is a deliberate, safer change for the no-reviewer case — call it out in the changelog. The interactive ask-the-human reviewer moves to the host (agemo), preserving its behaviour.
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

Do not implement before **M11** (rental harness → freeze 1.0). This is forward design. When a vertical genuinely needs interactive child-agent approval: (1) add the primitives types (additive); (2) add the `reviewer()` setter + `DenyReviewer` default, and resolve `AskUser` through the reviewer per §9a (fold review into the per-call future) — the §9d spike is done (verdict LARGE); follow its restructure surface, then rewrite the displaced event/op approval tests (R1); (3) add the subagent inheritance sugar so a child's `AskUser` routes to the parent session's reviewer/ops channel (the actual gap); (4) agemo provides its own host-owned `Reviewer` (owns stdin I/O, §9a) and wires it via `.reviewer(..)` — behavior-preserving for the root.
