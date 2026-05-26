//! Memory **schema** declaration (no storage).
//!
//! A `Harness` (defined in the future `motosan-agent-harness` crate) can declare which
//! long-term context keys it expects to read or write via a
//! [`MemorySchema`]. Storage backends, fetch policies, summarisation —
//! none of that lives here. This crate only owns the schema vocabulary so
//! schemas serialize across crate boundaries.
//!
//! Per decision D8 = A this is included even though no implementation
//! consumes it yet; the cost is small and the value (Harness can advertise
//! its memory shape) is real.
//!
//! # Examples
//!
//! ```
//! use motosan_agent_primitives::memory::{MemoryKey, MemoryKind, MemorySchema};
//!
//! let schema = MemorySchema {
//!     keys: vec![
//!         MemoryKey {
//!             name: "user_profile".into(),
//!             kind: MemoryKind::PerUser,
//!             description: "Stable user preferences across sessions.".into(),
//!         },
//!         MemoryKey {
//!             name: "scratchpad".into(),
//!             kind: MemoryKind::PerSession,
//!             description: "Notes the agent writes during one run.".into(),
//!         },
//!     ],
//! };
//! let json = serde_json::to_string(&schema).unwrap();
//! let back: MemorySchema = serde_json::from_str(&json).unwrap();
//! assert_eq!(back.keys.len(), 2);
//! ```

use serde::{Deserialize, Serialize};

/// Lifetime classification of a memory key.
///
/// Used by future storage backends to decide where a value lives. Schema
/// declarations in this crate are pure metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// Lives only for the current session; discarded when the session ends.
    PerSession,
    /// Tied to a specific user across all their sessions.
    PerUser,
    /// Shared across all sessions and users for this deployment.
    Global,
}

/// One declared memory slot.
///
/// `name` must be unique within a [`MemorySchema`]; the schema is purely
/// documentation until a storage backend reifies it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryKey {
    /// Identifier the harness uses when reading / writing this slot.
    pub name: String,
    /// Lifetime classification.
    pub kind: MemoryKind,
    /// Free-form description; surfaced in UIs and logs.
    pub description: String,
}

/// The full memory contract a harness declares.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySchema {
    /// Declared memory keys.
    pub keys: Vec<MemoryKey>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_schema() {
        let s = MemorySchema {
            keys: vec![MemoryKey {
                name: "k".into(),
                kind: MemoryKind::Global,
                description: "d".into(),
            }],
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: MemorySchema = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }

    #[test]
    fn memory_kind_serializes_snake_case() {
        let s = serde_json::to_string(&MemoryKind::PerSession).unwrap();
        assert_eq!(s, "\"per_session\"");
        let s = serde_json::to_string(&MemoryKind::PerUser).unwrap();
        assert_eq!(s, "\"per_user\"");
        let s = serde_json::to_string(&MemoryKind::Global).unwrap();
        assert_eq!(s, "\"global\"");
    }

    #[test]
    fn empty_schema_round_trip() {
        let s = MemorySchema::default();
        let json = serde_json::to_string(&s).unwrap();
        let back: MemorySchema = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
