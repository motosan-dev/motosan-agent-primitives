#![warn(missing_docs)]
//! # motosan-agent-primitives
//!
//! Contract layer of the Motosan agent framework — shared data types and
//! abstract middleware traits that every other crate in the family agrees on.
//!
//! This crate is the leaf of the dependency graph: it has no agent loop, no
//! LLM client, no sandbox, no provider SDKs. It depends only on the minimum
//! runtime primitives needed to define cancellable hooks (`tokio-util` for
//! [`tokio_util::sync::CancellationToken`]).
//!
//! ## Layering
//!
//! ```text
//!    ┌────────────────────────────────────────────┐
//!    │ motosan-agent-harness-{finance,rental,…}   │  vertical implementations
//!    ├────────────────────────────────────────────┤
//!    │ motosan-agent-loop      (ReAct engine)     │  runs Harness + Tools
//!    ├────────────────────────────────────────────┤
//!    │ motosan-agent-harness   (Harness trait)    │  composition contract
//!    ├────────────────────────────────────────────┤
//!    │ motosan-agent-tool │ motosan-ai │ sandbox  │  capability + infra
//!    ├────────────────────────────────────────────┤
//!    │ motosan-agent-primitives  ← THIS CRATE     │  shared types + Hook + Permission
//!    └────────────────────────────────────────────┘
//! ```
//!
//! ## What lives here
//!
//! - [`message`] — [`Message`], [`Role`], [`ContentBlock`] and friends
//! - [`tool`] — data-only [`ToolCall`], [`ToolResult`], [`ToolAnnotations`]
//!   (the `Tool` trait itself lives in `motosan-agent-tool`)
//! - [`permission`] — [`PermissionPolicy`] trait, [`Permission`],
//!   [`PermissionMode`], [`PermissionContext`]
//! - [`hook`] — [`Hook`] trait and the nine lifecycle context structs
//! - [`event`] — [`AgentEvent`] streaming output enum
//! - [`memory`] — [`MemorySchema`] declaration types (schema only, no storage)
//!
//! ## Stability
//!
//! `0.x` — API surface is iterating. Once two harnesses (finance + rental)
//! have been built against this crate the API will be frozen as `1.0`.

pub mod approval;
pub mod event;
pub mod hook;
pub mod memory;
pub mod message;
pub mod permission;
pub mod tool;

pub use approval::ReviewDecision;
pub use event::*;
pub use hook::*;
pub use memory::*;
pub use message::*;
pub use permission::*;
pub use tool::*;
