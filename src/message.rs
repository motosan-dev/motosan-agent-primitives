//! Conversation message types.
//!
//! [`Message`] is the shape of an LLM-conversation turn as it flows through
//! the Motosan stack: the agent loop, provider adapters, tools, and hooks
//! all agree on this representation.
//!
//! A [`Message`] has a [`Role`] (`system` / `user` / `assistant` / `tool`)
//! and a sequence of [`ContentBlock`] entries. Content is intentionally
//! multimodal from the start — text, images, documents, tool calls, and
//! tool results all share one enum so that providers can be added without
//! changing the surface API.
//!
//! # Examples
//!
//! ```
//! use motosan_agent_primitives::message::{Message, Role};
//!
//! let m = Message::text(Role::User, "What's the weather?");
//! assert_eq!(m.role, Role::User);
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

/// Stable identifier for a [`Message`].
///
/// Wraps a [`Uuid`] (v4) so messages can be referenced from events, logs,
/// and persisted transcripts without depending on positional ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(pub Uuid);

impl MessageId {
    /// Generate a fresh random `MessageId`.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for MessageId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Who authored a [`Message`].
///
/// Mirrors the OpenAI / Anthropic role taxonomy:
///
/// - [`Role::System`] — instruction prompt from the framework / harness.
/// - [`Role::User`] — input from the human or upstream agent.
/// - [`Role::Assistant`] — model output (may include tool calls).
/// - [`Role::Tool`] — output from a tool invocation, replying to an
///   assistant `tool_use` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// System / instruction prompt.
    System,
    /// User-authored input.
    User,
    /// Model-authored response.
    Assistant,
    /// Tool execution result.
    Tool,
}

/// Source of an image payload referenced from a [`ContentBlock::Image`].
///
/// Either a base64-encoded inline blob with a declared media type, or a URL
/// the provider is expected to fetch on the model's behalf.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ImageSource {
    /// Base64-encoded inline image bytes.
    Base64 {
        /// IANA media type (`image/png`, `image/jpeg`, ...).
        media_type: String,
        /// Base64-encoded image data (no `data:` prefix).
        data: String,
    },
    /// URL the provider should fetch.
    Url {
        /// Absolute URL pointing at the image.
        url: String,
    },
}

/// Source of a document payload referenced from a [`ContentBlock::Document`].
///
/// Mirrors [`ImageSource`] but for PDFs / structured documents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DocumentSource {
    /// Base64-encoded inline document bytes.
    Base64 {
        /// IANA media type (`application/pdf`, ...).
        media_type: String,
        /// Base64-encoded document data.
        data: String,
    },
    /// URL the provider should fetch.
    Url {
        /// Absolute URL pointing at the document.
        url: String,
    },
}

/// One slice of multimodal content inside a [`Message`].
///
/// All variants serialize with an internal `type` tag so providers can
/// reorder / mix freely without ambiguity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain UTF-8 text.
    Text {
        /// The text payload.
        text: String,
    },
    /// An image attachment.
    Image {
        /// Where the image bytes come from.
        source: ImageSource,
    },
    /// A document (e.g. PDF) attachment.
    Document {
        /// Where the document bytes come from.
        source: DocumentSource,
    },
    /// An assistant-authored request to invoke a tool.
    ///
    /// Carries the same shape as
    /// [`ToolCall`](crate::tool::ToolCall); duplicated as a content block
    /// because providers interleave it with text.
    ToolUse {
        /// Provider-assigned unique id for this invocation (echoed in the
        /// matching [`ContentBlock::ToolResult`]).
        id: String,
        /// Registered tool name.
        name: String,
        /// JSON arguments — the tool's input schema decides the shape.
        input: serde_json::Value,
    },
    /// A `tool`-role reply pairing with an earlier [`ContentBlock::ToolUse`].
    ToolResult {
        /// The `id` from the originating `ToolUse` block.
        tool_use_id: String,
        /// Result content. Typically `[ContentBlock::Text { … }]`.
        content: Vec<ContentBlock>,
        /// `true` if this represents a tool error (model should treat the
        /// content as a diagnostic, not a normal result).
        #[serde(default, skip_serializing_if = "is_false")]
        is_error: bool,
    },
    /// Structured JSON content that downstream processors (redact, citation
    /// extractors, validators) can walk recursively without re-parsing a
    /// string. Use this when the tool result is genuinely structured data
    /// rather than text; for opaque text, prefer [`ContentBlock::Text`].
    ///
    /// Wire-format tag: `"json"` (auto-derived from the enum's serde
    /// `rename_all = "snake_case"`).
    Json {
        /// The JSON payload — serialized as a normal JSON tree, NOT as a
        /// string. A consumer can `match` on this variant and walk
        /// `value` directly.
        value: serde_json::Value,
    },
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One turn in an LLM conversation.
///
/// Held flat — no nesting, no provider-specific shape. The agent loop owns
/// the transcript ordering; this type just describes one entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Stable id for this message.
    #[serde(default)]
    pub id: MessageId,
    /// Who authored the message.
    pub role: Role,
    /// Multimodal content blocks, in order.
    pub content: Vec<ContentBlock>,
    /// Wall-clock timestamp of when the message was created.
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
}

impl Message {
    /// Build a message containing a single text block.
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            id: MessageId::new(),
            role,
            content: vec![ContentBlock::Text { text: text.into() }],
            created_at: Utc::now(),
        }
    }

    /// Build an assistant message composed of one or more tool calls
    /// (`tool_use` blocks).
    ///
    /// Each `(id, name, input)` triple becomes a [`ContentBlock::ToolUse`].
    pub fn tool_calls<I, S1, S2>(calls: I) -> Self
    where
        I: IntoIterator<Item = (S1, S2, serde_json::Value)>,
        S1: Into<String>,
        S2: Into<String>,
    {
        let content = calls
            .into_iter()
            .map(|(id, name, input)| ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input,
            })
            .collect();
        Self {
            id: MessageId::new(),
            role: Role::Assistant,
            content,
            created_at: Utc::now(),
        }
    }

    /// Build a tool-role message replying to a batch of previous tool calls.
    ///
    /// Each `(tool_use_id, text)` pair becomes a [`ContentBlock::ToolResult`]
    /// wrapping a single text block. For richer payloads construct
    /// [`ContentBlock::ToolResult`] manually.
    pub fn tool_results<I, S1, S2>(results: I) -> Self
    where
        I: IntoIterator<Item = (S1, S2)>,
        S1: Into<String>,
        S2: Into<String>,
    {
        let content = results
            .into_iter()
            .map(|(tool_use_id, text)| ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: vec![ContentBlock::Text { text: text.into() }],
                is_error: false,
            })
            .collect();
        Self {
            id: MessageId::new(),
            role: Role::Tool,
            content,
            created_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn round_trip(block: &ContentBlock) -> ContentBlock {
        let s = serde_json::to_string(block).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn text_block_round_trip() {
        let b = ContentBlock::Text {
            text: "hello".into(),
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn image_block_round_trip_base64() {
        let b = ContentBlock::Image {
            source: ImageSource::Base64 {
                media_type: "image/png".into(),
                data: "iVBORw0KGgo=".into(),
            },
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn image_block_round_trip_url() {
        let b = ContentBlock::Image {
            source: ImageSource::Url {
                url: "https://example.com/cat.png".into(),
            },
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn document_block_round_trip() {
        let b = ContentBlock::Document {
            source: DocumentSource::Base64 {
                media_type: "application/pdf".into(),
                data: "JVBERi0=".into(),
            },
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn tool_use_round_trip() {
        let b = ContentBlock::ToolUse {
            id: "call_1".into(),
            name: "lookup".into(),
            input: json!({ "q": "weather" }),
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn tool_result_round_trip() {
        let b = ContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: vec![ContentBlock::Text {
                text: "sunny".into(),
            }],
            is_error: false,
        };
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn tool_result_error_round_trip() {
        let b = ContentBlock::ToolResult {
            tool_use_id: "call_1".into(),
            content: vec![ContentBlock::Text {
                text: "boom".into(),
            }],
            is_error: true,
        };
        let s = serde_json::to_string(&b).unwrap();
        assert!(s.contains("\"is_error\":true"));
        assert_eq!(round_trip(&b), b);
    }

    #[test]
    fn message_text_constructor() {
        let m = Message::text(Role::User, "hi");
        assert_eq!(m.role, Role::User);
        assert_eq!(m.content.len(), 1);
    }

    #[test]
    fn message_tool_calls_constructor() {
        let m = Message::tool_calls([("c1", "n", json!({}))]);
        assert_eq!(m.role, Role::Assistant);
        match &m.content[0] {
            ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "c1");
                assert_eq!(name, "n");
            }
            _ => panic!("expected ToolUse"),
        }
    }

    #[test]
    fn message_tool_results_constructor() {
        let m = Message::tool_results([("c1", "ok")]);
        assert_eq!(m.role, Role::Tool);
        match &m.content[0] {
            ContentBlock::ToolResult { tool_use_id, .. } => {
                assert_eq!(tool_use_id, "c1");
            }
            _ => panic!("expected ToolResult"),
        }
    }

    #[test]
    fn message_round_trip() {
        let m = Message::text(Role::User, "ping");
        let s = serde_json::to_string(&m).unwrap();
        let back: Message = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn content_block_json_round_trip() {
        use serde_json::json;
        let cb = ContentBlock::Json {
            value: json!({ "user": "alice", "score": 42, "tags": ["a", "b"] }),
        };
        let s = serde_json::to_string(&cb).unwrap();
        let back: ContentBlock = serde_json::from_str(&s).unwrap();
        assert_eq!(cb, back);
    }

    #[test]
    fn content_block_json_serde_tag() {
        use serde_json::json;
        let cb = ContentBlock::Json {
            value: json!({"k": 1}),
        };
        let v = serde_json::to_value(&cb).unwrap();
        assert_eq!(v["type"], "json");
        // value is the JSON tree itself, not a string:
        assert_eq!(v["value"]["k"], 1);
    }
}
