//! Tool **data types** (no trait).
//!
//! The actual `Tool` trait, `ToolContext`, `ToolOutput`, and `ToolError`
//! live in the `motosan-agent-tool` crate. Only the wire-format types that
//! other primitives (hooks, permission contexts, events) need to reference
//! live here:
//!
//! - [`ToolCall`] — assistant-side request to invoke a tool.
//! - [`ToolResult`] — tool-side reply to a [`ToolCall`].
//! - [`ToolAnnotations`] — capability metadata declared by a tool, read
//!   by [`PermissionPolicy`](crate::permission::PermissionPolicy).
//!
//! # Examples
//!
//! ```
//! use motosan_agent_primitives::tool::ToolCall;
//! use serde_json::json;
//!
//! let call = ToolCall {
//!     id: "call_1".into(),
//!     name: "get_weather".into(),
//!     input: json!({ "location": "Taipei" }),
//! };
//! let serialized = serde_json::to_string(&call).unwrap();
//! let back: ToolCall = serde_json::from_str(&serialized).unwrap();
//! assert_eq!(call, back);
//! ```

use serde::{Deserialize, Serialize};

use crate::message::ContentBlock;

/// An assistant-issued request to invoke a named tool.
///
/// Mirrors `ContentBlock::ToolUse` but as a stand-alone wire type — useful
/// when an event stream needs to emit "the model just asked for this tool
/// call" without quoting the whole content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Provider-assigned id, unique within the conversation. Echoed back
    /// in the matching [`ToolResult::tool_use_id`].
    pub id: String,
    /// Registered tool name. Must match a tool the executing harness has
    /// declared, otherwise the call is rejected before dispatch.
    pub name: String,
    /// JSON arguments. The tool's input schema decides the shape; this
    /// crate is schema-agnostic.
    pub input: serde_json::Value,
}

/// The reply pairing for a [`ToolCall`].
///
/// Always carries at least one [`ContentBlock`] of `content`. If `is_error`
/// is `true` the model should treat `content` as a diagnostic message
/// rather than a normal result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /// The `id` from the originating [`ToolCall`].
    pub tool_use_id: String,
    /// One or more content blocks describing the outcome. Typically a
    /// single [`ContentBlock::Text`].
    pub content: Vec<ContentBlock>,
    /// `true` if the tool failed (timeout, validation error, …).
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl ToolResult {
    /// Build a successful single-text result.
    pub fn text(tool_use_id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: vec![ContentBlock::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Build an error result with a diagnostic message.
    pub fn error(tool_use_id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tool_use_id: tool_use_id.into(),
            content: vec![ContentBlock::Text {
                text: message.into(),
            }],
            is_error: true,
        }
    }
}

/// Capability metadata a tool publishes about itself.
///
/// Read by [`PermissionPolicy`](crate::permission::PermissionPolicy) and by
/// downstream UIs that surface "this action will…" warnings before
/// executing a tool call.
///
/// # ⚠️ Load-bearing correctness warning — annotate `destructive` accurately
///
/// Tool authors **must** set [`destructive`](Self::destructive) honestly.
/// Under [`PermissionMode::Plan`](crate::permission::PermissionMode::Plan)
/// (decision D4 = C in the design doc), the framework allows tools with
/// `destructive: false` to run **even when `network_access: true`**. The
/// intent is to let plan-mode agents read docs / browse the web while
/// drafting a plan.
///
/// The risk: a tool that performs a network mutation (HTTP `POST` /
/// `DELETE`, irreversible API call, money movement, …) but is declared
/// with `destructive: false` will silently slip past plan mode and run.
/// If your tool mutates **anything** the user cares about — local files,
/// remote state, money, persistent data — set `destructive: true`. When
/// unsure, default to `true`; plan mode forgiving false positives is far
/// safer than missing false negatives.
///
/// See [`PermissionMode::Plan`](crate::permission::PermissionMode::Plan)
/// for the policy side of this contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolAnnotations {
    /// `true` if the tool only reads state; never writes anywhere.
    #[serde(default)]
    pub read_only: bool,
    /// `true` if the tool can mutate state the user would care about
    /// (local files, remote APIs that change resources, money, …).
    ///
    /// **Must be accurate** — see the type-level warning. Plan mode trusts
    /// this annotation to decide whether a tool may run.
    #[serde(default)]
    pub destructive: bool,
    /// `true` if the tool makes outbound network calls. Independent from
    /// `destructive`: a read-only HTTP GET is `network_access = true,
    /// destructive = false`; a `POST /orders` is both.
    #[serde(default)]
    pub network_access: bool,
    /// `true` if the tool may be re-invoked with the same input and is
    /// expected to produce the same result without additional side effects.
    /// Hint for caching / retry logic; not enforced.
    #[serde(default)]
    pub idempotent: bool,
}

impl Default for ToolAnnotations {
    /// The maximally cautious default: nothing claimed, treated as if it
    /// could do anything.
    fn default() -> Self {
        Self {
            read_only: false,
            destructive: false,
            network_access: false,
            idempotent: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_call_round_trip() {
        let c = ToolCall {
            id: "c1".into(),
            name: "lookup".into(),
            input: json!({ "q": 1 }),
        };
        let s = serde_json::to_string(&c).unwrap();
        let back: ToolCall = serde_json::from_str(&s).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn tool_result_text_constructor() {
        let r = ToolResult::text("c1", "ok");
        assert!(!r.is_error);
        assert_eq!(r.tool_use_id, "c1");
        match &r.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "ok"),
            _ => panic!("expected Text"),
        }
    }

    #[test]
    fn tool_result_error_constructor() {
        let r = ToolResult::error("c1", "boom");
        assert!(r.is_error);
    }

    #[test]
    fn tool_result_round_trip() {
        let r = ToolResult::text("c1", "hello");
        let s = serde_json::to_string(&r).unwrap();
        let back: ToolResult = serde_json::from_str(&s).unwrap();
        assert_eq!(r, back);
    }

    #[test]
    fn tool_result_error_field_omitted_when_false() {
        let r = ToolResult::text("c1", "ok");
        let s = serde_json::to_string(&r).unwrap();
        assert!(!s.contains("is_error"));
    }

    #[test]
    fn tool_annotations_round_trip() {
        let a = ToolAnnotations {
            read_only: true,
            destructive: false,
            network_access: true,
            idempotent: true,
        };
        let s = serde_json::to_string(&a).unwrap();
        let back: ToolAnnotations = serde_json::from_str(&s).unwrap();
        assert_eq!(a, back);
    }

    #[test]
    fn tool_annotations_default_is_cautious() {
        let a = ToolAnnotations::default();
        assert!(!a.read_only);
        assert!(!a.destructive);
        assert!(!a.network_access);
        assert!(!a.idempotent);
    }
}
