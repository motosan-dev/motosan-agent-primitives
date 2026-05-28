# M8.6.1 Phase G Revision — event-derived subagent stop notifications

**Date:** 2026-05-28
**Parent:** [M8.6.1 plan §G / D-861-5](M8_6_1_PATH_UNIFICATION_PLAN.md)
**Status:** Awaiting approval — replaces the original D-861-5 design

---

## 0. Why this revision exists

The original D-861-5 mandated a "single status-transition gate in `manager.rs`" that all 6 terminal-state writes would call through. The first execution attempt revealed three architectural problems that the gate-in-manager design can't cleanly solve:

1. **Driver tasks have no HookCtx.** Termination happens in `tokio::spawn`-ed background tasks, but `on_subagent_stop` requires HookCtx. The `pending_stops + ops poke` workaround the implementer tried is parallel state — not the "single gate" the plan promised.
2. **Channel close ≠ termination.** Persistence/restart machinery closes input channels intentionally; treating that as Completed would break recover paths.
3. **Long-lived semantics conflict with "reply = stop".** Children handle `send_message` → `wait` → `send_message` cycles; per-reply completion is wrong.

After reading OpenAI Codex CLI's `multi_agents` source (specifically `codex-rs/core/src/agent/status.rs` and `codex-rs/core/src/session/mod.rs:607`), the answer is cleaner than my original three options (A/B/C). Codex doesn't have a separate "stop notification channel" — they **derive status from events** in their unified event stream. The pattern is small, well-tested, and most of the infrastructure we'd need is already in motosan-agent-subagent.

## 1. What Codex taught us

| Codex source | Pattern | Direct quote / signature |
|---|---|---|
| `protocol/src/protocol.rs` (AgentStatus enum) | 7 variants split into "active" vs "terminal" with explicit `Interrupted` ("may receive more input") | `Interrupted` is documented as non-terminal |
| `agent/status.rs::agent_status_from_event` | Status is a pure function of the latest event | `fn agent_status_from_event(msg: &EventMsg) -> Option<AgentStatus>` |
| `agent/status.rs::is_final` | Centralized terminal check | Excludes `PendingInit`, `Running`, `Interrupted` |
| `session/mod.rs:607` | One `watch::channel<AgentStatus>` per session, observed from anywhere | `let (agent_status_tx, agent_status_rx) = watch::channel(AgentStatus::PendingInit);` |
| `tools/handlers/multi_agents/{spawn,send_input,wait,resume_agent,close_agent}.rs` | 5 lifecycle tools — long-lived but with explicit terminal events | — |

**Net pattern:** no parallel "notification" path. Driver emits a typed event into the existing event stream; the dispatcher derives status (and stop notifications) from those events.

## 2. What's already in motosan-agent-subagent (audit 2026-05-28)

The infrastructure we need is mostly built. Verified by grep:

| Concept | Exists today? | Where |
|---|---|---|
| `SubagentStatus` enum with `Running`, `Closed`, `Cancelled`, `Completed`, `Failed`, `Suspended` | ✅ | `src/subagent/handle.rs` |
| `Suspended` as a non-terminal variant (Codex-style "may resume") | ✅ already named correctly | `handle.rs` |
| `SubagentEvent` enum with Spawned/Closed/MessageSent/etc | ✅ partial | `src/subagent/event.rs` |
| `watch::channel<SubagentStatus>` per session | ✅ | `driver.rs:295`, `manager.rs:187,710,820,920` |
| `ctx.emit(SubagentEvent::X)` pattern from interceptor contexts | ✅ widely used | `manager.rs:81,91,243,278,324,450,614` |
| Driver-side event emission to a shared stream | ❌ **missing** | driver writes `status_tx` directly; no event |
| `derive_subagent_stop_from_event(...)` pure function | ❌ missing | — |
| `is_terminal(&SubagentStatus)` helper | ❌ missing (status check open-coded) | — |

So this revision is mostly **wiring existing parts in a new pattern**, plus 1 new SubagentEvent variant, 1 helper function, and a small dispatcher subscription.

## 3. Design decisions

### D-PG-1. Add `SubagentEvent::Terminated` variant

```rust
// src/subagent/event.rs
pub enum SubagentEvent {
    // ... existing variants ...
    Terminated {
        session_id: String,
        status: SubagentStatus,     // must be one of the terminal variants
        stop_reason: StopReason,    // primitives::StopReason
        final_message: Option<Message>,  // for Completed; None for others
    },
}
```

Driver emits this when its task ends (per the classification table in D-PG-3). Manager emits the same when it terminates a child synchronously. **Single event type — both paths converge on it.**

### D-PG-2. Add `is_terminal(&SubagentStatus) -> bool` helper

Mirrors Codex's `is_final`. Lives in `src/subagent/handle.rs`:

```rust
impl SubagentStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(self,
            SubagentStatus::Closed
            | SubagentStatus::Cancelled
            | SubagentStatus::Completed
            | SubagentStatus::Failed { .. }
        )
        // Note: Running and Suspended are NOT terminal
    }
}
```

`Suspended` deliberately excluded — matches Codex's `Interrupted` handling. Closes the "channel close ≠ termination" hole.

### D-PG-3. Driver exit classification

Driver's exit reasons map to terminal-or-not:

| Driver exit cause | Status | Terminal? | StopReason |
|---|---|---|---|
| Child LLM emits TurnComplete (model decides it's done) | `Completed` | ✅ | `StopReason::Completed` |
| Child LLM hits iteration limit | `Failed { reason }` | ✅ | `StopReason::BudgetExhausted` |
| Driver panics / unexpected error | `Failed { reason }` | ✅ | `StopReason::Error { message }` |
| Parent cancels (cancel_all) | `Cancelled` | ✅ | `StopReason::UserCancelled` |
| Input channel closes during restart / suspend | `Suspended` | ❌ | (no Terminated event) |
| Explicit close_agent tool call | `Closed` | ✅ | `StopReason::UserCancelled` |

Driver MUST classify its exit. If the exit is `Suspended`, it does NOT emit Terminated — recover path takes over.

### D-PG-4. Centralized derive function

```rust
// src/subagent/stop_derivation.rs (NEW file, ~30 LOC)
pub(crate) fn derive_subagent_stop(event: &SubagentEvent) -> Option<SubagentResult> {
    match event {
        SubagentEvent::Terminated { session_id, status, stop_reason, final_message }
            if status.is_terminal() => {
            Some(SubagentResult {
                session_id: session_id.clone(),
                stop_reason: stop_reason.clone(),
                final_message: final_message.clone(),
            })
        }
        _ => None,  // non-terminal events don't fire on_subagent_stop
    }
}
```

Pure function. Test in isolation. Easy to extend with new event variants.

### D-PG-5. Hook dispatch via the new event channel (driven by D-PG-8)

`SubagentInterceptor`'s lifecycle methods (which DO have HookCtx) drain the subagent event channel introduced in D-PG-8, derive stop notifications, and dispatch:

```rust
// In SubagentInterceptor::intercept_tool_call, after_tool_result, on_terminal — every entry
fn drain_and_dispatch_terminations(&mut self, ctx: &mut HookCtx<'_>) {
    while let Ok(event) = self.subagent_event_rx.try_recv() {
        if let Some(result) = derive_subagent_stop(&event) {
            ctx.notify_subagent_stop(result);
        }
        // Non-terminal events still get forwarded out via ctx.emit if desired.
    }
}
```

Drain on EVERY interceptor lifecycle entry — `intercept_tool_call`, `after_tool_result`, `on_terminal`, `before_iteration`. This guarantees that as soon as the parent's next interceptor tick happens, any driver-emitted terminations are dispatched.

**Critical correction from initial plan draft:** the original D-PG-5 claimed "existing event pipeline" — that was wrong. `ctx.emit()` requires HookCtx, which driver tasks don't have. We need a new HookCtx-free sender (D-PG-8) to bridge.

### D-PG-6. Idempotency at emit (status_tx CAS guard)

Driver and manager paths MUST NOT both emit Terminated for the same session. Both gate at emit site using the existing `status_tx` watch channel as the source of truth:

```rust
// helper used by both driver and manager close paths
fn emit_terminated_once(
    status_tx: &watch::Sender<SubagentStatus>,
    event_tx: &mpsc::Sender<SubagentEvent>,  // from D-PG-8
    new_terminal: SubagentStatus,
    stop_reason: StopReason,
    final_message: Option<Message>,
    session_id: &str,
) {
    let current = status_tx.borrow().clone();
    if current.is_terminal() {
        return;  // someone else already terminated this session
    }
    status_tx.send_replace(new_terminal.clone());
    let _ = event_tx.try_send(SubagentEvent::Terminated {
        session_id: session_id.into(),
        status: new_terminal,
        stop_reason,
        final_message,
    });
    // If event_tx is dropped (receiver gone), event is lost — acceptable per D-PG-8 backpressure note.
}
```

Compare-and-swap on the watch channel + try_send to the event channel. No locks, no Mutex queue.

### D-PG-8. Driver-to-dispatcher transport — new `mpsc::Sender<SubagentEvent>` per subagent

**Source of the prior plan's gap:** driver tasks have no HookCtx, so they cannot call `ctx.emit()`. They need a HookCtx-free channel to write into. This mirrors Codex's `tx_event: mpsc::Sender<EventMsg>` at `session/mod.rs:607`.

**Mechanism:**

```rust
// At spawn time, in Manager::spawn:
let (subagent_event_tx, subagent_event_rx) = mpsc::channel::<SubagentEvent>(SUBAGENT_EVENT_CAPACITY);

// Driver gets the sender — clonable, Send + Sync, no HookCtx needed:
tokio::spawn(child_driver(
    /* ... existing args ... */,
    subagent_event_tx,  // NEW
));

// Manager stores the receiver alongside other per-subagent state.
// SubagentInterceptor owns the receivers (one per active subagent),
// drains them in lifecycle methods per D-PG-5.
```

**Per-subagent channel** (not session-global) because:
- Lifetime matches the subagent — sender drops when driver task ends, receiver closes naturally
- Multiple concurrent subagents don't share a queue
- Receiver cleanup is implicit when the subagent is reaped

**Capacity:** small (e.g. 16) is plenty — Terminated is emitted at most once per session per termination cause; intermediate events (if added later) are also low-volume per-subagent.

**Backpressure:** `send().await` from driver. If receiver is gone (e.g. interceptor dropped), send returns Err — driver logs and continues exit. Termination event lost in that case, but that's acceptable since the subagent state is already terminal and no observer is watching.

**Idempotency** (replaces D-PG-6's CAS approach): mpsc preserves send order; we use a `bool already_terminated` flag on the driver side to guard against double-emit. Manager's close path uses the watch channel's current value as the guard (status already terminal → skip). Either way, channel can't deliver duplicates because either driver-side or manager-side guards prevent the second send.

### D-PG-7. Suspend path explicitly does NOT emit Terminated

When the recover/restart machinery transitions a child to `Suspended`, it writes the status but emits NO event. The interceptor's hook is silent. When the same child later resumes (status → `Running` again), still no event. Only when it FINALLY transitions to a terminal state does Terminated fire.

## 4. Implementation steps (2 days total — was 1.5, +0.5 for D-PG-8 channel work)

### Step 1 — Add `SubagentEvent::Terminated` + `is_terminal` helper (2h)

- `src/subagent/event.rs`: add Terminated variant
- `src/subagent/handle.rs`: add `is_terminal()` method on SubagentStatus
- `src/subagent/stop_derivation.rs` (new file): `derive_subagent_stop` function
- Tests for derive function + is_terminal (~5 cases each)

### Step 2 — Add per-subagent event channel infrastructure (D-PG-8) (3h)

- Manager: in each subagent-spawn site (manager.rs:187/710/820/920), create `mpsc::channel::<SubagentEvent>(16)`. Store receiver per-subagent in the existing per-subagent state struct (keyed by session_id).
- Driver: extend `child_driver` signature to accept `subagent_event_tx: mpsc::Sender<SubagentEvent>`.
- SubagentInterceptor: gain access to the receiver map (Arc<Mutex<HashMap<SessionId, mpsc::Receiver<SubagentEvent>>>>). Add `drain_and_dispatch_terminations(ctx)` helper per D-PG-5.
- **Unit test**: driver sends Terminated into channel; receiver-side test asserts derive + notify behavior in isolation (no full engine roundtrip).

### Step 3 — Emit Terminated from driver paths via the new channel (3h)

Audit driver.rs:67, 81-83 + handle.rs:32. Classify each per D-PG-3 table. Driver calls `subagent_event_tx.try_send(SubagentEvent::Terminated { ... })` for terminal exits; skip emission for Suspended exit. Use local `bool emitted_terminal` to prevent double-emit if multiple driver code paths converge.

### Step 4 — Emit Terminated from manager close paths via the same channel (2h)

Migrate the 4 manager-side terminal-status writes (manager.rs:359-362, 790-796) to push into the same per-subagent event channel. Manager-side idempotency guard: check current `SubagentStatus` via `status_tx.borrow()` before emitting — if already terminal, skip (per D-PG-6).

### Step 5 — Wire `drain_and_dispatch_terminations` into SubagentInterceptor lifecycle methods (2h)

Add `self.drain_and_dispatch_terminations(ctx)` as the first line of:
- `intercept_tool_call`
- `after_tool_result`
- `on_terminal`
- (and any other lifecycle method the engine actually invokes — verify by grep)

This guarantees that any driver-emitted termination is dispatched on the parent's next tick.

### Step 6 — Integration tests (4h)

Per subagent#1 acceptance:
- `subagent_stop_fires_on_natural_completion` — child driver returns TurnComplete → Hook sees Completed
- `subagent_stop_fires_on_child_failure` — driver panics → Hook sees Error
- `subagent_stop_fires_on_parent_cancellation` — cancel_all → Hook sees UserCancelled (exactly once, not double)
- `subagent_stop_does_not_fire_on_suspend` — restart path transitions to Suspended → Hook silent
- `subagent_stop_fires_once_on_double_close` — explicit close + late manager cleanup → Hook fires once
- Existing explicit-close test continues to pass unchanged

### Step 7 — CHANGELOG 0.3.1 + commit + push + close subagent#1 (1h)

## 5. Acceptance

- All 5 new tests + existing explicit-close test green
- `cargo test --all-features` in subagent green
- `cargo test --all-features` in loop still green (no regression in loop tests that exercise subagent through interceptors)
- subagent#1 closed with commit SHA reference
- No primitives commit (D-861-8 still holds)
- No new mpsc/oneshot channels added — Terminated rides the existing SubagentEvent stream

## 6. What this revision does NOT change

- Phase I (architectural invariants test in loop) — unchanged, still required
- The remaining 10 acceptance gates from M8.6.1 §6 — only gates 7 and 9 are affected by this revision; the other 9 stay as-is
- Loop side (0.25.0) — no changes; this is purely subagent-side work
- M9 readiness criteria — still need this completed before M9 starts

## 7. Effort vs original Phase G

| | Original Phase G | This revision |
|---|---|---|
| Total | 1 day | **2 days** |
| New code | Single gate refactor in manager.rs | 1 event variant + 1 helper + 1 derive fn + per-subagent mpsc channel + dispatch wiring across 4 interceptor methods |
| Risk | High (cross-task sync via shared state) | Low (Codex-faithful channel pattern, no Mutex, no parallel notification path) |
| Aligns with | Plan's "Option A" stub | Codex CLI's `tx_event` pattern verified at `codex-rs/core/src/session/mod.rs:607` |

The +1 day buys correctness, no Mutex contention, no stale-notification risk, and architectural alignment with the proven Codex pattern.

## 8. Open questions

### Q-PG-1. Should `SubagentStatus::Completed` carry the final message?

Codex's `AgentStatus::Completed(Option<String>)` does. Our current `Completed` is unit-variant. If we don't add the payload now, `final_message` only flows via the SubagentEvent::Terminated event, never on the Status itself. Minor API question — probably fine to defer.

**Recommendation:** leave `SubagentStatus::Completed` as unit. Carry `final_message` on the event only. If consumers need it on the status later, additive change.

### Q-PG-2. Should `is_terminal` be public or pub(crate)?

Public is more useful for downstream consumers (M9 FinanceHarness AuditLogHook may want to check). Cost: adds to public surface.

**Recommendation:** `pub`. It's a trivial query and naturally useful.

### Q-PG-3. Does the Terminated event also fire when the WHOLE engine shuts down (parent cancel cascade)?

If yes, every child gets Terminated{Cancelled} during shutdown — could be noisy. If no, children leak as "Running" in audit log.

**Recommendation:** yes, fire. AuditLogHook needs to know children stopped. Noise is preferable to silent leak.

---

These 3 open questions can be resolved during execution if needed. Plan is otherwise complete.
