//! The scan engine: protected fences, name resolution, helpers, diagnostics.

use super::{parse, parse_known};
use crate::parse::json_values::{extract_first_json_value_with_end, find_json_end};
use crate::parse::protected::fence_ranges;
use crate::parse::{
    extract_json_values, parse_arguments_value, parse_tool_call_value,
    parse_tool_calls_from_json_value,
};
use crate::types::ParseDiagnostic;

#[test]
fn a_call_inside_a_language_fence_is_an_example_not_a_call() {
    let text = "Here is the format:\n```bash\n<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"rm -rf /\"}}</tool_call>\n```\nDo not run it.";
    let (cleaned, calls) = parse(text);
    assert!(calls.is_empty(), "fenced example must not dispatch");
    assert_eq!(cleaned, text);
}

#[test]
fn a_call_inside_a_bare_fence_still_parses() {
    let text =
        "```\n<tool_call>{\"name\":\"shell\",\"arguments\":{\"command\":\"ls\"}}</tool_call>\n```";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 1);
}

#[test]
fn a_call_after_a_closed_fence_parses() {
    let text = "```python\nprint('<tool_call>')\n```\n<tool_call>{\"name\":\"echo\",\"arguments\":{}}</tool_call>";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 1);
}

#[test]
fn fence_ranges_cover_languages_and_unclosed_fences() {
    let text = "a\n```rust\nx\n```\nb\n~~~js\ny\n";
    let ranges = fence_ranges(text);
    assert_eq!(ranges.len(), 2);
    assert_eq!(&text[ranges[0].clone()], "```rust\nx\n```");
    assert_eq!(ranges[1].end, text.len());
    assert!(fence_ranges("```\nplain\n```").is_empty());
    assert!(fence_ranges("```tool_call\n{}\n```").is_empty());
}

// ── A call tag on the fence line itself ────────────────────────────────────
//
// `DeepSeek` V4 Flash wrote ```` ```<tool_call> ```` — the opener as the info
// string — and never closed the fence. The info string read as a language,
// so the unclosed fence protected the call to end of text as an example.

#[test]
fn an_unclosed_fence_whose_info_string_is_a_call_tag_is_a_call() {
    let text = concat!(
        "<todos>\n- [ ] find the tool\n</todos>\n</tool_call>\n",
        "```<tool_call>\n<invoke name=\"tool_search\">\n",
        "<parameter name=\"query\" string=\"true\">list repositories</parameter>\n",
        "</invoke>\n</tool_call>"
    );
    let outcome = parse_known(text, &["tool_search"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "tool_search");
    assert_eq!(
        outcome.calls[0].arguments,
        serde_json::json!({"query": "list repositories"})
    );
}

#[test]
fn a_fence_whose_info_string_is_a_named_invoke_is_a_call() {
    let text =
        "```<invoke name=\"tool_search\">\n<parameter name=\"query\">repos</parameter>\n</invoke>";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].name, "tool_search");
}

#[test]
fn a_closed_fence_whose_info_string_is_a_call_tag_is_a_call() {
    let text = "```<tool_call>\n<invoke name=\"tool_search\">\n<parameter name=\"query\">repos</parameter>\n</invoke>\n</tool_call>\n```";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 1, "{calls:?}");
}

#[test]
fn a_language_fence_still_protects_a_call_tag_example() {
    let example = "<tool_call>\n<invoke name=\"shell\"><parameter name=\"command\">rm -rf /</parameter></invoke>\n</tool_call>";
    for text in [
        format!("```xml\n{example}"),
        format!("```xml<tool_call>\n{example}"),
        format!("```text <tool_call>\n{example}"),
    ] {
        let (_, calls) = parse(&text);
        assert!(calls.is_empty(), "{text:?} dispatched {calls:?}");
    }
}

#[test]
fn names_are_repaired_against_known_tools() {
    let outcome = parse_known(
        "<tool_call>{\"name\":\"terminal\\\" parameter=\\\"command\",\"arguments\":{\"command\":\"ls\"}}</tool_call>",
        &["terminal", "read_file"],
    );
    assert_eq!(outcome.calls[0].name, "terminal");
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| matches!(d, ParseDiagnostic::NameRepaired { to, .. } if to == "terminal"))
    );

    let outcome = parse_known(
        "<tool_call>{\"name\":\"functions.read_file\",\"arguments\":{}}</tool_call>",
        &["read_file"],
    );
    assert_eq!(outcome.calls[0].name, "read_file");
    let outcome = parse_known(
        "<tool_call>{\"name\":\"Read File\",\"arguments\":{}}</tool_call>",
        &["read_file"],
    );
    assert_eq!(outcome.calls[0].name, "read_file");
    let outcome = parse_known(
        "<tool_call>{\"name\":\"raed_file\",\"arguments\":{}}</tool_call>",
        &["read_file", "write_file"],
    );
    assert_eq!(outcome.calls[0].name, "read_file");
}

#[test]
fn an_unknown_name_is_returned_and_flagged() {
    let outcome = parse_known(
        "<tool_call>{\"name\":\"launch_missiles\",\"arguments\":{}}</tool_call>",
        &["read_file"],
    );
    assert_eq!(outcome.calls.len(), 1);
    assert_eq!(outcome.calls[0].name, "launch_missiles");
    assert!(matches!(
        outcome.diagnostics[0],
        ParseDiagnostic::UnknownTool { .. }
    ));
}

#[test]
fn malformed_and_unterminated_blocks_are_reported() {
    let outcome = parse_known(
        "<tool_call>nope</tool_call> and <tool_call>{\"name\":\"x\"",
        &[],
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| matches!(d, ParseDiagnostic::MalformedBlock { .. }))
    );
    assert!(
        outcome
            .diagnostics
            .iter()
            .any(|d| matches!(d, ParseDiagnostic::UnterminatedBlock { .. }))
    );
}

#[test]
fn mixed_grammars_in_one_response_parse_in_source_order() {
    let text = concat!(
        "<tool_call>{\"name\":\"a\",\"arguments\":{}}</tool_call>",
        "<invoke name=\"b\"><parameter name=\"k\">v</parameter></invoke>",
        "<｜tool▁call▁begin｜>c<｜tool▁sep｜>{}<｜tool▁call▁end｜>",
        "[TOOL_CALLS][{\"name\":\"d\",\"arguments\":{}}]"
    );
    let (_, calls) = parse(text);
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c", "d"]);
}

// ── helpers kept public for hosts ───────────────────────────────────────────

#[test]
fn parse_argument_helpers_cover_string_non_string_and_missing_values() {
    assert_eq!(
        parse_arguments_value(Some(&serde_json::json!("{\"value\":1}"))),
        serde_json::json!({ "value": 1 })
    );
    assert_eq!(
        parse_arguments_value(Some(&serde_json::json!("not-json"))),
        serde_json::json!({})
    );
    assert_eq!(
        parse_arguments_value(Some(&serde_json::json!({ "value": 2 }))),
        serde_json::json!({ "value": 2 })
    );
    assert_eq!(parse_arguments_value(None), serde_json::json!({}));
}

#[test]
fn parse_tool_call_value_supports_function_shape_flat_shape_and_invalid_names() {
    let function_shape = serde_json::json!({
        "function": { "name": "shell", "arguments": "{\"command\":\"ls\"}" }
    });
    let parsed = parse_tool_call_value(&function_shape).expect("function call should parse");
    assert_eq!(parsed.name, "shell");
    assert_eq!(parsed.arguments, serde_json::json!({ "command": "ls" }));

    let flat_shape = serde_json::json!({ "name": "echo", "arguments": { "value": "hi" } });
    let parsed = parse_tool_call_value(&flat_shape).expect("flat call should parse");
    assert_eq!(parsed.name, "echo");

    assert!(parse_tool_call_value(&serde_json::json!({ "name": "   " })).is_none());
    assert!(parse_tool_call_value(&serde_json::json!({ "function": {} })).is_none());
    assert!(parse_tool_call_value(&serde_json::json!({ "tool": "echo", "args": {} })).is_none());
}

#[test]
fn parse_tool_calls_from_json_value_handles_envelopes_arrays_and_singletons() {
    let wrapped = serde_json::json!({
        "tool_calls": [
            { "name": "echo", "arguments": { "value": "one" } },
            { "function": { "name": "shell", "arguments": "{\"command\":\"pwd\"}" } }
        ],
        "content": "assistant text"
    });
    let calls = parse_tool_calls_from_json_value(&wrapped);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1].name, "shell");

    let single = serde_json::json!({ "name": "echo", "arguments": { "value": "three" } });
    assert_eq!(parse_tool_calls_from_json_value(&single).len(), 1);

    // Tagged contexts widen `input` into arguments.
    let answer = serde_json::json!({ "name": "Alice", "input": { "value": "hi" } });
    assert_eq!(
        parse_tool_calls_from_json_value(&answer)[0].arguments,
        serde_json::json!({ "value": "hi" })
    );
}

#[test]
fn json_scanners_cover_common_edge_cases() {
    let extracted =
        extract_first_json_value_with_end(" text {\"ok\":true} trailing ").expect("json");
    assert_eq!(extracted.0, serde_json::json!({ "ok": true }));
    assert!(extracted.1 > 0);
    assert!(extract_first_json_value_with_end("no json here").is_none());

    let values = extract_json_values("before {\"a\":1} [1,2] after");
    assert_eq!(
        values,
        vec![serde_json::json!({ "a": 1 }), serde_json::json!([1, 2])]
    );
    assert!(extract_json_values("").is_empty());
    assert!(extract_json_values("{not json} [still bad]").is_empty());

    assert_eq!(
        find_json_end("  {\"a\":\"}\"}tail"),
        Some("  {\"a\":\"}\"}".len())
    );
    assert_eq!(find_json_end("[1,2,3]"), None);
    assert!(find_json_end("{\"escaped\":\"\\\\\"}\"}tail").is_some());
    assert!(find_json_end("{\"unfinished\": true").is_none());
}
