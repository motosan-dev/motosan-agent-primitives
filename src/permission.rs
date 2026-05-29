//! Permission policy contracts.
//!
//! A session has exactly one [`PermissionMode`] (its current "trust level",
//! e.g. plan vs accept-edits) and exactly one composed
//! [`PermissionPolicy`] (the rule object the agent loop consults before
//! every tool call).
//!
//! # Composition (decision D3 = A — most-restrictive wins)
//!
//! When multiple stacked harnesses each declare a policy, the framework
//! composes them as **most-restrictive wins** (order-independent):
//!
//! 1. If any policy returns [`Permission::Deny`], the call is denied.
//! 2. Otherwise, if any policy returns [`Permission::AskUser`], the user
//!    is asked.
//! 3. Otherwise, if all return [`Permission::Allow`], the call is allowed.
//!
//! Composition logic itself lives in `motosan-agent-loop` (or wherever
//! composition is actually performed). This crate just defines the
//! contract and the deterministic ordering above.
//!
//! # Plan mode (decision D4 = C)
//!
//! See [`PermissionMode::Plan`] for the full rule. The short version: plan
//! mode denies only `destructive` tools; read-only and network access are
//! allowed so the agent can fetch documentation and browse while planning.
//! This is more permissive than the original "deny network too" proposal
//! and shifts correctness onto tool-author annotations — see
//! [`ToolAnnotations`].

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::message::Message;
use crate::tool::ToolAnnotations;

/// Outcome of one permission check.
///
/// The agent loop maps these as follows:
///
/// - [`Allow`](Self::Allow) — execute the tool.
/// - [`Deny`](Self::Deny) — refuse the call; surface `reason` to the model.
/// - [`AskUser`](Self::AskUser) — pause the loop and prompt the human via
///   the configured ask-user channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Permission {
    /// Tool may run.
    Allow,
    /// Tool must not run; `reason` is shown to the model.
    Deny {
        /// Human-readable explanation.
        reason: String,
    },
    /// Human approval required before running.
    AskUser {
        /// Optional prompt text shown to the user.
        prompt: Option<String>,
    },
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Permission::Allow => write!(f, "allow"),
            Permission::Deny { reason } => write!(f, "deny: {reason}"),
            Permission::AskUser { prompt } => match prompt {
                Some(p) => write!(f, "ask_user: {p}"),
                None => write!(f, "ask_user"),
            },
        }
    }
}

/// Current trust mode of the session.
///
/// Changes only on explicit user action (slash command, UI toggle). The
/// agent loop reads this on every tool dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// "Read-only sandbox while drafting a plan."
    ///
    /// **Rule (decision D4 = C):** denies tools whose
    /// [`ToolAnnotations::destructive`](crate::tool::ToolAnnotations::destructive)
    /// flag is `true`. Tools with `destructive: false` are allowed even
    /// when `network_access: true` — the framework deliberately permits
    /// documentation fetches and HTTP `GET` while planning.
    ///
    /// **⚠️ Load-bearing trust statement:** plan mode does **not** infer
    /// destructiveness from `network_access`. A tool declared with
    /// `destructive: false` that actually performs a network mutation
    /// (HTTP `POST`, irreversible side effect, money movement, …) **will
    /// run in plan mode**. Tool authors are responsible for accurate
    /// annotations; see the type-level warning on
    /// [`ToolAnnotations`]. When the
    /// destructive flag is wrong, plan mode is unsafe.
    Plan,
    /// User must approve every non-read-only tool individually.
    AcceptEdits,
    /// Anything goes (interactive shells, free-running agents). Use with
    /// caution; the policy is bypassed except for sandbox-level limits.
    BypassPermissions,
}

/// Inputs to one [`PermissionPolicy::check`] call.
///
/// Carries everything a policy needs to make its decision without forcing
/// it to depend on the `Tool` trait or look anything up. The policy reads
/// these fields, returns a [`Permission`], and is done.
#[derive(Debug, Clone)]
pub struct PermissionContext<'a> {
    /// Current session id.
    pub session_id: &'a str,
    /// Provider-assigned id for the tool call being checked.
    pub tool_use_id: &'a str,
    /// Name of the tool being invoked.
    pub tool_name: &'a str,
    /// JSON arguments that will be passed to the tool.
    pub tool_input: &'a serde_json::Value,
    /// Annotations declared by the tool. See
    /// [`ToolAnnotations`] for the
    /// destructive-annotation correctness contract.
    pub annotations: &'a ToolAnnotations,
    /// Current session trust mode.
    pub mode: PermissionMode,
    /// Recent conversation history the policy may inspect when rendering an
    /// approval prompt or deciding whether the call is contextually
    /// reasonable.
    ///
    /// Borrowed to keep the struct zero-alloc; the slice points into the
    /// agent loop's transcript and is valid for the duration of the
    /// [`check`](PermissionPolicy::check) call. The framework chooses the
    /// window size (default 10 most recent messages); policies must treat
    /// an empty slice as a normal case (e.g. cold start, no history yet).
    /// Added in 0.2.0 (M10 D-M10-3) — see CHANGELOG.
    pub recent_messages: &'a [Message],
}

/// A pluggable rule object that decides whether a tool call may run.
///
/// One policy per session — when stacked harnesses each contribute a
/// policy, the framework composes them per the rules at the module
/// level. Implementations should be cheap; the agent loop calls `check`
/// before every tool invocation.
///
/// Object-safe: usable as `Arc<dyn PermissionPolicy>`.
///
/// # Examples
///
/// Implement a simple policy that distinguishes the three outcomes:
///
/// ```no_run
/// use async_trait::async_trait;
/// use motosan_agent_primitives::permission::{
///     Permission, PermissionContext, PermissionPolicy,
/// };
///
/// struct DemoPolicy;
///
/// #[async_trait]
/// impl PermissionPolicy for DemoPolicy {
///     async fn check(&self, ctx: &PermissionContext<'_>) -> Permission {
///         if ctx.annotations.destructive {
///             // Deny case
///             Permission::Deny { reason: "destructive blocked".into() }
///         } else if ctx.annotations.network_access {
///             // AskUser case
///             Permission::AskUser { prompt: Some("hit network?".into()) }
///         } else {
///             // Allow case
///             Permission::Allow
///         }
///     }
/// }
/// ```
#[async_trait]
pub trait PermissionPolicy: Send + Sync {
    /// Decide whether the tool call described by `ctx` may run.
    async fn check(&self, ctx: &PermissionContext<'_>) -> Permission;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    /// Object-safety smoke test: must compile.
    #[allow(dead_code)]
    fn assert_object_safe(_p: Arc<dyn PermissionPolicy>) {}

    struct AllowAll;
    #[async_trait]
    impl PermissionPolicy for AllowAll {
        async fn check(&self, _ctx: &PermissionContext<'_>) -> Permission {
            Permission::Allow
        }
    }

    struct DenyDestructive;
    #[async_trait]
    impl PermissionPolicy for DenyDestructive {
        async fn check(&self, ctx: &PermissionContext<'_>) -> Permission {
            if ctx.annotations.destructive {
                Permission::Deny {
                    reason: "destructive tools blocked".into(),
                }
            } else {
                Permission::Allow
            }
        }
    }

    fn ctx<'a>(
        annotations: &'a ToolAnnotations,
        input: &'a serde_json::Value,
        mode: PermissionMode,
    ) -> PermissionContext<'a> {
        PermissionContext {
            session_id: "s",
            tool_use_id: "t",
            tool_name: "n",
            tool_input: input,
            annotations,
            mode,
            recent_messages: &[],
        }
    }

    #[tokio::test]
    async fn allow_all_returns_allow() {
        let p: Arc<dyn PermissionPolicy> = Arc::new(AllowAll);
        let ann = ToolAnnotations::default();
        let inp = json!({});
        let c = ctx(&ann, &inp, PermissionMode::Plan);
        assert_eq!(p.check(&c).await, Permission::Allow);
    }

    #[tokio::test]
    async fn deny_destructive_blocks() {
        let p: Arc<dyn PermissionPolicy> = Arc::new(DenyDestructive);
        let ann = ToolAnnotations {
            destructive: true,
            ..Default::default()
        };
        let inp = json!({});
        let c = ctx(&ann, &inp, PermissionMode::Plan);
        match p.check(&c).await {
            Permission::Deny { .. } => {}
            other => panic!("expected Deny, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ask_user_round_trip() {
        let p = Permission::AskUser {
            prompt: Some("ok?".into()),
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: Permission = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn permission_modes_serialize_snake_case() {
        let modes = [
            PermissionMode::Plan,
            PermissionMode::AcceptEdits,
            PermissionMode::BypassPermissions,
        ];
        for m in modes {
            let s = serde_json::to_string(&m).unwrap();
            let back: PermissionMode = serde_json::from_str(&s).unwrap();
            assert_eq!(m, back);
        }
    }

    /// M10 D-M10-3: PermissionContext now carries a borrowed slice of
    /// recent messages so a policy can render a context-aware approval
    /// prompt without forcing tools to redundantly include conversation
    /// state in their args.
    #[tokio::test]
    async fn permission_context_exposes_recent_messages() {
        use crate::message::Role;

        struct InspectsHistory;
        #[async_trait]
        impl PermissionPolicy for InspectsHistory {
            async fn check(&self, ctx: &PermissionContext<'_>) -> Permission {
                if ctx.recent_messages.is_empty() {
                    Permission::AskUser { prompt: None }
                } else {
                    Permission::Allow
                }
            }
        }

        let ann = ToolAnnotations::default();
        let inp = json!({});
        let history = vec![
            Message::text(Role::User, "buy 10 AAPL"),
            Message::text(Role::Assistant, "confirming order"),
        ];
        let c = PermissionContext {
            session_id: "s",
            tool_use_id: "t",
            tool_name: "place_order",
            tool_input: &inp,
            annotations: &ann,
            mode: PermissionMode::AcceptEdits,
            recent_messages: &history,
        };
        assert_eq!(c.recent_messages.len(), 2);
        let p: Arc<dyn PermissionPolicy> = Arc::new(InspectsHistory);
        assert_eq!(p.check(&c).await, Permission::Allow);

        // Empty slice is a valid input (cold start case).
        let empty = PermissionContext {
            session_id: "s",
            tool_use_id: "t",
            tool_name: "place_order",
            tool_input: &inp,
            annotations: &ann,
            mode: PermissionMode::AcceptEdits,
            recent_messages: &[],
        };
        match p.check(&empty).await {
            Permission::AskUser { .. } => {}
            other => panic!("expected AskUser on empty history, got {other:?}"),
        }
    }
}
