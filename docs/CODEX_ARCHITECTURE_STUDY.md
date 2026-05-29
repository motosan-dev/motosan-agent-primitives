# Codex Architecture Study

**Date:** 2026-05-29
**Author:** investigation for Motosan framework design
**Subject:** OpenAI Codex (`github.com/openai/codex`) — local clone `~/Projects/wade/codex`, `codex-rs` workspace @ `9f42c89` (2026-05-24)
**Purpose:** Reference study to inform Motosan's multi-agent + approval design. Captured as an input for **M11+** (post-1.0), NOT a directive to change anything now.

> Scope note: `codex-rs` is a 93-crate Rust workspace. This study focuses on the **architecturally significant systems** (protocol, turn loop, tools, security/approval, multi-agent, extensibility). Peripheral crates (`tui`, `app-server-transport`, `cloud-tasks`, `realtime-webrtc`, `windows-sandbox-rs`, …) were only skimmed. File paths are relative to `codex-rs/` unless noted.

---

## 1. Overall shape: a large `core` + SQ/EQ protocol

Codex is **not** layered the way Motosan is (thin contract crate + separate `tool`/`loop`/`harness` crates). It is one large **`core`** crate (~90 modules: `session`, `tasks`, `session/turn`, `tools`, `guardian`, `hook_runtime`, `thread_manager`, `goals`, `skills`, `sandboxing`, …) wrapped by `protocol` / `app-server` / `exec` / `tui` / `cli`.

The public interface is a **dual-queue (SQ/EQ) protocol** (`protocol/src/protocol.rs`):

- **Submission queue** — `Op` enum: `UserInput`, `ExecApproval`, `PatchApproval`, `ResolveElicitation`, `InterAgentCommunication`, `RequestPermissionsResponse`, `DynamicToolResponse`, `Interrupt`, …
- **Event queue** — `EventMsg` enum (`protocol.rs:1137`): `ExecApprovalRequest`, `ApplyPatchApprovalRequest`, `RequestUserInput`, …

Entry point: `CodexThread::submit(op) -> String` (`codex_thread.rs:131`). **Every interaction is "push an `Op`, receive a stream of `Event`s."** The TUI and app-server are just clients of this protocol. This is the canonical "Codex-style mpsc channel" referenced in Motosan's M8.6.1 plan.

**Contrast with Motosan:** Motosan calls traits in-process (`Engine`, `Tool`, `Hook`). Codex routes everything through queues, which natively supports UI / IPC / remote drivers but adds protocol surface.

## 2. Turn loop (`core/src/session/turn.rs:131` `run_turn`)

Order of a single turn:

```
run_pre_sampling_compact            // compact context first (overflow handling)
record_context_updates
build_skills_and_plugins            // inject skills / plugins / connectors
run_pending_session_start_hooks     // SessionStart hooks
run_hooks_and_record_inputs         // UserPromptSubmit hooks
merge_connector_selection
→ (model sampling + tool-call loop)
```

Notable: **compaction, skill injection, and hooks are first-class steps in the turn loop**, not bolted on. Motosan models autocompact/redact as `impl Hook`; Codex weaves them directly into `run_turn`.

## 3. Tool system (`core/src/tools/`)

- Structure: `handlers/` (individual tools), `runtimes/`, `lifecycle.rs`, `code_mode/`, `dynamic.rs`.
- Built-in handlers: `shell`, `unified_exec`, `apply_patch` (with a `.lark` grammar), `mcp` + `mcp_resource`, `plan`, `goal`, `request_permissions`, `request_plugin_install`, `extension_tools`, **`multi_agents` / `multi_agents_v2` / `agent_jobs`**, `dynamic` (runtime-defined tools).
- **`code_mode`**: lets the model invoke tools by writing code ("code as actions").
- **MCP is first-class**: `mcp-server` / `rmcp-client` / `codex-mcp` crates wire tools, resources, and approval elicitation end-to-end.

## 4. Security model: three separated axes (the most transferable idea)

Codex splits "is this allowed?" into **three independent axes**. Motosan currently collapses all three into one `PermissionPolicy::check`.

| Axis | Codex | Role |
|---|---|---|
| **Auto-safe rules** | `execpolicy` crate (+ `execpolicy-legacy`) | Which commands are "known safe" and auto-approved (allowlist / rule language) |
| **Isolation** | `SandboxPolicy` + `linux-sandbox` / `windows-sandbox` / `bwrap` / `sandboxing` | Real OS-level sandbox (network, writable paths) |
| **Who answers when asking** | `AskForApproval` (`UnlessTrusted` / `OnRequest` / `Never` / `Granular`) + **Guardian** | Approval mode + the reviewer |

Approval is centralized in **`Session::request_command_approval`** (`session/mod.rs:1994`):

```rust
let (tx_approve, rx_approve) = oneshot::channel();
ts.insert_pending_approval(approval_id, tx_approve);     // register in the turn's pending-approval map
self.send_event(EventMsg::ExecApprovalRequest { .. });   // emit to client over the event queue
rx_approve.await.unwrap_or(ReviewDecision::Abort)        // agent loop blocks on the oneshot
```

The human (TUI / app-server) answers asynchronously via `Op::ExecApproval { id, decision }`, which resolves the oneshot. `ReviewDecision` = `Approved` / `ApprovedForSession` / `Denied` / `Abort`.

**Guardian** (`core/src/guardian/`): when `approval_policy ∈ {OnRequest, Granular}` and `approvals_reviewer == AutoReview` (`review.rs:145` `routes_approval_to_guardian`), approvals do not go to a human — they are adjudicated by a **guardian reviewer that is itself a sub-agent** (`SessionSource::SubAgent(Other("guardian"))`). `review.rs:542` `review_approval_request` spins up a guardian review turn that returns a `ReviewDecision`. I.e. **another agent acts as the approval officer.**

## 5. Multi-agent (the direct answer to "can policy work together across agents?")

Multi-agent is **exposed as tools** (`multi_agents` / `multi_agents_v2` / `agent_jobs` handlers) — the model spawns/coordinates sub-agents via tool calls, the same shape as Motosan's spawn/send/wait. The substrate differs significantly:

- **Topology store**: `agent-graph-store` ("storage-neutral parent/child topology for thread-spawned agents") + `thread_manager.rs` (`spawn_thread_with_source`, `list_thread_spawn_descendants`, depth tracking).
- **Fork semantics**: `ForkSnapshot` / `TruncateToLastSamplingBoundary` — a child forks from the parent's history at a safe sampling boundary.
- **Inheritance (key)**: per `multi_agents.rs` docs, sub-agents *"inherit runtime-only state such as **provider, approval policy, sandbox, and cwd**, and then optionally layer role-specific config on top."* → **A child inherits the parent's approval policy + sandbox by default.**
- **Delegation + approval back-flow**: `codex_delegate.rs` wires a delegated sub-agent's `ExecApprovalRequest` / `RequestPermissions` / `RequestUserInput` back through channels — so **a child's approval/input requests bubble up to the same central reviewer** (human or guardian).
- **Inter-agent comms**: `Op::InterAgentCommunication`; `agent-identity` crate (agent identity).

**Codex's answer to "policy working together":** child **inherits** parent policy by default + approval requests **flow back** to one central sink + the reviewer is **swappable** (human or guardian agent). Motosan today: child does **not** inherit (no policy field on `ChildSpec`), and a child's `AskUser` has **no answering channel**.

## 6. Extensibility

- **Hooks** (`codex_hooks` crate + `core/src/hook_runtime.rs`): `PreToolUse` / `PostToolUse` / `SessionStart` / `UserPromptSubmit` / `Stop` — **nearly identical lifecycle names to Motosan's `Hook` trait** (both are Claude-Code-style). Outcomes can inject context (`ContextInjectingHookOutcome`).
- **Skills** (`skills` / `core-skills`), **Plugins** (`plugin` / `core-plugins` + a `request_plugin_install` tool), **Connectors** (`connectors`), **Memories** (`memories`).
- **Goals** (`goals.rs`, ~1882 lines): persisted per-thread goals; the turn loop applies `GoalRuntimeEvent`s (e.g. usage-limit handling).
- **Collaboration templates** (`collaboration-mode-templates`: `plan` / `execute` / `pair_programming` / `default`).

## 7. Comparison with Motosan + recommendation

| Dimension | Motosan | Codex |
|---|---|---|
| Structure | thin contract layer + layered crates (third-party-vertical friendly) | one large `core` + SQ/EQ protocol |
| Interface | direct trait calls (in-process `Engine`) | everything via `Op`/`Event` queues (UI/IPC/remote-ready) |
| Policy | pluggable `PermissionPolicy` trait (per Engine) | static `AskForApproval` + `execpolicy` + sandbox — **three separated axes** |
| Child-agent policy | not inherited, must wire manually; child `AskUser` has no back-flow | **inherited by default** + approval back-flow + swappable reviewer (incl. guardian agent) |
| Hooks | `Hook` trait (9 lifecycle methods) | `codex_hooks` (same lifecycle names) |
| Compaction / skills | modeled as `impl Hook` | woven into the turn loop |

### What is worth borrowing (M11+ input, not now)

1. **Make approval session/manager-level infrastructure, with child requests flowing back** (the `codex_delegate` pattern). This directly fills Motosan's known gap: a sub-agent's `AskUser` has no answering bridge today (the agemo stdin bridge is wired only to the root engine).
2. **Separate the three security axes** (auto-safe allowlist / sandbox / who-answers) instead of collapsing everything into `PermissionPolicy::check`. Motosan already has the ingredients (`ToolAnnotations` read_only/destructive/network + the sandbox crate).
3. **Default child inheritance of parent policy/sandbox** — a better default than "manually share the `Arc`."
4. **Swappable reviewer, including a guardian-style reviewer agent** — elegant, but explicitly post-1.0.

### What NOT to copy

- The **monolithic `core`** — Motosan's layered thin crates are better for third parties writing verticals.
- **Replacing the pluggable `PermissionPolicy` trait with a static enum** — Codex is single-domain (shell/exec/code); Motosan is multi-vertical (finance/rental/healthcare), where domain-specific policies (`FinanceApprovalPolicy`) are the point.
- **Full SQ/EQ-everywhere** — unless/until Motosan needs an app-server / remote driver; in-process traits are simpler otherwise.

### Sequencing

Do **not** let this block **M11** (rental harness → freeze 1.0). Multi-agent approval is not on M11's critical path. Treat this study as a design input; implement the centralized-approval channel when a vertical actually needs interactive child-agent approval (post-M11, when the API is more stable).

---

## Appendix: key file map (codex-rs @ 9f42c89)

| System | Files |
|---|---|
| Protocol (SQ/EQ) | `protocol/src/protocol.rs` (`Op`, `EventMsg`) |
| Thread entry | `core/src/codex_thread.rs` (`CodexThread::submit`) |
| Turn loop | `core/src/session/turn.rs` (`run_turn`) |
| Approval channel | `core/src/session/mod.rs` (`request_command_approval`) |
| Guardian | `core/src/guardian/{review,approval_request}.rs` |
| Security | `execpolicy/`, `sandboxing/`, `linux-sandbox/`, `windows-sandbox-rs/`, `bwrap/` |
| Multi-agent | `core/src/thread_manager.rs`, `core/src/codex_delegate.rs`, `agent-graph-store/`, `agent-identity/`, `core/src/tools/handlers/multi_agents*.rs` |
| Hooks | `hooks/` (`codex_hooks`), `core/src/hook_runtime.rs` |
| Tools | `core/src/tools/` (`handlers/`, `runtimes/`, `lifecycle.rs`, `code_mode/`) |
| Goals / skills / plugins | `core/src/goals.rs`, `skills/` + `core-skills/`, `plugin/` + `core-plugins/` |
