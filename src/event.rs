//! Streaming output events emitted by the agent loop.
//!
//! [`AgentEvent`] is the wire format for "what is the agent doing right
//! now". The agent loop emits a stream of these (over a channel, an SSE
//! pipe, stdout JSONL, …) and downstream consumers — UIs, logs, evaluators
//! — react to them.
//!
//! Variants serialize with an internal `event` tag so consumers can match
//! by string without needing to know every variant.
//!
//! # Examples
//!
//! ```
//! use motosan_agent_primitives::event::AgentEvent;
//! use motosan_agent_primitives::hook::StopReason;
//!
//! let ev = AgentEvent::AgentStop {
//!     session_id: "s".into(),
//!     reason: StopReason::Completed,
//! };
//! let line = serde_json::to_string(&ev).unwrap();
//! // Each JSONL line is a self-contained event.
//! assert!(line.contains("\"event\":\"agent_stop\""));
//! ```
//!
//! Streaming consumption — drive a UI / log from an `AgentEvent` stream:
//!
//! ```
//! use motosan_agent_primitives::event::AgentEvent;
//! use motosan_agent_primitives::hook::StopReason;
//!
//! fn render(stream: impl IntoIterator<Item = AgentEvent>) -> String {
//!     let mut out = String::new();
//!     for ev in stream {
//!         match ev {
//!             AgentEvent::MessageDelta { text, .. } => out.push_str(&text),
//!             AgentEvent::AgentStop { reason: StopReason::Completed, .. } => {
//!                 out.push_str("\n[done]");
//!             }
//!             _ => {} // ignore the rest for this example
//!         }
//!     }
//!     out
//! }
//! ```

use serde::{Deserialize, Serialize};

use crate::hook::StopReason;
use crate::message::{Message, MessageId, Role};
use crate::tool::{ToolCall, ToolResult};

/// Result of a spawned subagent reported back via
/// [`AgentEvent::SubagentResult`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentResult {
    /// Subagent session id.
    pub session_id: String,
    /// Why the subagent stopped.
    pub stop_reason: StopReason,
    /// Optional final assistant message produced by the subagent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_message: Option<Message>,
}

/// One unit of streaming output from the agent loop.
///
/// Ten variants cover the full lifecycle: agent start/stop, message
/// start/delta/end (for token streaming), tool call/result, subagent
/// emission, ask-user prompt, and a generic error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Session started.
    AgentStart {
        /// Session id.
        session_id: String,
    },
    /// Session stopped.
    AgentStop {
        /// Session id.
        session_id: String,
        /// Why the loop stopped.
        reason: StopReason,
    },
    /// A new message is about to be streamed. `MessageDelta` events with
    /// the same `message_id` follow until `MessageEnd`.
    MessageStart {
        /// Session id.
        session_id: String,
        /// Stable id assigned to this message.
        message_id: MessageId,
        /// Author of the message.
        role: Role,
    },
    /// One incremental text chunk for an in-progress message.
    MessageDelta {
        /// Session id.
        session_id: String,
        /// Id of the message this chunk belongs to.
        message_id: MessageId,
        /// Newly appended text.
        text: String,
    },
    /// The message identified by `message_id` is now complete.
    MessageEnd {
        /// Session id.
        session_id: String,
        /// The fully assembled message.
        message: Message,
    },
    /// The agent has dispatched a tool call.
    ToolCallStart {
        /// Session id.
        session_id: String,
        /// The dispatched call (possibly after hook rewrite).
        tool_call: ToolCall,
    },
    /// A tool call finished and produced a result.
    ToolCallEnd {
        /// Session id.
        session_id: String,
        /// The reply to the matching [`AgentEvent::ToolCallStart`].
        tool_result: ToolResult,
    },
    /// A spawned subagent emitted its final result.
    SubagentResult {
        /// Parent session id.
        session_id: String,
        /// Subagent outcome.
        result: SubagentResult,
    },
    /// The agent is requesting human approval (e.g. from a
    /// [`PermissionPolicy`](crate::permission::PermissionPolicy::check)
    /// returning [`Permission::AskUser`](crate::permission::Permission::AskUser)).
    AskUser {
        /// Session id.
        session_id: String,
        /// Provider-assigned id of the tool call awaiting approval.
        tool_use_id: String,
        /// Prompt text to surface to the human.
        prompt: String,
    },
    /// A non-fatal error worth surfacing to consumers; the loop may
    /// continue. Fatal errors come through [`AgentEvent::AgentStop`] with
    /// [`StopReason::Error`].
    Error {
        /// Session id.
        session_id: String,
        /// Diagnostic message.
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ContentBlock, Message, Role};
    use serde_json::json;

    fn round_trip(ev: &AgentEvent) -> AgentEvent {
        let s = serde_json::to_string(ev).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn agent_start_shape() {
        let ev = AgentEvent::AgentStart {
            session_id: "s".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "agent_start");
        assert_eq!(round_trip(&ev), ev);
    }

    #[test]
    fn agent_stop_shape() {
        let ev = AgentEvent::AgentStop {
            session_id: "s".into(),
            reason: StopReason::Completed,
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "agent_stop");
        assert_eq!(round_trip(&ev), ev);
    }

    #[test]
    fn message_delta_shape() {
        let ev = AgentEvent::MessageDelta {
            session_id: "s".into(),
            message_id: MessageId::new(),
            text: "Hel".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "message_delta");
        assert_eq!(v["text"], "Hel");
        assert_eq!(round_trip(&ev), ev);
    }

    #[test]
    fn message_start_and_end_shape() {
        let mid = MessageId::new();
        let start = AgentEvent::MessageStart {
            session_id: "s".into(),
            message_id: mid,
            role: Role::Assistant,
        };
        assert_eq!(
            serde_json::to_value(&start).unwrap()["event"],
            "message_start"
        );
        let end = AgentEvent::MessageEnd {
            session_id: "s".into(),
            message: Message {
                id: mid,
                role: Role::Assistant,
                content: vec![ContentBlock::Text { text: "hi".into() }],
                created_at: chrono::Utc::now(),
            },
        };
        assert_eq!(serde_json::to_value(&end).unwrap()["event"], "message_end");
        assert_eq!(round_trip(&start), start);
        assert_eq!(round_trip(&end), end);
    }

    #[test]
    fn tool_call_events_shape() {
        let call = ToolCall {
            id: "c1".into(),
            name: "x".into(),
            input: json!({}),
        };
        let start = AgentEvent::ToolCallStart {
            session_id: "s".into(),
            tool_call: call.clone(),
        };
        let end = AgentEvent::ToolCallEnd {
            session_id: "s".into(),
            tool_result: ToolResult::text("c1", "ok"),
        };
        assert_eq!(
            serde_json::to_value(&start).unwrap()["event"],
            "tool_call_start"
        );
        assert_eq!(
            serde_json::to_value(&end).unwrap()["event"],
            "tool_call_end"
        );
        assert_eq!(round_trip(&start), start);
        assert_eq!(round_trip(&end), end);
    }

    #[test]
    fn subagent_result_shape() {
        let ev = AgentEvent::SubagentResult {
            session_id: "s".into(),
            result: SubagentResult {
                session_id: "sub".into(),
                stop_reason: StopReason::Completed,
                final_message: None,
            },
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap()["event"],
            "subagent_result"
        );
        assert_eq!(round_trip(&ev), ev);
    }

    #[test]
    fn ask_user_shape() {
        let ev = AgentEvent::AskUser {
            session_id: "s".into(),
            tool_use_id: "c1".into(),
            prompt: "ok?".into(),
        };
        assert_eq!(serde_json::to_value(&ev).unwrap()["event"], "ask_user");
        assert_eq!(round_trip(&ev), ev);
    }

    #[test]
    fn error_shape() {
        let ev = AgentEvent::Error {
            session_id: "s".into(),
            message: "boom".into(),
        };
        assert_eq!(serde_json::to_value(&ev).unwrap()["event"], "error");
        assert_eq!(round_trip(&ev), ev);
    }
}
