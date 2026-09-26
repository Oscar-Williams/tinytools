//! Regression tests for the explicit `function_call:` JSON envelope.

use super::parse;
use crate::types::CallSource;

#[test]
fn function_call_prefix_recovers_a_narrated_json_call() {
    let (text, calls) = parse(
        r#"Let me proceed with querying the tasks ledger. function_call:{"id":"call_3rY","call":"read_ledger","arguments":{"ledger":"tasks","query":"2026-09-01"}}"#,
    );

    assert_eq!(text, "Let me proceed with querying the tasks ledger.");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "read_ledger");
    assert_eq!(calls[0].source, CallSource::TaggedJson);
    assert_eq!(
        calls[0].arguments,
        serde_json::json!({"ledger": "tasks", "query": "2026-09-01"})
    );
}

#[test]
fn function_call_prefix_accepts_the_standard_name_field() {
    let (text, calls) =
        parse(r#"function_call: {"name":"echo","arguments":{"value":"ok"}} trailing"#);

    assert_eq!(text, "trailing");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo");
    assert_eq!(calls[0].arguments, serde_json::json!({"value": "ok"}));
}

#[test]
fn an_unrelated_function_call_object_stays_visible() {
    let input = r#"function_call:{"message":"this is not a tool call"}"#;

    let (text, calls) = parse(input);

    assert_eq!(text, input);
    assert!(calls.is_empty());
}

#[test]
fn a_fenced_function_call_example_is_not_executed() {
    let input = "```text\nfunction_call:{\"name\":\"echo\",\"arguments\":{}}\n```";

    let (text, calls) = parse(input);

    assert_eq!(text, input);
    assert!(calls.is_empty());
}

#[test]
fn an_invalid_marked_object_does_not_hide_a_later_call() {
    let (text, calls) = parse(
        r#"function_call:{"message":"not a call"} then function_call:{"call":"echo","arguments":{}}"#,
    );

    assert_eq!(text, r#"function_call:{"message":"not a call"} then"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo");
}

#[test]
fn an_unterminated_marked_object_stays_visible_in_batch_mode() {
    let input = r#"function_call:{"call":"echo""#;

    let (text, calls) = parse(input);

    assert_eq!(text, input);
    assert!(calls.is_empty());
}

#[test]
fn an_invalid_marked_json_object_stays_visible() {
    let input = r#"function_call:{call:"echo"}"#;

    let (text, calls) = parse(input);

    assert_eq!(text, input);
    assert!(calls.is_empty());
}
