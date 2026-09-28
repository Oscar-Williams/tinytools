//! `<tool_call>` family: spellings, garbled pipes, fences, Kimi bodies.

use super::parse;
use crate::parse::grammar::tagged::recover_sentinel_body;
use crate::types::CallSource;
use crate::{PFormatRegistry, PFormatToolParams, parse_tool_calls_with_pformat};

#[test]
fn canonical_tag_with_surrounding_prose() {
    let xml = "before\n<tool_call>\n{\"name\":\"echo\",\"arguments\":{\"value\":\"two\"}}\n</tool_call>\nafter";
    let (text, calls) = parse(xml);
    assert_eq!(text, "before\nafter");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, serde_json::json!({ "value": "two" }));
    assert_eq!(calls[0].source, CallSource::TaggedJson);
    assert!(calls[0].id.is_none(), "text calls never carry an id");
}

#[test]
fn multiple_calls_keep_prose_between_them() {
    let text = r#"a<tool_call>{"name":"one","arguments":{}}</tool_call>b<tool_call>{"name":"two","arguments":{"x":1}}</tool_call>c"#;
    let (cleaned, calls) = parse(text);
    assert_eq!(cleaned, "a\nb\nc");
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "one");
    assert_eq!(calls[1].name, "two");
}

#[test]
fn missing_arguments_default_to_empty_object() {
    let (_, calls) = parse(r#"<tool_call>{"name":"noargs"}</tool_call>"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, serde_json::json!({}));
}

#[test]
fn spelling_variants_and_bare_invoke_literal() {
    let (text, calls) =
        parse("<invoke>{\"name\":\"echo\",\"arguments\":{\"value\":\"three\"}}</invoke>");
    assert!(text.is_empty());
    assert_eq!(calls.len(), 1);

    let (_, calls) = parse("<toolcall>{\"name\":\"a\",\"arguments\":{}}</toolcall>");
    assert_eq!(calls[0].name, "a");
    let (_, calls) = parse("<tool-call>{\"name\":\"b\",\"arguments\":{}}</tool-call>");
    assert_eq!(calls[0].name, "b");
}

#[test]
fn attribute_form_and_pipe_variant_open_a_block() {
    let (cleaned, calls) =
        parse(r#"<tool_call id="call_0">{"name":"foo","arguments":{"a":1}}</tool_call>"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "foo");
    assert!(cleaned.is_empty());

    let (_, calls) = parse(r#"<tool_call|>{"name":"bar","arguments":{}}</tool_call>"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "bar");
}

#[test]
fn plural_tool_calls_tag_is_not_an_opener() {
    let (cleaned, calls) = parse("the <tool_calls> key holds them");
    assert!(calls.is_empty());
    assert_eq!(cleaned, "the <tool_calls> key holds them");
}

#[test]
fn malformed_body_is_dropped_without_dispatching() {
    let (text, calls) = parse("before <tool_call>not-json</tool_call> after");
    assert_eq!(text, "before\nafter");
    assert!(calls.is_empty());
}

#[test]
fn unclosed_tag_with_balanced_json_still_recovers() {
    let (text, calls) = parse("before <toolcall>{\"name\":\"echo\",\"arguments\":{}}");
    assert_eq!(text, "before");
    assert_eq!(calls.len(), 1);
}

#[test]
fn unclosed_tag_recovery_preserves_unrelated_markup_after_the_json() {
    // No tag-family marker exists anywhere after this opener, so a `</div>`
    // right after the recovered JSON is narrative, not a stray tool-call
    // closer — it must not be swallowed as if it were one.
    let (text, calls) = parse("<toolcall>{\"name\":\"echo\",\"arguments\":{}}</div>visible");
    assert_eq!(calls.len(), 1);
    assert_eq!(text, "</div>visible");
}

#[test]
fn unclosed_tag_without_json_is_kept_as_text() {
    let (text, calls) = parse("before <tool-call>not-json");
    assert_eq!(text, "before <tool-call>not-json");
    assert!(calls.is_empty());

    let (cleaned, calls) = parse("text <tool_call>{\"name\":\"x\"");
    assert!(calls.is_empty());
    assert_eq!(cleaned, "text <tool_call>{\"name\":\"x\"");
}

#[test]
fn prose_mention_without_closing_angle_is_not_a_tag() {
    let (cleaned, calls) = parse("wrap it in <tool_call and go");
    assert!(calls.is_empty());
    assert_eq!(cleaned, "wrap it in <tool_call and go");
}

#[test]
fn plain_text_is_returned_verbatim() {
    let (cleaned, calls) = parse("just a normal answer");
    assert!(calls.is_empty());
    assert_eq!(cleaned, "just a normal answer");
}

#[test]
fn fenced_tool_call_block_parses() {
    let markdown =
        "lead\n```tool_call\n{\"name\":\"echo\",\"arguments\":{\"value\":\"four\"}}\n```\ntrail";
    let (text, calls) = parse(markdown);
    assert_eq!(text, "lead\ntrail");
    assert_eq!(calls.len(), 1);
}

#[test]
fn fenced_tool_calls_plural_block_parses() {
    // `protected::TOOL_CALL_LANGUAGES` already classifies `tool_calls`
    // (plural) as a call language, not a protected code example; this
    // grammar must recognize it as an opener too, not just decline to
    // protect it.
    let markdown = "before\n```tool_calls\n[{\"name\":\"a\",\"arguments\":{}}]\n```\nafter";
    let (text, calls) = parse(markdown);
    assert_eq!(text, "before\nafter");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "a");
}

#[test]
fn fenced_block_closed_by_stray_tag_parses() {
    let hybrid = "```tool_call\n{\"name\":\"echo\",\"arguments\":{}}\n</tool_call>\nrest";
    let (text, calls) = parse(hybrid);
    assert_eq!(calls.len(), 1);
    assert_eq!(text, "rest");
}

#[test]
fn a_tag_body_may_carry_its_own_json_fence() {
    let text = "<tool_call>\n```json\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}\n```\n</tool_call>";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "shell");
}

#[test]
fn a_tag_body_may_hold_several_calls() {
    let response = r#"<tool_call>{"name":"get_weather","arguments":{"city":"London"}}{"name":"get_time","arguments":{"tz":"UTC"}}</tool_call>"#;
    let (_, calls) = parse(response);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "get_weather");
    assert_eq!(calls[1].name, "get_time");
}

#[test]
fn a_tagged_body_honours_argument_key_aliases() {
    for alias in ["args", "parameters", "params", "input"] {
        let text =
            format!(r#"<tool_call>{{"name":"shell","{alias}":{{"command":"ls"}}}}</tool_call>"#);
        let (_, calls) = parse(&text);
        assert_eq!(calls.len(), 1, "{alias}");
        assert_eq!(calls[0].arguments["command"], "ls", "{alias}");
    }
}

#[test]
fn a_tagged_body_with_relaxed_json_is_repaired() {
    let (_, calls) =
        parse(r#"<tool_call>{name:"get_weather",arguments:{city:"Paris"}}</tool_call>"#);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["city"], "Paris");
}

// ── garbled sentinel pipes leaked into the markers ──────────────────────────

#[test]
fn garbled_pipe_tags_with_json_body_and_call_prefix_parse() {
    let garbled = r#"<|tool_call>call:{"name": "GMAIL_LIST_THREADS", "arguments": {"query": "\"University of Colorado\"", "verbose": true}}<tool_call|>"#;
    let (_text, calls) = parse(garbled);
    assert_eq!(calls.len(), 1, "expected the garbled call to be recovered");
    assert_eq!(calls[0].name, "GMAIL_LIST_THREADS");
    assert_eq!(calls[0].arguments["query"], "\"University of Colorado\"");
    assert_eq!(calls[0].arguments["verbose"], true);
}

#[test]
fn garbled_pipe_tags_recover_multiple_parallel_calls() {
    let garbled = concat!(
        r#"<|tool_call>call:{"name": "GMAIL_LIST_THREADS", "arguments": {"query": "a"}}<tool_call|>"#,
        r#"<|tool_call>call:{"name": "GMAIL_LIST_THREADS", "arguments": {"query": "b"}}<tool_call|>"#,
    );
    let (_t, calls) = parse(garbled);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].arguments["query"], "a");
    assert_eq!(calls[1].arguments["query"], "b");
}

#[test]
fn symmetric_pipe_tags_pair_positionally() {
    let garbled = r#"<|tool_call|>{"name":"echo","arguments":{"msg":"hi"}}<|tool_call|>"#;
    let (_t, calls) = parse(garbled);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["msg"], "hi");

    let (_t, calls) = parse(r#"<tool_call>{"name":"b","arguments":{}}</tool_call|>"#);
    assert_eq!(calls.len(), 1);
}

#[test]
fn pformat_pipes_in_a_body_are_not_garbled_tags() {
    let mut registry = PFormatRegistry::new();
    registry.insert(
        "get_weather".into(),
        PFormatToolParams::from_schema(&serde_json::json!({
            "type": "object",
            "properties": { "city": { "type": "string" }, "unit": { "type": "string" } },
            "required": ["city", "unit"]
        })),
    );
    let (_, calls) = parse_tool_calls_with_pformat(
        "<tool_call>get_weather[0|London|1|metric]</tool_call>",
        &registry,
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["city"], "London");
    assert_eq!(calls[0].source, CallSource::PFormat);
}

// ── Kimi `NAME{…}` bodies (#5119) ───────────────────────────────────────────

#[test]
fn kimi_name_brace_body_with_quote_sentinels_parses() {
    let garbled = r#"<|tool_call>call:GMAIL_FETCH_EMAILS{label_ids:[<|"|>INBOX<|"|>],max_results:1,verbose:true}<tool_call|>"#;
    let (_text, calls) = parse(garbled);
    assert_eq!(calls.len(), 1, "the garbled Kimi call must be recovered");
    assert_eq!(calls[0].name, "GMAIL_FETCH_EMAILS");
    assert_eq!(
        calls[0].arguments["label_ids"],
        serde_json::json!(["INBOX"])
    );
    assert_eq!(calls[0].arguments["max_results"], 1);
    assert_eq!(calls[0].arguments["verbose"], true);
}

#[test]
fn kimi_name_brace_body_integer_only_parses() {
    let garbled = r"<|tool_call>call:GMAIL_FETCH_EMAILS{max_results:5}<tool_call|>";
    let (_text, calls) = parse(garbled);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments["max_results"], 5);
}

#[test]
fn sentinel_body_recovery_leaves_other_shapes_alone() {
    assert!(recover_sentinel_body(r#"{"name":"echo","arguments":{}}"#).is_none());
    assert!(recover_sentinel_body("get_weather[London|metric]").is_none());
    assert!(recover_sentinel_body("FOO{a:1} trailing").is_none());
}

// ── P-Format registry interplay ─────────────────────────────────────────────

fn echo_registry() -> PFormatRegistry {
    let mut reg = PFormatRegistry::new();
    reg.insert(
        "echo".to_string(),
        PFormatToolParams::from_schema(&serde_json::json!({
            "type": "object",
            "properties": { "value": { "type": "string" } }
        })),
    );
    reg
}

#[test]
fn a_pformat_tag_does_not_suppress_a_sibling_glm_tag() {
    let response =
        "<tool_call>echo[0|hello]</tool_call>\n<tool_call>shell/command>ls -la</tool_call>";
    let (_narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["echo", "shell"]);
}

#[test]
fn a_pformat_tag_does_not_suppress_a_sibling_fenced_json_tag() {
    let response = "<tool_call>echo[0|hello]</tool_call>\n<tool_call>\n```json\n{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}\n```\n</tool_call>";
    let (_narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    let names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["echo", "shell"]);
}

#[test]
fn a_json_body_is_not_double_counted_by_the_glm_fallback() {
    let response = "<tool_call>echo[0|hello]</tool_call>\n<tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"cat a/b>c\"}}</tool_call>";
    let (_narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert_eq!(calls.iter().filter(|c| c.name == "shell").count(), 1);
}

#[test]
fn a_tagged_body_with_a_registry_still_honours_argument_key_aliases() {
    let response = "<tool_call>echo[0|hello]</tool_call>\n<tool_call>{\"name\": \"shell\", \"args\": {\"command\": \"ls\"}}</tool_call>";
    let (_narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    let shell = calls
        .iter()
        .find(|c| c.name == "shell")
        .expect("aliased tagged call");
    assert_eq!(shell.arguments["command"], "ls");
}

#[test]
fn pformat_registry_with_plain_text_yields_nothing() {
    let (text, calls) = parse_tool_calls_with_pformat("plain text", &echo_registry());
    assert_eq!(text, "plain text");
    assert!(calls.is_empty());
}

#[test]
fn pformat_call_and_claude_invoke_both_survive() {
    let (_, calls) = parse_tool_calls_with_pformat(
        "<tool_call>echo[0|hi]</tool_call><invoke name=\"other\"><parameter name=\"x\">y</parameter></invoke>",
        &echo_registry(),
    );
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "echo");
    assert_eq!(calls[1].name, "other");
    assert_eq!(calls[1].arguments, serde_json::json!({"x": "y"}));
}

// ── Code-call bodies ────────────────────────────────────────────────────────

#[test]
fn a_code_call_body_parses_with_its_own_source() {
    let response = "Looking.\n<tool_call>\necho(value=\"hello\")\n</tool_call>\ndone";
    let (narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert_eq!(narrative, "Looking.\ndone");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo");
    assert_eq!(calls[0].arguments, serde_json::json!({"value": "hello"}));
    assert_eq!(calls[0].source, CallSource::Code);
    assert!(calls[0].id.is_none(), "text calls never carry an id");
}

#[test]
fn code_calls_need_a_registry() {
    // Without a registry there is no layout to bind against, so the body is
    // a recognised block with no call in it — exactly the P-Format rule.
    let (_, calls) = parse("<tool_call>echo(value=\"hello\")</tool_call>");
    assert!(calls.is_empty());
}

#[test]
fn several_code_calls_in_one_tag_keep_source_order() {
    let response = "<tool_call>\necho(\"a\")\necho(value=\"b\");\n</tool_call>";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    let values: Vec<&str> = calls
        .iter()
        .map(|c| c.arguments["value"].as_str().unwrap())
        .collect();
    assert_eq!(values, ["a", "b"]);
    assert!(calls.iter().all(|c| c.source == CallSource::Code));
}

#[test]
fn a_code_call_wrapped_in_a_python_fence_inside_the_tag_is_unwrapped() {
    let response = "<tool_call>\n```python\necho(value=\"hi\")\n```\n</tool_call>";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source, CallSource::Code);
}

#[test]
fn a_top_level_python_fence_is_an_example_not_a_call() {
    let response = "Like this:\n```python\necho(value=\"hi\")\n```\n";
    let (text, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert!(calls.is_empty());
    assert!(text.contains("echo(value=\"hi\")"));
}

#[test]
fn a_fenced_tool_call_block_holds_a_code_call() {
    let response = "```tool_call\necho(value=\"hi\")\n```";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].source, CallSource::Code);
}

#[test]
fn code_pformat_json_and_glm_siblings_all_survive() {
    let response = "<tool_call>echo(value=\"c\")</tool_call>\n\
                    <tool_call>echo[0|p]</tool_call>\n\
                    <tool_call>{\"name\": \"shell\", \"arguments\": {\"command\": \"ls\"}}</tool_call>\n\
                    <tool_call>shell/command>pwd</tool_call>";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    let sources: Vec<CallSource> = calls.iter().map(|c| c.source).collect();
    assert_eq!(
        sources,
        [
            CallSource::Code,
            CallSource::PFormat,
            CallSource::TaggedJson,
            CallSource::Glm
        ]
    );
}

#[test]
fn a_prose_body_mentioning_a_call_is_malformed_not_a_call() {
    let response = "<tool_call>I will call echo(value=\"hi\") now</tool_call>";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert!(calls.is_empty());
}

#[test]
fn a_code_call_to_an_unknown_tool_is_not_a_call() {
    let response = "<tool_call>rm_rf(path=\"/\")</tool_call>";
    let (_, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert!(calls.is_empty());
}

// ── Doubled tags ────────────────────────────────────────────────────────────
//
// `DeepSeek` V4 under a code dialect wraps the block twice:
// `<tool_call>\n<tool_call>\nNAME(...)\n</tool_call>\n</tool_call>`. Positional
// pairing used to close the first tag on the empty body and drop the call,
// and the whole thing leaked into the visible reply.

#[test]
fn a_doubled_opener_is_one_block_and_its_extra_closer_is_swallowed() {
    let response = "I'll search.\n\n<tool_call>\n<tool_call>\necho(value=\"kashmir\")\n</tool_call>\n</tool_call>";
    let (narrative, calls) = parse_tool_calls_with_pformat(response, &echo_registry());
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(calls[0].name, "echo");
    assert_eq!(calls[0].arguments, serde_json::json!({"value": "kashmir"}));
    assert_eq!(narrative, "I'll search.");
    assert!(!narrative.contains("tool_call"), "{narrative:?}");
}

#[test]
fn a_doubled_opener_around_a_json_body_parses_too() {
    let (text, calls) = parse(
        "<tool_call>\n<tool_call>\n{\"name\":\"echo\",\"arguments\":{\"value\":\"x\"}}\n</tool_call>\n</tool_call>\nafter",
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, serde_json::json!({"value": "x"}));
    assert_eq!(text, "after");
}

#[test]
fn two_adjacent_blocks_are_still_two_blocks() {
    // The doubled-opener rule only fires on a whitespace-only gap; a real
    // body between two openers is still the first block's body.
    let text = "<tool_call>{\"name\":\"one\",\"arguments\":{}}</tool_call><tool_call>{\"name\":\"two\",\"arguments\":{}}</tool_call>";
    let (_, calls) = parse(text);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].name, "one");
    assert_eq!(calls[1].name, "two");
}

#[test]
fn an_orphaned_closer_with_no_opener_anywhere_is_removed_not_shown() {
    // A model that made its actual call over a structured/native channel has
    // no `<tool_call>` opener anywhere in its text at all, but sometimes still
    // types a habitual `</tool_call>` in its narrative. `next_opener` never
    // treats a closing marker as an opener, so nothing pairs with it — it
    // must not be left in the visible text as literal markup.
    let (text, calls) = parse(
        "Let me check the details on the top contenders to find the best one for you.\n</tool_call>\nHere are the results.",
    );
    assert_eq!(
        text,
        "Let me check the details on the top contenders to find the best one for you.\nHere are the results."
    );
    assert!(calls.is_empty());
}

#[test]
fn a_doubled_opener_does_not_swallow_an_unrelated_closing_tag() {
    // Only the extra `</tool_call>` a doubled opener leaves behind is
    // protocol furniture; a real closing tag right after it (`</div>`, from
    // whatever markup the model echoed) is narrative and must survive.
    let (text, calls) = parse(
        "<tool_call>\n<tool_call>\n{\"name\":\"echo\",\"arguments\":{}}\n</tool_call>\n</div>visible",
    );
    assert_eq!(calls.len(), 1);
    assert_eq!(text, "</div>visible");
}

#[test]
fn a_doubled_opener_swallows_a_pipe_form_duplicate_closer() {
    // `TAG_RE` matches the pipe-form closer `<|/tool_call|>` too, so the
    // doubled-opener path must recognize it as a closer to swallow, not
    // leave it dangling as narrative text.
    let (text, calls) = parse(
        "<|tool_call|>\n<|tool_call|>\n{\"name\":\"echo\",\"arguments\":{}}\n<|/tool_call|>\n<|/tool_call|>\nafter",
    );
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(text, "after");
}

#[test]
fn a_doubled_opener_swallows_a_newline_leaked_duplicate_closer() {
    // `TAG_RE`'s `\s` matches any whitespace, not just space and tab, so
    // `<\n/tool_call>` is one complete closer marker; `is_closing_marker`
    // must classify it as such too, or the extra closer is left behind for
    // the narrative to leak.
    let (text, calls) = parse(
        "<tool_call>\n<tool_call>\n{\"name\":\"echo\",\"arguments\":{}}\n</tool_call>\n<\n/tool_call>\nafter",
    );
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(text, "after");
}

#[test]
fn a_bare_trailing_opener_is_dropped_not_shown() {
    // An abandoned block at the end of a reply carries no call and no
    // information; showing `<tool_call>` to the user is never right.
    let (text, calls) = parse("Let me fetch a few sites directly.\n\n<tool_call>\n");
    assert!(calls.is_empty());
    assert_eq!(text, "Let me fetch a few sites directly.");
    // A block with real (if unparseable) content is still kept as text.
    let (text, _) = parse("before <tool-call>not-json");
    assert_eq!(text, "before <tool-call>not-json");
}

// ── DeepSeek DSML on the tag family ─────────────────────────────────────────

/// `deepseek` emits its DSML marker on the `tool_call` tag, not only on
/// `<invoke>`, and with the fullwidth bar its template actually uses.
/// Observed live from `deepseek-v4-flash` driving the code dialect: the call
/// parsed as prose, so the turn ended with the model's lead-in and the tool
/// was never run.
#[test]
fn dsml_marker_on_the_tool_call_tag_parses() {
    let raw = concat!(
        "I'll look it up.\n\n",
        "<｜DSML｜tool_call>\n",
        "{\"name\": \"GMAIL_FETCH_EMAILS\", \"arguments\": {\"max_results\": 1}}\n",
        "</｜DSML｜tool_call>"
    );
    let (text, calls) = crate::parse::parse_tool_calls(raw);
    assert_eq!(calls.len(), 1, "DSML tool_call tag must parse: {calls:?}");
    assert_eq!(calls[0].name, "GMAIL_FETCH_EMAILS");
    assert_eq!(text.trim(), "I'll look it up.");
}

/// Doubled bars and the ASCII spelling are the same marker.
#[test]
fn dsml_marker_variants_on_the_tag_parse() {
    for open_close in [
        ("<｜｜DSML｜｜tool_call>", "</｜｜DSML｜｜tool_call>"),
        ("<|DSML|tool_call>", "</|DSML|tool_call>"),
        ("<｜DSML｜tool_call>", "<｜DSML｜tool_call>"),
    ] {
        let raw = format!(
            "{}\n{{\"name\": \"echo\", \"arguments\": {{}}}}\n{}",
            open_close.0, open_close.1
        );
        let (_, calls) = crate::parse::parse_tool_calls(&raw);
        assert_eq!(
            calls.len(),
            1,
            "variant {open_close:?} must parse: {calls:?}"
        );
        assert_eq!(calls[0].name, "echo");
    }
}

/// The DSML *wrapper* element is plural and is not a call marker; treating it
/// as one would open a block on the wrapper and close it on the first inner
/// tag, losing the call inside.
#[test]
fn the_plural_dsml_wrapper_is_not_a_tag_marker() {
    let raw = concat!(
        "<｜DSML｜tool_calls>\n",
        "<｜DSML｜tool_call>\n",
        "{\"name\": \"echo\", \"arguments\": {}}\n",
        "</｜DSML｜tool_call>\n",
        "</｜DSML｜tool_calls>"
    );
    let (_, calls) = crate::parse::parse_tool_calls(raw);
    assert_eq!(calls.len(), 1, "the inner call is the only call: {calls:?}");
    assert_eq!(calls[0].name, "echo");
}

/// A `DeepSeek` turn may emit the invoke form *without* a `name` attribute and
/// carry the name in the JSON body instead. The named DSML spelling
/// (`<｜DSML｜invoke name="x">`) and the bare unprefixed `<invoke>` both parsed
/// already; only their combination was missed, and the call was dropped as
/// prose.
#[test]
fn a_bare_dsml_invoke_carries_its_name_in_the_body() {
    let raw = concat!(
        "<｜DSML｜ invoke>\n",
        "{\"arguments\": {\"command\": \"ls\"}, \"name\": \"shell\"}",
        "</｜DSML｜ parameter>\n",
        "</｜DSML｜ invoke>"
    );
    let (_, calls) = crate::parse::parse_tool_calls(raw);
    assert_eq!(calls.len(), 1, "the bare DSML invoke is a call: {calls:?}");
    assert_eq!(calls[0].name, "shell");

    // The spellings that already worked must keep working: the prefix is
    // optional, and an ASCII bar, a doubled bar and a namespace are the same
    // accommodation `invoke_xml` makes on the named form.
    for open_close in [
        ("<invoke>", "</invoke>"),
        ("<|DSML| invoke>", "</|DSML| invoke>"),
        ("<｜｜DSML｜｜invoke>", "</｜｜DSML｜｜invoke>"),
        ("<atem:invoke>", "</atem:invoke>"),
    ] {
        let (open, close) = open_close;
        let raw = format!("{open}{{\"name\":\"echo\",\"arguments\":{{}}}}{close}");
        let (_, calls) = crate::parse::parse_tool_calls(&raw);
        assert_eq!(
            calls.len(),
            1,
            "variant {open_close:?} must parse: {calls:?}"
        );
        assert_eq!(calls[0].name, "echo");
    }
}

/// A block that decodes to nothing must not bury the calls after it.
///
/// Verbatim from a `deepseek` turn: an unterminated `<tool_call>` whose body is
/// `{"arguments":{…}}` with no name — genuinely unrecoverable, since the name
/// survived only in a corrupted `<｜DSML｜ parameter name="name":"file_write"}`
/// line and inventing one is never right — followed by a complete
/// `<｜DSML｜ invoke>`. The failed block used to run to end-of-text and take
/// the good call with it, so the whole response parsed as prose and both calls
/// were lost.
#[test]
fn an_undecodable_block_does_not_swallow_the_call_after_it() {
    let raw = concat!(
        "Heredocs aren't working in this shell. Writing the script to a file instead.\n\n",
        "<tool_call>\n",
        "{\"arguments\":{\"path\":\"work/extract.py\",\"content\":\"import re\"}}",
        "</｜DSML｜ parameter>\n",
        "<｜DSML｜ parameter name=\"name\":\"file_write\"}</｜DSML｜ parameter>\n",
        "</｜DSML｜ invoke>\n",
        "<｜DSML｜ invoke>\n",
        "{\"arguments\":{\"category\":\"read\",\"command\":\"ls\"},\"name\":\"shell\"}",
        "</｜DSML｜ parameter>\n",
        "</｜DSML｜ invoke>\n",
        "</｜DSML｜ calls>"
    );
    let outcome = super::parse_known(raw, &["file_write", "shell"]);
    assert_eq!(
        outcome.calls.len(),
        1,
        "the well-formed call survives its malformed neighbour: {:?}",
        outcome.calls
    );
    assert_eq!(outcome.calls[0].name, "shell");

    // Reduced to the essential shape, so a future change that reintroduces the
    // swallow fails here with less noise.
    let raw = concat!(
        "<tool_call>\n{\"arguments\":{\"path\":\"x\"}}</｜DSML｜ parameter>\n",
        "<｜DSML｜ invoke>\n{\"arguments\":{\"command\":\"ls\"},\"name\":\"shell\"}</｜DSML｜ invoke>"
    );
    let outcome = super::parse_known(raw, &["file_write", "shell"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "shell");
}

#[test]
fn a_bare_invoke_requires_its_own_closer() {
    let raw = concat!(
        "<invoke>{\"name\":\"echo\",\"arguments\":{\"text\":\"literal </atem:invoke> marker\"}}",
        "</invoke>"
    );
    let (_, calls) = parse(raw);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0].arguments,
        serde_json::json!({"text": "literal </atem:invoke> marker"})
    );
}

#[test]
fn a_bare_dsml_invoke_allows_equivalent_closer_spacing() {
    let raw = concat!(
        "<｜DSML｜ invoke>{\"name\":\"echo\",\"arguments\":{}}",
        "</｜DSML｜invoke>"
    );
    let (text, calls) = parse(raw);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(text.is_empty(), "{text:?}");
}

#[test]
fn a_fenced_invoke_block_can_close_with_an_invoke_tag() {
    let raw = "```invoke\n{\"name\":\"echo\",\"arguments\":{}}</invoke>";
    let (text, calls) = parse(raw);
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert!(text.is_empty(), "{text:?}");
}

#[test]
fn recovery_leaves_a_named_invoke_after_a_complete_malformed_body() {
    let raw = concat!(
        "<tool_call>{\"arguments\":{}}",
        "<atem:invoke name=\"shell\"><parameter name=\"command\">ls</parameter></atem:invoke>"
    );
    let outcome = super::parse_known(raw, &["shell"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "shell");
}

#[test]
fn a_named_invoke_precedes_a_later_bare_invoke_closer() {
    let raw = concat!(
        "<invoke>{\"name\":\"echo\",\"arguments\":{}}",
        "<atem:invoke name=\"shell\"><parameter name=\"command\">ls</parameter></atem:invoke>",
        "</invoke>"
    );
    let outcome = super::parse_known(raw, &["echo", "shell"]);
    assert_eq!(outcome.calls.len(), 2, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "echo");
    assert_eq!(outcome.calls[1].name, "shell");
}

#[test]
fn recovery_does_not_execute_a_named_invoke_inside_malformed_json() {
    let raw = concat!(
        "<tool_call>{\"arguments\":{\"example\":\"<invoke name=\\\"shell\\\">",
        "<parameter name=\\\"command\\\">rm -rf /</parameter></invoke>\"}}"
    );
    let (_, calls) = parse(raw);
    assert!(calls.is_empty(), "{calls:?}");
}

// ── Invoke XML inside the tag ───────────────────────────────────────────────
//
// Told to call tools "inside <tool_call> tags", `DeepSeek` V4 writes its
// native invoke XML there. The tag claimed the block and found no JSON, so
// the call was dropped as malformed even though the same invoke parses bare.

#[test]
fn a_named_invoke_wrapped_in_a_tool_call_tag_is_decoded() {
    let raw = "Searching.\n<tool_call>\n<invoke name=\"tool_search\">\n<parameter name=\"query\">repos</parameter>\n</invoke>\n</tool_call>";
    let outcome = super::parse_known(raw, &["tool_search"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "tool_search");
    assert_eq!(
        outcome.calls[0].arguments,
        serde_json::json!({"query": "repos"})
    );
    assert_eq!(outcome.calls[0].source, CallSource::InvokeXml);
    assert_eq!(outcome.text, "Searching.");
}

#[test]
fn a_wrapped_invoke_with_string_attributes_is_decoded() {
    let raw = concat!(
        "<tool_call>\n<invoke name=\"tool_search\">\n",
        "<parameter name=\"query\" string=\"true\">repos</parameter>\n",
        "<parameter name=\"limit\" string=\"false\">5</parameter>\n",
        "</invoke>\n</tool_call>"
    );
    let outcome = super::parse_known(raw, &["tool_search"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(
        outcome.calls[0].arguments,
        serde_json::json!({"query": "repos", "limit": 5})
    );
}

/// A `<todo>` block, a line of narration, then the wrapped invoke inside a
/// closed bare fence (no info string, so not protected).
#[test]
fn a_todo_block_then_a_closed_bare_fenced_wrapped_invoke_is_decoded() {
    let raw = concat!(
        "<todo>\n- [x] read the request\n- [ ] find the tool\n</todo>\n\n",
        "Let me find the right tool.\n\n",
        "```\n<tool_call>\n<invoke name=\"tool_search\">\n",
        "<parameter name=\"query\" string=\"true\">list repositories</parameter>\n",
        "</invoke>\n</tool_call>\n```"
    );
    let outcome = super::parse_known(raw, &["tool_search"]);
    assert_eq!(outcome.calls.len(), 1, "{:?}", outcome.calls);
    assert_eq!(outcome.calls[0].name, "tool_search");
    assert_eq!(
        outcome.calls[0].arguments,
        serde_json::json!({"query": "list repositories"})
    );
}

#[test]
fn an_invoke_after_other_body_text_in_the_tag_is_not_decoded() {
    let raw = "<tool_call>see <invoke name=\"shell\"><parameter name=\"command\">ls</parameter></invoke></tool_call>";
    let (_, calls) = parse(raw);
    assert!(calls.is_empty(), "{calls:?}");
}
