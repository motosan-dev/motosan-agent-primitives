# Approval Capability Study — pi vs Codex vs the Motosan Reviewer seam

**Date:** 2026-05-30
**Purpose:** Verify, against the real source of both reference projects, whether the Motosan `Reviewer`/approval design (spec `2026-05-29-reviewer-approval-seam-design.md`) can deliver pi's and Codex's approval functionality. Corrects an earlier too-optimistic claim that "Codex core maps with 2 small caveats."
**Sources:** local clones — `earendil-works/pi` (`~/Projects/wade/pi`), `openai/codex` (`~/Projects/wade/codex`, `codex-rs`).

---

## TL;DR

- **pi** is a **strict subset** of what Motosan already has. Fully implementable, trivially. The Reviewer seam exceeds it.
- **Codex's *swappable reviewer* idea** maps to the Motosan `Reviewer` seam (and the answering channel already exists in the loop).
- **Codex's *full approval system* does NOT** map to the current design. It is an **exec/patch-centric, three-axis** system (execpolicy + OS sandbox + policy-mutating decisions), engine-enforced. Matching it is **several milestones of new infrastructure**, not part of the Reviewer seam. `Reviewer seam ≠ Codex approval system`.

---

## pi — verified strict subset

- **The whole gate is binary:** `BeforeToolCallResult { block?: boolean; reason?: string }` (`packages/agent/src/types.ts`). No richer decision, no amendment, no session cache.
- **Human approval = a UI primitive returning a boolean:** the coding-agent extension exposes `confirm(title, message): Promise<boolean>` (`packages/coding-agent/src/core/extensions/types.ts:129`); an extension calls it inside `beforeToolCall` and returns `{ block: !ok }`.
- **No sub-agents / multi-agent** in the core (the spawn/delegate grep hits are process-spawn in `bash.ts` etc., not agents).
- **No sandbox in the approval path** (a `bun/restore-sandbox-env.ts` exists for the runtime, but it is not an approval-integrated sandbox).

**Mapping to Motosan:** pi's `block/allow` = a `PermissionPolicy` returning `Deny`/`Allow` or a `Hook::pre_tool_use` `Abort`; pi's `confirm()` = a `Reviewer::review()` awaiting a host confirm. Motosan's `Allow|Deny|AskUser` + `Reviewer` is strictly more expressive. **No gap.**

---

## Codex — verified, and larger than represented

### Approval is exec/patch-centric, not generic-per-tool

Approval is built around shell commands and patches: `assess_command_safety` / `assess_patch_safety` (`core/src/safety.rs`) and `create_exec_approval_requirement_for_command` (`core/src/exec_policy.rs:272`). The decision type is:

```rust
pub enum SafetyCheck {           // core/src/safety.rs:22
    AutoApprove { sandbox_type, user_explicitly_approved },
    AskUser,
    Reject { reason },
}
```

Motosan's model is the opposite: a **generic per-tool** `PermissionPolicy` keyed on `ToolAnnotations` (any tool). Codex's execpolicy machinery (command-prefix rules) is specific to shell commands and does not port 1:1.

### Three-axis security converging in the safety check

`assess_patch_safety(action, policy: AskForApproval, permission_profile, file_system_sandbox_policy, cwd, windows_sandbox_level)` shows the axes that feed one decision:

1. **execpolicy** — "known safe" command-prefix rules (auto-approve without asking).
2. **`SandboxPolicy`** — real OS isolation: `DangerFullAccess` / `ReadOnly { network_access }` / workspace-write (`protocol/src/protocol.rs:858`), enforced by `linux-sandbox` / `windows-sandbox` / `bwrap`.
3. **`AskForApproval`** — the mode deciding *when* to ask.

Motosan has the `motosan-sandbox` crate but it is **not wired into the approval path**, and there is **no execpolicy** equivalent.

### `ReviewDecision` is rich and **policy-mutating** (7 variants)

```rust
pub enum ReviewDecision {        // protocol/src/protocol.rs:3530
    Approved,
    ApprovedExecpolicyAmendment { proposed_execpolicy_amendment },  // persist a command-prefix rule
    ApprovedForSession,                                             // auto-approve matching for the session
    NetworkPolicyAmendment { network_policy_amendment },            // persist allow/deny for a host
    Denied,        // (default)
    TimedOut,
    Abort,         // stop until next user input
}
```

The approve-with-amendment variants **feed the decision back into execpolicy / network-policy state** for future calls. Motosan's `ReviewDecision { Approve, Deny }` is far thinner — and crucially, **Motosan has no execpolicy or network-policy layer to amend.** So this is not a small enum addition; it presupposes infrastructure Motosan does not have.

### Engine-enforced + multi-agent inheritance

- Every exec/patch is safety-checked by the engine — approval is **enforced**, not opt-in.
- Sub-agents **inherit** runtime state including **approval policy + sandbox** (`core/src/tools/handlers/multi_agents.rs` module doc), giving the uniform central-sink behaviour.

---

## Honest gap analysis — what the Reviewer seam does and does NOT deliver

| Capability | pi | Codex | Reviewer seam (current design) |
|---|---|---|---|
| binary block / allow | ✅ (the whole thing) | ✅ (a sub-case) | ✅ `PermissionPolicy` Deny/Allow |
| ask a human | ✅ `confirm()` | ✅ | ✅ a `Reviewer` awaiting the host channel |
| reviewer is an agent (guardian) | — | ✅ | ✅ (needs Phase 3 subagent) |
| multi-agent central sink + inheritance | — | ✅ | ✅ (shared `Arc<dyn Reviewer>`, Phase 3) |
| **swappable reviewer** | — | ✅ | ✅ — this is exactly the seam |
| **engine-enforced / can't-bypass** | n/a | ✅ engine | ⚠️ **convention** (share one reviewer), not enforced |
| **exec/patch-centric execpolicy (auto-safe rules)** | — | ✅ | ❌ not modeled (Motosan is generic per-tool; no execpolicy) |
| **OS sandbox in the approval path** | — | ✅ | ❌ crate exists, **not wired** |
| **policy-mutating decisions** (persist execpolicy / network rule, approve-for-session) | — | ✅ 7-variant `ReviewDecision` | ❌ `Approve\|Deny` only; **no policy layer to amend** |

**Conclusion:** the Reviewer seam matches Codex's *"who answers an escalation, swappably"* idea — that part is sound and the channel exists. But **Codex's full approval system is much larger**: an exec-centric three-axis design (execpolicy + sandbox + policy-mutating decisions), engine-enforced. Delivering that in Motosan would require, as separate efforts:

1. An **execpolicy-equivalent** (auto-safe rules) — likely keyed on `ToolAnnotations` rather than command prefixes.
2. **Wiring `motosan-sandbox`** into the approval path.
3. A **richer `ReviewDecision`** + a **policy/network-rule layer** the reviewer can amend, plus a session approval cache.
4. (Optional) **engine-enforced** uniformity instead of convention.

These are post-1.0 milestones in their own right, **not** part of the Reviewer seam (Task 7 / Batch A/B).

---

## Recommendation

- Build the **Reviewer seam** as designed — it delivers pi-parity and Codex's *swappable-reviewer* idea, and closes the child-AskUser gap. That is a complete, valuable increment.
- Do **not** claim "full Codex approval parity" for it. The three-axis security + policy-mutating decisions are a separate, larger track to scope explicitly if/when a vertical needs it.
- Spec `2026-05-29-reviewer-approval-seam-design.md` §9f mapping is updated to carry these caveats (it previously read too optimistically).
