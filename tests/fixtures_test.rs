//! Verifies that the handwritten JSON fixtures in `tests/fixtures/` parse
//! into the expected [`ContentBlock`] variants and round-trip through serde.

use motosan_agent_primitives::message::ContentBlock;

fn load(name: &str) -> String {
    let path = format!("tests/fixtures/content_blocks/{name}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn parse(name: &str) -> ContentBlock {
    let raw = load(name);
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parse {name}: {e}"))
}

#[test]
fn text_fixture_parses() {
    assert!(matches!(parse("text.json"), ContentBlock::Text { .. }));
}

#[test]
fn image_base64_fixture_parses() {
    assert!(matches!(parse("image_base64.json"), ContentBlock::Image { .. }));
}

#[test]
fn image_url_fixture_parses() {
    assert!(matches!(parse("image_url.json"), ContentBlock::Image { .. }));
}

#[test]
fn document_fixture_parses() {
    assert!(matches!(parse("document.json"), ContentBlock::Document { .. }));
}

#[test]
fn tool_use_fixture_parses() {
    assert!(matches!(parse("tool_use.json"), ContentBlock::ToolUse { .. }));
}

#[test]
fn tool_result_fixture_parses() {
    let b = parse("tool_result.json");
    match b {
        ContentBlock::ToolResult { is_error, .. } => assert!(!is_error),
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn tool_result_error_fixture_parses() {
    let b = parse("tool_result_error.json");
    match b {
        ContentBlock::ToolResult { is_error, .. } => assert!(is_error),
        _ => panic!("expected ToolResult"),
    }
}

#[test]
fn all_fixtures_round_trip() {
    for name in [
        "text.json",
        "image_base64.json",
        "image_url.json",
        "document.json",
        "tool_use.json",
        "tool_result.json",
        "tool_result_error.json",
    ] {
        let parsed: ContentBlock = serde_json::from_str(&load(name)).unwrap();
        let reserialized = serde_json::to_string(&parsed).unwrap();
        let reparsed: ContentBlock = serde_json::from_str(&reserialized).unwrap();
        assert_eq!(parsed, reparsed, "{name} did not round-trip");
    }
}
