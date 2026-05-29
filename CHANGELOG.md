## 0.2.0 — 2026-05-29

First deliberately breaking release since 0.1.0. Both changes are additive
field adds to context structs that hook / policy implementations read; they
do **not** alter the [`Hook`] or [`PermissionPolicy`] trait method signatures.
Migration cost is per literal struct constructor (test fixtures, internal
adapters), not per impl. See [M10_PLAN.md](M10_PLAN.md) §3 D-M10-2 and
§3 D-M10-3 for the rationale and the FinanceHarness consumer feedback
(AWKWARDNESS items #3 and #4, M9_GATE_DIAGRAM §5 item #4) that drove these.

BREAKING:
- `PostToolUseFailureCtx` gained `pub result: ToolResult` (M10 D-M10-2,
  closes AWKWARDNESS #3 + M9_GATE #1 partial). The failure path now carries
  the same wire shape the model sees, so audit hooks can record
  `ctx.result` directly instead of synthesizing a `ToolResult` from
  `ctx.failure`. Owned (not borrowed) to match the existing
  `PostToolUseCtx.tool_result` pattern. All in-tree literal constructors
  (test fixtures) must add the new field.
- `PermissionContext<'a>` gained `pub recent_messages: &'a [Message]` (M10
  D-M10-3, closes M9_GATE #4). Policies can now inspect the recent
  conversation slice when deciding or when rendering an approval prompt,
  removing the prior workaround of forcing tools to redundantly include
  context in their args. Borrowed slice keeps the struct zero-alloc and
  preserves the existing `'a` lifetime. Empty slice is valid (cold start
  / opt-out). The framework (loop) chooses the window size; the trait
  contract is just "a slice the policy may read."

## 0.1.1 — 2026-05-26

ADDED:
- `ContentBlock::Json { value: serde_json::Value }` variant. Wire tag: `"json"`. Use this for structured tool results so downstream processors can walk the JSON tree without re-parsing a string.
