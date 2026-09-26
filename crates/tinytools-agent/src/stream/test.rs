//! Unit tests for the stream scrubber.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use super::StreamScrubber;
use crate::parse::parse_tool_calls;

/// Feeds every fragment and appends the flush, returning the visible text and
/// every call released along the way.
fn scrub_all(fragments: &[&str]) -> (String, usize) {
    let mut s = StreamScrubber::new();
    let mut out = String::new();
    let mut calls = 0;
    for f in fragments {
        let step = s.feed(f);
        out.push_str(&step.text);
        calls += step.calls.len();
    }
    let step = s.flush();
    out.push_str(&step.text);
    calls += step.calls.len();
    (out, calls)
}

#[test]
fn plain_text_passes_through_unchanged() {
    assert_eq!(
        scrub_all(&["hello ", "world", " done"]).0,
        "hello world done"
    );
}

#[test]
fn a_complete_block_in_one_fragment_is_dropped_and_its_call_released() {
    let (out, calls) =
        scrub_all(&[r#"before <tool_call>{"name":"x","arguments":{}}</tool_call> after"#]);
    assert_eq!(out, "before  after");
    assert_eq!(calls, 1);
}

#[test]
fn markup_split_across_fragments_never_leaks() {
    let (out, calls) = scrub_all(&[
        "answer: ",
        "<tool_",
        "call>{\"name\":",
        "\"x\",\"arguments\":{\"a\":1}}",
        "</tool_",
        "call> end",
    ]);
    assert_eq!(out, "answer:  end");
    assert_eq!(calls, 1);
}

#[test]
fn function_call_prefix_split_across_fragments_is_scrubbed() {
    let (out, calls) = scrub_all(&[
        "answer: ",
        "function_",
        "call:{\"id\":\"c1\",\"call\":\"read_ledger\",\"arguments\":",
        "{\"ledger\":\"tasks\"}} done",
    ]);
    assert_eq!(out, "answer:  done");
    assert_eq!(calls, 1);
}

#[test]
fn a_function_call_marker_with_no_body_is_held_until_the_next_fragment() {
    let mut s = StreamScrubber::new();
    let first = s.feed("answer: function_call: ");
    assert_eq!(first.text, "answer: ");
    assert!(first.calls.is_empty());

    let second = s.feed(r#"{"call":"echo","arguments":{}} done"#);
    assert_eq!(second.text, " done");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].name, "echo");
}

#[test]
fn a_partial_open_marker_is_held_not_emitted() {
    let mut s = StreamScrubber::new();
    let first = s.feed("value <tool");
    assert_eq!(first.text, "value ", "partial `<tool` must be held");
    let second = s.feed("_call>{\"name\":\"x\",\"arguments\":{}}</tool_call>!");
    assert_eq!(second.text, "!");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(s.flush().text, "");
}

#[test]
fn an_attribute_open_form_split_mid_tag_is_held() {
    let mut s = StreamScrubber::new();
    let mut out = String::new();
    out.push_str(&s.feed("ok <tool_call id=\"c").text);
    out.push_str(
        &s.feed("all_0\">{\"name\":\"x\",\"arguments\":{}}</tool_call>")
            .text,
    );
    out.push_str(&s.flush().text);
    assert_eq!(out, "ok ");
}

#[test]
fn deepseek_delimiters_split_across_fragments_are_scrubbed() {
    let (out, calls) = scrub_all(&[
        "r <｜tool▁ca",
        "ll▁begin｜>{\"name\":\"a\",\"arguments\":{}}<｜tool▁call",
        "▁end｜> s",
    ]);
    assert_eq!(out, "r  s");
    assert!(!out.contains("tool▁call"));
    assert_eq!(calls, 1);
}

#[test]
fn dsml_split_across_fragments_is_scrubbed() {
    let (out, calls) = scrub_all(&[
        "Sure. <｜DS",
        "ML｜tool_calls><｜DSML｜invoke name=\"read\">{\"path\":\"a\"}",
        "</｜DSML｜invoke></｜DSML｜tool_calls> Done.",
    ]);
    assert_eq!(out.trim(), "Sure.  Done.".trim());
    assert!(!out.contains("DSML"), "{out}");
    assert_eq!(calls, 1);
}

#[test]
fn a_whitespace_prefixed_dsml_opener_split_before_the_marker_is_held() {
    // The complete-tag regex tolerates a space before the marker
    // (`< | DSML | invoke`), but that must not regress streaming: a
    // fragment boundary landing right after `<` and before the marker has
    // arrived must still be held, or the opener is lost the moment the
    // held-back fragment is flushed as narrative.
    let mut s = StreamScrubber::new();
    let first = s.feed("< ");
    assert_eq!(
        first.text, "",
        "a possible whitespace-prefixed DSML opener must be held"
    );
    assert!(first.calls.is_empty());

    let second = s.feed("| DSML | invoke name=\"read\">{\"path\":\"a\"}</| DSML | invoke>");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].name, "read");
}

#[test]
fn plural_tool_calls_prose_is_not_held() {
    assert_eq!(
        scrub_all(&["the <tool_calls> ", "key"]).0,
        "the <tool_calls> key"
    );
}

#[test]
fn flush_surfaces_a_dangling_open_verbatim_untrimmed() {
    let mut s = StreamScrubber::new();
    let mid = s.feed("  a <tool_call ");
    assert_eq!(mid.text, "  a ", "the in-progress open tag is held");
    assert_eq!(s.flush().text, "<tool_call ");
}

#[test]
fn a_harmony_call_is_held_until_its_terminator() {
    let mut s = StreamScrubber::new();
    let first = s.feed("<|channel|>commentary to=functions.read<|message|>{\"path\":");
    assert_eq!(first.text, "");
    let second = s.feed("\"a\"}<|call|>tail");
    assert_eq!(second.text, "tail");
    assert_eq!(second.calls[0].name, "read");
}

#[test]
fn a_fenced_example_split_across_fragments_never_leaks_a_call() {
    // A language-tagged fence opener released before its closing fence
    // arrives would erase the only record that the buffer is still inside
    // protected content; the next fragment's `<tool_call>` would then be
    // read as a real call instead of the documentation example it is.
    let mut s = StreamScrubber::new();
    let first = s.feed("example:\n```bash\n");
    assert_eq!(
        first.text, "example:\n",
        "the open fence must be held, not drained"
    );
    assert!(first.calls.is_empty());

    let second =
        s.feed("echo <tool_call>{\"name\":\"x\",\"arguments\":{}}</tool_call>\n```\nafter");
    assert!(
        second.calls.is_empty(),
        "the fenced example must not dispatch a call: {second:?}"
    );
    assert_eq!(
        second.text,
        "```bash\necho <tool_call>{\"name\":\"x\",\"arguments\":{}}</tool_call>\n```\nafter"
    );
}

#[test]
fn a_namespaced_invoke_opener_split_before_its_closing_bracket_is_held() {
    // The namespace prefix (`atem:`) is open-ended and not in any fixed
    // opener list, so this can only be caught by recognizing the tag
    // structurally rather than by literal prefix matching.
    let mut s = StreamScrubber::new();
    let first = s.feed("<atem:invoke name=\"read\"");
    assert_eq!(
        first.text, "",
        "an unterminated namespaced opener must be held"
    );
    assert!(first.calls.is_empty());

    let second = s.feed("><parameter name=\"path\">a</parameter></atem:invoke>");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].name, "read");
}

#[test]
fn a_sentinel_split_on_an_unlisted_bar_underscore_combination_is_held() {
    // The bar style and word separator vary independently across the four
    // sentinel spellings; a split right after the fullwidth-bar/ASCII-
    // underscore combination must still be held even though it was not one
    // of the two combinations the fixed opener list used to carry.
    let mut s = StreamScrubber::new();
    let first = s.feed("<｜tool_");
    assert_eq!(
        first.text, "",
        "an unlisted bar/separator split must be held"
    );
    assert!(first.calls.is_empty());

    let second =
        s.feed("call_begin｜>get_weather<｜tool_sep｜>{\"city\":\"Paris\"}<｜tool_call_end｜>");
    assert_eq!(second.calls.len(), 1);
    assert_eq!(second.calls[0].name, "get_weather");
}

#[test]
fn a_harmony_channel_header_without_message_yet_is_held() {
    // The header names a target but `<|message|>` has not streamed in yet,
    // so the scrubber must hold the fragment rather than guess.
    let mut s = StreamScrubber::new();
    let first = s.feed("<|channel|>commentary to=functions.read");
    assert_eq!(first.text, "", "a pending channel header must be held");
    let second = s.feed("<|message|>{\"path\":\"a\"}<|call|>tail");
    assert_eq!(second.text, "tail");
    assert_eq!(second.calls[0].name, "read");
}

#[test]
fn stream_matches_batch_parser_on_the_visible_text() {
    let full = r#"lead <tool_call>{"name":"a","arguments":{}}</tool_call> mid <tool_call>{"name":"b","arguments":{"k":1}}</tool_call> tail"#;
    let (batch, calls) = parse_tool_calls(full);
    assert_eq!(calls.len(), 2);
    let frags: Vec<String> = full.chars().map(|c| c.to_string()).collect();
    let refs: Vec<&str> = frags.iter().map(String::as_str).collect();
    let (streamed, stream_calls) = scrub_all(&refs);
    assert_eq!(
        streamed.split_whitespace().collect::<Vec<_>>(),
        batch.split_whitespace().collect::<Vec<_>>()
    );
    assert_eq!(stream_calls, 2);
}

#[test]
fn known_tools_repair_streamed_names() {
    let mut s = StreamScrubber::new().with_known_tools(vec!["read_file".into()]);
    let step = s.feed("<tool_call>{\"name\":\"functions.read_file\",\"arguments\":{}}</tool_call>");
    assert_eq!(step.calls[0].name, "read_file");
}

#[test]
fn a_code_call_split_mid_string_is_released_once_and_never_shown() {
    let registry = crate::build_registry([(
        "echo",
        serde_json::json!({"type": "object", "properties": {"value": {"type": "string"}}}),
    )]);
    let mut s = StreamScrubber::new().with_registry(std::sync::Arc::new(registry));
    let mut out = String::new();
    let mut calls = Vec::new();
    for f in [
        "Sure. <tool_call>\necho(value=\"he",
        "llo)\")\n</tool_",
        "call> done",
    ] {
        let step = s.feed(f);
        out.push_str(&step.text);
        calls.extend(step.calls);
    }
    let step = s.flush();
    out.push_str(&step.text);
    calls.extend(step.calls);

    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo");
    assert_eq!(calls[0].arguments["value"], "hello)");
    assert_eq!(calls[0].source, crate::types::CallSource::Code);
    assert!(!out.contains("echo("), "markup leaked: {out:?}");
    assert!(out.contains("Sure.") && out.contains("done"));
}

#[test]
fn a_doubled_blocks_extra_closer_split_across_fragments_never_leaks() {
    // The doubled opener's *inner* closer can arrive in one fragment and the
    // matching extra closer in the next. The block must stay pending across
    // that boundary rather than finalize on the inner closer alone and let
    // the later `</tool_call>` fall through as visible text.
    let (out, calls) = scrub_all(&[
        "before <tool_call>\n<tool_call>\n{\"name\":\"x\",\"arguments\":{}}\n</tool_call>\n",
        "</tool_call> after",
    ]);
    assert_eq!(out, "before  after");
    assert_eq!(calls, 1);
    assert!(!out.contains("tool_call"), "{out:?}");
}

#[test]
fn a_doubled_blocks_extra_closer_split_mid_marker_never_leaks() {
    // The fragment boundary can land *inside* the extra closer itself, not
    // just before it: `</tool_` in one fragment, `call>` in the next. That
    // partial marker must be held rather than released as text once its
    // first fragment is scanned.
    let (out, calls) = scrub_all(&[
        "before <tool_call>\n<tool_call>\n{\"name\":\"x\",\"arguments\":{}}\n</tool_call>\n</tool_",
        "call> after",
    ]);
    assert_eq!(out, "before  after");
    assert_eq!(calls, 1);
    assert!(!out.contains("tool_"), "{out:?}");
}
