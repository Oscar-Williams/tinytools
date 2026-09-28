//! `<NAME><param>value</param></NAME>` element calls.

use crate::stream::StreamScrubber;
use crate::types::{CallSource, ParseDiagnostic, ParseOptions, ParseOutcome};
use crate::{PFormatRegistry, build_registry};
use std::sync::Arc;

const TODO: &str = "<todo>\n<todos>\n[{\"status\": \"in_progress\", \"description\": \"step one\"}, {\"status\": \"pending\", \"description\": \"step two\"}]\n</todos>\n</todo>";

fn registry() -> PFormatRegistry {
    build_registry([
        (
            "todo",
            serde_json::json!({"type": "object", "properties": {"todos": {"type": "array"}}}),
        ),
        (
            "tool_search",
            serde_json::json!({"type": "object", "properties": {"query": {"type": "string"}}}),
        ),
    ])
}

fn parse_with(text: &str, known: &[&str], registry: Option<&PFormatRegistry>) -> ParseOutcome {
    let known: Vec<String> = known.iter().map(ToString::to_string).collect();
    let mut options = ParseOptions::new().with_known_tools(&known);
    if let Some(registry) = registry {
        options = options.with_registry(registry);
    }
    crate::parse::parse_text(text, &options)
}

fn parse(text: &str) -> ParseOutcome {
    parse_with(text, &["todo", "tool_search"], Some(&registry()))
}

fn malformed(outcome: &ParseOutcome) -> usize {
    outcome
        .diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d,
                ParseDiagnostic::MalformedBlock {
                    source: CallSource::Element,
                    ..
                }
            )
        })
        .count()
}

#[test]
fn a_todo_element_call_is_decoded() {
    let outcome = parse(&format!("Planning.\n{TODO}"));
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "todo");
    assert_eq!(outcome.calls[0].source, CallSource::Element);
    assert_eq!(
        outcome.calls[0].arguments,
        serde_json::json!({"todos": [
            {"status": "in_progress", "description": "step one"},
            {"status": "pending", "description": "step two"}
        ]})
    );
    assert_eq!(outcome.text, "Planning.");
}

#[test]
fn a_todo_element_then_a_fenced_wrapped_invoke_yields_both_in_order() {
    let text = format!(
        "{TODO}\n```<tool_call>\n<invoke name=\"tool_search\">\n<parameter name=\"query\">repos</parameter>\n</invoke>\n</tool_call>"
    );
    let outcome = parse(&text);
    let names: Vec<&str> = outcome.calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["todo", "tool_search"]);
}

#[test]
fn an_element_for_an_unoffered_tool_is_left_alone() {
    let outcome = parse_with(TODO, &["tool_search"], Some(&registry()));
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert_eq!(outcome.text, TODO);
}

#[test]
fn an_element_without_a_registry_is_left_alone() {
    let outcome = parse_with(TODO, &["todo"], None);
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert_eq!(outcome.text, TODO);
}

#[test]
fn ordinary_markup_is_not_a_call() {
    let outcome = parse("<div><p>x</p></div>");
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert!(outcome.diagnostics.is_empty());
    assert_eq!(outcome.text, "<div><p>x</p></div>");
}

#[test]
fn prose_inside_a_known_tool_tag_is_left_alone() {
    let text = "<todo>remember to <todos>ship</todos> it</todo>";
    let outcome = parse(text);
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert!(outcome.diagnostics.is_empty());
    assert_eq!(outcome.text, text);
}

#[test]
fn a_language_fence_protects_an_element_example() {
    let outcome = parse(&format!("```xml\n{TODO}\n```"));
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
}

#[test]
fn a_child_that_is_not_a_parameter_is_malformed() {
    let outcome = parse("<todo><item>ship</item></todo>");
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert_eq!(malformed(&outcome), 1, "{:?}", outcome.diagnostics);
}

#[test]
fn undecodable_json_in_a_claimed_element_is_malformed_in_batch_and_stream() {
    let text = "<todo>\n<todos>\n[{\"status\": \"pending\", \n</todos>\n</todo>";
    let outcome = parse(text);
    assert!(outcome.calls.is_empty(), "{:?}", outcome.calls);
    assert_eq!(malformed(&outcome), 1, "{:?}", outcome.diagnostics);

    let mut scrubber = StreamScrubber::new()
        .with_known_tools(vec!["todo".into()])
        .with_registry(Arc::new(registry()));
    let (mut calls, mut diagnostics, mut shown) = (Vec::new(), Vec::new(), String::new());
    for chunk in text.as_bytes().chunks(7) {
        let step = scrubber.feed(std::str::from_utf8(chunk).unwrap_or_default());
        calls.extend(step.calls);
        diagnostics.extend(step.diagnostics);
        shown.push_str(&step.text);
    }
    let step = scrubber.flush();
    calls.extend(step.calls);
    diagnostics.extend(step.diagnostics);
    shown.push_str(&step.text);
    assert!(calls.is_empty(), "{calls:?}");
    let stream_malformed = diagnostics
        .iter()
        .filter(|d| {
            matches!(
                d,
                ParseDiagnostic::MalformedBlock {
                    source: CallSource::Element,
                    ..
                }
            )
        })
        .count();
    assert_eq!(stream_malformed, 1, "{diagnostics:?}");
    assert!(!shown.contains("<todos>"), "{shown:?}");
}

#[test]
fn a_streamed_todo_element_yields_one_call_and_no_markup() {
    let mut scrubber = StreamScrubber::new()
        .with_known_tools(vec!["todo".into()])
        .with_registry(Arc::new(registry()));
    let (mut calls, mut shown) = (Vec::new(), String::new());
    for chunk in TODO.as_bytes().chunks(5) {
        let step = scrubber.feed(std::str::from_utf8(chunk).unwrap_or_default());
        calls.extend(step.calls);
        shown.push_str(&step.text);
    }
    let step = scrubber.flush();
    calls.extend(step.calls);
    shown.push_str(&step.text);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(!shown.contains("todos"), "{shown:?}");
}
