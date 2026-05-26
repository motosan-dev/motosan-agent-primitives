## 0.1.1 — 2026-05-26

ADDED:
- `ContentBlock::Json { value: serde_json::Value }` variant. Wire tag: `"json"`. Use this for structured tool results so downstream processors can walk the JSON tree without re-parsing a string.
