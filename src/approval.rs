//! Approval resolution contract — the answering half of `Permission::AskUser`.
//!
//! [`PermissionPolicy`](crate::permission::PermissionPolicy) *decides*
//! (`Allow | Deny | AskUser`). When the composed decision is `AskUser`, the
//! framework consults the session's single `Reviewer` to *resolve* it into a
//! final `ReviewDecision`. See the design spec for the full rationale.

use serde::{Deserialize, Serialize};

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
