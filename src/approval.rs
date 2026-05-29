//! Approval resolution contract — the answering half of `Permission::AskUser`.
//!
//! [`PermissionPolicy`](crate::permission::PermissionPolicy) *decides*
//! (`Allow | Deny | AskUser`). When the composed decision is `AskUser`, the
//! framework consults the session's single `Reviewer` to *resolve* it into a
//! final `ReviewDecision`. See the design spec for the full rationale.

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::message::Message;
use crate::tool::{ToolAnnotations, ToolCall};

/// Final verdict produced by a reviewer for an escalated tool call.
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

/// Everything a reviewer needs to resolve one escalated tool call.
///
/// **Owned, not borrowed** (cf. [`PermissionContext<'_>`](crate::permission::PermissionContext)):
/// a reviewer may queue this, move it to another task/thread, or hold it across
/// a long guardian turn, so it cannot borrow from the engine's transient state.
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

#[cfg(test)]
mod request_tests {
    use super::*;
    use crate::message::Role;
    use crate::tool::{ToolAnnotations, ToolCall};
    use tokio_util::sync::CancellationToken;

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

    #[test]
    fn approval_request_is_cloneable_and_retains_data() {
        let req = sample_request();
        let copy = req.clone();
        assert_eq!(copy.tool_call.name, "place_order");
        assert_eq!(req.recent_messages.len(), 1);
        assert_eq!(req.prompt.as_deref(), Some("Approve buying 10 AAPL?"));
        fn _assert_send_static<T: Send + 'static>() {}
        _assert_send_static::<ApprovalRequest>();
    }
}
