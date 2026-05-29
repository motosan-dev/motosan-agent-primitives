//! Hook middleware contract.
//!
//! A [`Hook`] is an async observer / rewriter attached to the agent loop's
//! lifecycle. The loop fires one of nine [lifecycle events](#lifecycle) at
//! each phase; each [`Hook`] gets to inspect, rewrite, skip, or abort.
//!
//! # Design decisions baked in
//!
//! - **D2 = B (revised after Codex + Claude Agent SDK research):**
//!   hooks **return by value**, never `&mut Ctx`. To rewrite a tool's
//!   input you return [`HookResult::Continue`] with
//!   `updated_input: Some(value)`. Mutation via `ctx.field = …` is
//!   structurally impossible — context structs do not expose `&mut`. This
//!   makes cancellation safe: a hook killed mid-rewrite simply has its
//!   return value discarded; there is no half-mutated state.
//!
//! - **D5 = A (revised after Codex research):** every lifecycle context
//!   struct carries a
//!   [`tokio_util::sync::CancellationToken`]. Long-running
//!   hooks (PII redaction, audit log flush, …) should poll
//!   `cancellation_token.is_cancelled()` or `select!` against
//!   `cancelled().await` and return early.
//!
//! - **Nine events, not eight:** `post_tool_use` fires on success;
//!   [`post_tool_use_failure`](Hook::post_tool_use_failure) is a separate
//!   event fired on tool error or cancellation. Audit hooks usually
//!   override both; rewrite hooks usually only override `post_tool_use`.
//!
//! # Lifecycle
//!
//! | Method                  | When                                     |
//! |-------------------------|------------------------------------------|
//! | [`session_start`]       | Session begins                           |
//! | [`session_end`]         | Session ends (clean or aborted)          |
//! | [`user_prompt_submit`]  | Before sending a user message to the LLM |
//! | [`pre_tool_use`]        | Before dispatching a tool call           |
//! | [`post_tool_use`]       | After successful tool call               |
//! | [`post_tool_use_failure`] | After tool error / cancellation        |
//! | [`pre_compact`]         | Before transcript compaction             |
//! | [`stop`]                | Loop is about to stop                    |
//! | [`subagent_stop`]       | A spawned subagent stopped               |
//!
//! [`session_start`]: Hook::session_start
//! [`session_end`]: Hook::session_end
//! [`user_prompt_submit`]: Hook::user_prompt_submit
//! [`pre_tool_use`]: Hook::pre_tool_use
//! [`post_tool_use`]: Hook::post_tool_use
//! [`post_tool_use_failure`]: Hook::post_tool_use_failure
//! [`pre_compact`]: Hook::pre_compact
//! [`stop`]: Hook::stop
//! [`subagent_stop`]: Hook::subagent_stop

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::message::Message;
use crate::tool::{ToolCall, ToolResult};

/// What a hook returns to the loop.
///
/// **Always return by value.** Mutation of any [`Hook`] ctx struct via
/// `ctx.field = …` is unsupported — the agent loop discards in-place
/// changes. To rewrite a tool's input you return
/// [`HookResult::Continue`] with `updated_input: Some(value)`.
///
/// See the module-level docs for the cancellation-safety rationale.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookResult {
    /// Proceed with the next hook (or the framework's default behaviour).
    ///
    /// If `updated_input` is `Some(v)`, the next hook / the dispatcher sees
    /// `v` as the tool's input. If `None`, the prior input is preserved.
    /// Observation-only hooks always return
    /// `Continue { updated_input: None }`.
    Continue {
        /// Optional rewritten tool input. Only meaningful for
        /// [`Hook::pre_tool_use`]; ignored by other events.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<serde_json::Value>,
    },
    /// Skip the rest of the chain and the default behaviour entirely.
    ///
    /// Use sparingly — `Skip` on `pre_tool_use` means the tool will NOT
    /// run; the framework injects a synthetic
    /// [`ToolResult::error`](crate::tool::ToolResult::error) so the model
    /// sees the skip.
    Skip {
        /// Diagnostic shown in events / surfaced to the model.
        reason: String,
    },
    /// Abort the entire agent loop.
    ///
    /// Treated as a fatal user-initiated stop; the loop emits
    /// [`StopReason::AbortedByHook`].
    Abort {
        /// Diagnostic shown in events / surfaced to the model.
        reason: String,
    },
}

impl Default for HookResult {
    fn default() -> Self {
        HookResult::Continue {
            updated_input: None,
        }
    }
}

impl HookResult {
    /// Convenience constructor: continue without rewriting input.
    pub fn cont() -> Self {
        HookResult::default()
    }

    /// Convenience constructor: continue with a rewritten input.
    pub fn rewrite(input: serde_json::Value) -> Self {
        HookResult::Continue {
            updated_input: Some(input),
        }
    }
}

/// Why the agent loop stopped.
///
/// Emitted by the loop when it terminates a session. Hooks observing
/// `session_end` / `stop` may inspect this; they cannot change it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopReason {
    /// Model produced a final answer with no further tool calls.
    Completed,
    /// User cancelled (Ctrl-C, UI stop button, …).
    UserCancelled,
    /// A hook returned [`HookResult::Abort`].
    AbortedByHook {
        /// Reason carried by the abort.
        reason: String,
    },
    /// Hit the configured max-iterations / max-tokens budget.
    BudgetExhausted,
    /// Unexpected error.
    Error {
        /// Diagnostic message.
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Lifecycle context structs (one per Hook method).
//
// Every ctx struct carries:
//   - session_id  — current session
//   - cancellation_token — long-running hooks must observe this (D5)
// ---------------------------------------------------------------------------

/// Context passed to [`Hook::session_start`].
#[derive(Debug, Clone)]
pub struct SessionStartCtx {
    /// Current session id.
    pub session_id: String,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::session_end`].
#[derive(Debug, Clone)]
pub struct SessionEndCtx {
    /// Current session id.
    pub session_id: String,
    /// Why the session ended.
    pub stop_reason: StopReason,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::user_prompt_submit`].
#[derive(Debug, Clone)]
pub struct UserPromptSubmitCtx {
    /// Current session id.
    pub session_id: String,
    /// The user message about to be sent to the LLM.
    pub message: Message,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::pre_tool_use`].
///
/// This is the **only** ctx whose hook can meaningfully return
/// [`HookResult::Continue`] with `updated_input: Some(...)`. The framework
/// applies that rewrite to the tool's input before dispatch.
#[derive(Debug, Clone)]
pub struct PreToolUseCtx {
    /// Current session id.
    pub session_id: String,
    /// The tool call about to be dispatched.
    pub tool_call: ToolCall,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::post_tool_use`] (success path).
#[derive(Debug, Clone)]
pub struct PostToolUseCtx {
    /// Current session id.
    pub session_id: String,
    /// The original tool call.
    pub tool_call: ToolCall,
    /// The successful tool result.
    pub tool_result: ToolResult,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Why a tool invocation failed, passed to [`Hook::post_tool_use_failure`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolFailure {
    /// The tool's own implementation reported an error.
    Error {
        /// Diagnostic message from the tool.
        message: String,
    },
    /// The tool was cancelled mid-execution (session abort, timeout, …).
    Cancelled,
    /// The tool exceeded the configured timeout budget.
    Timeout,
}

/// Context passed to [`Hook::post_tool_use_failure`] (error / cancel path).
///
/// Fires whenever a tool dispatch does **not** produce a successful
/// [`ToolResult`]. Audit hooks should override both this and
/// [`Hook::post_tool_use`] to capture every outcome.
#[derive(Debug, Clone)]
pub struct PostToolUseFailureCtx {
    /// Current session id.
    pub session_id: String,
    /// The original tool call.
    pub tool_call: ToolCall,
    /// Why the call failed.
    pub failure: ToolFailure,
    /// The final [`ToolResult`] the model will see for this failed call.
    ///
    /// Carries the same wire shape the model receives, with
    /// [`ToolResult::is_error`] set to `true` and `content` describing the
    /// failure. Audit hooks can record this directly instead of
    /// reconstructing a result from [`failure`](Self::failure). Added in
    /// 0.2.0 (M10 D-M10-2) — see CHANGELOG.
    pub result: ToolResult,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::pre_compact`].
#[derive(Debug, Clone)]
pub struct PreCompactCtx {
    /// Current session id.
    pub session_id: String,
    /// Messages the loop is about to compact (summarise / drop).
    pub messages: Vec<Message>,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::stop`].
#[derive(Debug, Clone)]
pub struct StopCtx {
    /// Current session id.
    pub session_id: String,
    /// Why the loop is stopping.
    pub stop_reason: StopReason,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

/// Context passed to [`Hook::subagent_stop`].
#[derive(Debug, Clone)]
pub struct SubagentStopCtx {
    /// Parent session id.
    pub session_id: String,
    /// Subagent session id that just stopped.
    pub subagent_session_id: String,
    /// Why the subagent stopped.
    pub stop_reason: StopReason,
    /// Cancellation token; hooks should observe this.
    pub cancellation_token: CancellationToken,
}

// ---------------------------------------------------------------------------
// The Hook trait itself.
// ---------------------------------------------------------------------------

/// Async middleware attached to the agent loop's lifecycle.
///
/// Object-safe: usable as `Arc<dyn Hook>`. Multiple hooks may be
/// registered per session; the loop fires each event through every hook
/// in registration order until one returns [`HookResult::Skip`] or
/// [`HookResult::Abort`].
///
/// Every method has a default implementation that returns
/// [`HookResult::cont()`]; override only the events you care about.
///
/// # Mutating tool input
///
/// To rewrite a `pre_tool_use` input, **return** the new value:
///
/// ```
/// use async_trait::async_trait;
/// use motosan_agent_primitives::hook::{Hook, HookResult, PreToolUseCtx};
/// use serde_json::json;
///
/// struct RedactHook;
///
/// #[async_trait]
/// impl Hook for RedactHook {
///     async fn pre_tool_use(&self, ctx: &PreToolUseCtx) -> HookResult {
///         // CORRECT: return a rewritten value.
///         let mut new_input = ctx.tool_call.input.clone();
///         if let Some(obj) = new_input.as_object_mut() {
///             obj.insert("ssn".into(), json!("<redacted>"));
///         }
///         HookResult::rewrite(new_input)
///     }
/// }
/// ```
///
/// `ctx.field = new_value` would not propagate — `&PreToolUseCtx` is a
/// shared reference, and the loop discards mutations even if you cloned
/// the ctx into an owning local. The cancellation-safety rationale is
/// covered at the module level.
///
/// # Skipping and aborting
///
/// ```
/// use async_trait::async_trait;
/// use motosan_agent_primitives::hook::{Hook, HookResult, PreToolUseCtx};
///
/// struct GuardHook;
///
/// #[async_trait]
/// impl Hook for GuardHook {
///     async fn pre_tool_use(&self, ctx: &PreToolUseCtx) -> HookResult {
///         if ctx.tool_call.name == "format_disk" {
///             HookResult::Abort { reason: "are you kidding".into() }
///         } else if ctx.tool_call.name == "slow_op" {
///             HookResult::Skip { reason: "policy".into() }
///         } else {
///             HookResult::cont()
///         }
///     }
/// }
/// ```
///
/// # Observing cancellation
///
/// Long-running hooks must poll
/// [`CancellationToken::is_cancelled`](tokio_util::sync::CancellationToken::is_cancelled)
/// or `select!` on `cancelled().await`:
///
/// ```no_run
/// use async_trait::async_trait;
/// use motosan_agent_primitives::hook::{Hook, HookResult, SessionStartCtx};
/// use tokio::time::{sleep, Duration};
///
/// struct WarmCache;
///
/// #[async_trait]
/// impl Hook for WarmCache {
///     async fn session_start(&self, ctx: &SessionStartCtx) -> HookResult {
///         tokio::select! {
///             _ = ctx.cancellation_token.cancelled() => {
///                 // Bail out cleanly.
///                 HookResult::cont()
///             }
///             _ = sleep(Duration::from_secs(5)) => {
///                 // Real work finished.
///                 HookResult::cont()
///             }
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait Hook: Send + Sync {
    /// Fired once when a session begins.
    async fn session_start(&self, _ctx: &SessionStartCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired once when a session ends, clean or aborted.
    async fn session_end(&self, _ctx: &SessionEndCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired before a user message is sent to the LLM.
    async fn user_prompt_submit(&self, _ctx: &UserPromptSubmitCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired before dispatching a tool call.
    ///
    /// This is the event where rewrite hooks live. Return
    /// [`HookResult::rewrite`] to replace the tool's input.
    async fn pre_tool_use(&self, _ctx: &PreToolUseCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired after a tool call completed **successfully**.
    ///
    /// For audit purposes you also want
    /// [`post_tool_use_failure`](Self::post_tool_use_failure).
    async fn post_tool_use(&self, _ctx: &PostToolUseCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired after a tool call **failed** (error / cancel / timeout).
    ///
    /// Separate from [`post_tool_use`](Self::post_tool_use) so audit hooks
    /// can capture failures without conditional branches on
    /// [`ToolResult::is_error`](crate::tool::ToolResult::is_error). Rewrite
    /// hooks rarely need this.
    async fn post_tool_use_failure(&self, _ctx: &PostToolUseFailureCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired before transcript compaction.
    async fn pre_compact(&self, _ctx: &PreCompactCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired right before the agent loop stops.
    async fn stop(&self, _ctx: &StopCtx) -> HookResult {
        HookResult::cont()
    }

    /// Fired when a spawned subagent stops.
    async fn subagent_stop(&self, _ctx: &SubagentStopCtx) -> HookResult {
        HookResult::cont()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;
    use serde_json::json;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    /// Object-safety smoke test.
    #[allow(dead_code)]
    fn assert_object_safe(_h: Arc<dyn Hook>) {}

    /// Example: logging hook that overrides BOTH post_tool_use variants.
    struct LoggingHook {
        success_count: AtomicU32,
        failure_count: AtomicU32,
    }

    #[async_trait]
    impl Hook for LoggingHook {
        async fn post_tool_use(&self, _ctx: &PostToolUseCtx) -> HookResult {
            self.success_count.fetch_add(1, Ordering::SeqCst);
            HookResult::cont()
        }
        async fn post_tool_use_failure(&self, _ctx: &PostToolUseFailureCtx) -> HookResult {
            self.failure_count.fetch_add(1, Ordering::SeqCst);
            HookResult::cont()
        }
    }

    fn tool_call() -> ToolCall {
        ToolCall {
            id: "c1".into(),
            name: "x".into(),
            input: json!({}),
        }
    }

    #[tokio::test]
    async fn logging_hook_counts_both_outcomes() {
        let h = LoggingHook {
            success_count: AtomicU32::new(0),
            failure_count: AtomicU32::new(0),
        };
        let token = CancellationToken::new();
        let ok = PostToolUseCtx {
            session_id: "s".into(),
            tool_call: tool_call(),
            tool_result: ToolResult::text("c1", "ok"),
            cancellation_token: token.clone(),
        };
        let fail = PostToolUseFailureCtx {
            session_id: "s".into(),
            tool_call: tool_call(),
            failure: ToolFailure::Cancelled,
            result: ToolResult::error("c1", "cancelled"),
            cancellation_token: token.clone(),
        };
        h.post_tool_use(&ok).await;
        h.post_tool_use_failure(&fail).await;
        assert_eq!(h.success_count.load(Ordering::SeqCst), 1);
        assert_eq!(h.failure_count.load(Ordering::SeqCst), 1);
    }

    struct RewriteHook;
    #[async_trait]
    impl Hook for RewriteHook {
        async fn pre_tool_use(&self, _ctx: &PreToolUseCtx) -> HookResult {
            HookResult::rewrite(json!({ "rewritten": true }))
        }
    }

    #[tokio::test]
    async fn rewrite_hook_returns_updated_input() {
        let h = RewriteHook;
        let ctx = PreToolUseCtx {
            session_id: "s".into(),
            tool_call: tool_call(),
            cancellation_token: CancellationToken::new(),
        };
        match h.pre_tool_use(&ctx).await {
            HookResult::Continue {
                updated_input: Some(v),
            } => assert_eq!(v, json!({ "rewritten": true })),
            other => panic!("expected Continue/updated_input, got {other:?}"),
        }
    }

    #[test]
    fn hook_result_round_trips() {
        for r in [
            HookResult::cont(),
            HookResult::rewrite(json!({ "k": 1 })),
            HookResult::Skip {
                reason: "no".into(),
            },
            HookResult::Abort {
                reason: "stop".into(),
            },
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: HookResult = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back);
        }
    }

    #[test]
    fn stop_reason_round_trips() {
        for r in [
            StopReason::Completed,
            StopReason::UserCancelled,
            StopReason::AbortedByHook { reason: "x".into() },
            StopReason::BudgetExhausted,
            StopReason::Error {
                message: "boom".into(),
            },
        ] {
            let s = serde_json::to_string(&r).unwrap();
            let back: StopReason = serde_json::from_str(&s).unwrap();
            assert_eq!(r, back);
        }
    }

    /// Sanity-check: every Hook method has a default that returns Continue.
    #[tokio::test]
    async fn defaults_return_continue() {
        struct NullHook;
        #[async_trait]
        impl Hook for NullHook {}
        let h = NullHook;
        let token = CancellationToken::new();
        let _ = Role::User; // keep import used for tests below if needed
        assert_eq!(
            h.session_start(&SessionStartCtx {
                session_id: "s".into(),
                cancellation_token: token.clone(),
            })
            .await,
            HookResult::cont()
        );
    }

    /// M10 D-M10-2: PostToolUseFailureCtx now carries the final ToolResult
    /// the model sees alongside the failure enum. Audit hooks can read it
    /// directly instead of synthesizing one from `failure`.
    #[test]
    fn post_tool_use_failure_ctx_exposes_result() {
        let token = CancellationToken::new();
        let res = ToolResult::error("c1", "boom");
        let ctx = PostToolUseFailureCtx {
            session_id: "s".into(),
            tool_call: tool_call(),
            failure: ToolFailure::Error {
                message: "boom".into(),
            },
            result: res.clone(),
            cancellation_token: token,
        };
        assert_eq!(ctx.result, res);
        assert!(ctx.result.is_error);
        assert_eq!(ctx.result.tool_use_id, "c1");
    }
}
