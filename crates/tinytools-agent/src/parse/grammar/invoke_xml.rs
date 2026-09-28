//! `<invoke name="…"><parameter name="…">…</parameter></invoke>` and its
//! relatives.
//!
//! One parser covers every XML-shaped call because they differ only in the
//! tag prefix and the attribute spelling:
//!
//! * Claude's native form, `<invoke name="read"><parameter name="path">…`;
//! * `DeepSeek` DSML, `<｜DSML｜invoke name="read">…</｜DSML｜invoke>` inside a
//!   `<｜DSML｜tool_calls>` wrapper — with single or doubled bars, fullwidth
//!   or ASCII, an optional space after the marker (`<｜｜DSML｜｜ invoke`), and
//!   an optional space before the marker on either the opening or closing
//!   tag (`< |DSML|invoke`, `</ |DSML|invoke>`) — some models add readable
//!   spacing around the whole delimiter, not just after it;
//! * namespaced variants such as `<atem:invoke name="default.terminal">`;
//! * `<function name="…">` (Gemma) and `<function=NAME>` (Llama / Qwen), with
//!   `<parameter=k>v</parameter>` children.
//!
//! Arguments come from the parameter children when there are any, with the
//! DSML `<parameter name="arguments">{json}</parameter>` envelope unwrapped;
//! otherwise from a JSON body, tolerating an orphan `</parameter>` the model
//! left behind. Wrapper tags (`<tool_calls>`, `<function_calls>`, `<calls>`)
//! are protocol furniture and are removed without producing a call.

use std::sync::LazyLock;

use regex::Regex;

use super::{Block, Decoded, Grammar, Probe, ScanMode, pending_opener, prefer_pending};
use crate::repair::json::recover_object;
use crate::types::{CallSource, ParseOptions, ParsedToolCall};

/// The invoke-XML grammar.
#[derive(Debug)]
pub(crate) struct InvokeXml;

/// Optional tag prefix: a DSML marker or an XML namespace.
const PREFIX: &str = r"(?:[|｜]{1,2}\s*DSML\s*[|｜]{1,2}\s*|[A-Za-z_][\w.-]*:)?";

/// `<invoke name="…">`, `<function name="…">`, `<function=NAME>`.
static OPEN_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?is)<\s*{PREFIX}(?:invoke|function)(?:\s+[^>]*?\bname\s*=\s*"([^"]*)"[^>]*|\s*=\s*([^\s>,]+)[^>]*)>"#
    ))
    .ok()
});

/// Wrapper tags around a group of invokes, open or close.
static WRAPPER_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?is)</?\s*{PREFIX}(?:tool_calls|function_calls|calls)\s*>"
    ))
    .ok()
});

/// The start of an `<invoke`/`<function` opener, with or without a namespace
/// or DSML prefix, that does not itself require the tag's `>` to match.
///
/// [`OPEN_RE`] only matches a *complete* opening tag, so a fragment boundary
/// that falls before the `>` — `<atem:invoke name="read"` with nothing
/// after it yet — makes it match nothing at all. The unprefixed and DSML
/// spellings are covered by the fixed literal list `probe` also holds on
/// (`"<invoke "`, `"<function"`, `"<|DSML"`, `"<｜DSML"`), but an XML
/// namespace prefix is open-ended and cannot be enumerated the same way;
/// this regex recognizes the tag structurally instead, so a stream split
/// mid-namespace still holds the fragment back.
static OPEN_START_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(&format!(r"(?is)<\s*{PREFIX}(?:invoke|function)\b")).ok());

/// A closing tag ending an invoke: its own, `</function>`, or a stray
/// `</tool_call>` some templates substitute.
static CLOSE_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?is)</\s*{PREFIX}(?:invoke|function|tool_call)\s*>"
    ))
    .ok()
});

/// `<parameter name="k" …>v</parameter>` and `<parameter=k>v</parameter>`.
static PARAMETER_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"(?is)<\s*{PREFIX}parameter(?:\s+[^>]*?\bname\s*=\s*"([^"]*)"[^>]*|\s*=\s*([^\s>]+)[^>]*)>(.*?)</\s*{PREFIX}parameter\s*>"#
    ))
    .ok()
});

/// An orphan closing parameter tag left in a JSON body.
static ORPHAN_PARAMETER_CLOSE_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(&format!(r"(?is)</\s*{PREFIX}parameter\s*>")).ok());

impl Grammar for InvokeXml {
    fn source(&self) -> CallSource {
        CallSource::InvokeXml
    }

    fn probe(&self, text: &str, from: usize, _options: &ParseOptions<'_>, mode: ScanMode) -> Probe {
        let literal_pending = pending_opener(
            text,
            from,
            &["<invoke ", "<function", "<|DSML", "<｜DSML"],
            ">",
            mode,
        );
        let namespaced_pending =
            (mode == ScanMode::Stream).then(|| Self::pending_namespaced_open(text, from));
        let pending = [literal_pending, namespaced_pending.flatten()]
            .into_iter()
            .flatten()
            .min();
        prefer_pending(Self::probe_decided(text, from, mode), pending)
    }

    fn openers(&self) -> &'static [&'static str] {
        &[
            "<invoke ",
            "<function",
            "<|DSML",
            "<｜DSML",
            "</|DSML",
            "</｜DSML",
            "<tool_calls>",
            "<function_calls>",
            "</tool_calls>",
            "</function_calls>",
        ]
    }
}

impl InvokeXml {
    /// The start of a namespaced `<ns:invoke …` (or `<ns:function …`) opener
    /// whose `>` has not arrived yet, if any.
    fn pending_namespaced_open(text: &str, from: usize) -> Option<usize> {
        let re = OPEN_START_RE.as_ref()?;
        let hay = &text[from..];
        let m = re.find(hay)?;
        if hay[m.end()..].contains('>') {
            return None;
        }
        Some(from + m.start())
    }

    /// The next block whose opener is fully present.
    fn probe_decided(text: &str, from: usize, mode: ScanMode) -> Probe {
        let (Some(open_re), Some(wrapper_re), Some(close_re)) =
            (OPEN_RE.as_ref(), WRAPPER_RE.as_ref(), CLOSE_RE.as_ref())
        else {
            return Probe::None;
        };
        let hay = &text[from..];

        let wrapper = wrapper_re.find(hay);
        let open = open_re.captures(hay);

        // A wrapper tag before the next invoke is furniture: remove it alone.
        // A bare `<tool_calls>` opener with no invoke anywhere after it is a
        // prose mention (the JSON key, say) and stays.
        if let Some(w) = wrapper
            && open
                .as_ref()
                .is_none_or(|o| w.start() < o.get(0).map_or(usize::MAX, |m| m.start()))
            && (open.is_some() || is_closer_or_prefixed(w.as_str()))
        {
            return Probe::Found(Block {
                start: from + w.start(),
                end: from + w.end(),
                decoded: Decoded::Noise,
            });
        }

        let Some(open) = open else {
            return Probe::None;
        };
        let Some(open_match) = open.get(0) else {
            return Probe::None;
        };
        let name = open
            .get(1)
            .or_else(|| open.get(2))
            .map_or("", |m| m.as_str().trim());
        let start = from + open_match.start();
        let body_start = from + open_match.end();
        let after = &text[body_start..];

        let close = close_re.find(after).map(|m| (m.start(), m.end()));
        let Some((body_end, close_end)) = close else {
            if mode == ScanMode::Stream {
                return Probe::Pending { start };
            }
            return Probe::Found(Block {
                start,
                end: text.len(),
                decoded: Decoded::Verbatim,
            });
        };

        let body = &after[..body_end];
        let end = body_start + close_end;
        if name.is_empty() {
            return Probe::Found(Block {
                start,
                end,
                decoded: Decoded::Malformed {
                    body_chars: body.chars().count(),
                },
            });
        }

        let arguments = decode_arguments(body);
        Probe::Found(Block {
            start,
            end,
            decoded: Decoded::Calls(vec![ParsedToolCall::new(
                name,
                arguments,
                CallSource::InvokeXml,
            )]),
        })
    }
}

/// Every call in a tag body that *is* invoke XML — `<tool_call><invoke
/// name="x">…</invoke></tool_call>`, what `DeepSeek` V4 writes when told to
/// call tools inside `<tool_call>` tags. Empty unless the body opens with a
/// named invoke, so an invoke quoted inside some other body (a JSON string,
/// say) is not executed. Once the body does open with one, every later
/// invoke in it is decoded too, exactly as the same text outside a tag is.
pub(crate) fn decode_body(body: &str) -> Vec<ParsedToolCall> {
    let body = body.trim_start();
    if OPEN_RE
        .as_ref()
        .and_then(|re| re.find(body))
        .is_none_or(|m| m.start() != 0)
    {
        return Vec::new();
    }
    let mut calls = Vec::new();
    let mut from = 0;
    while let Probe::Found(block) = InvokeXml::probe_decided(body, from, ScanMode::Batch) {
        if let Decoded::Calls(found) = block.decoded {
            calls.extend(found);
        }
        from = block.end;
    }
    calls
}

/// Whether a wrapper tag is a closer or carries a DSML / namespace prefix —
/// either is unambiguous protocol furniture even with no invoke in sight.
fn is_closer_or_prefixed(tag: &str) -> bool {
    let inner = &tag[1..];
    inner.starts_with('/')
        || !inner.starts_with(|c: char| c.is_ascii_alphabetic())
        || inner.contains(':')
}

/// Arguments from parameter children or a JSON body.
fn decode_arguments(body: &str) -> serde_json::Value {
    let Some(parameter_re) = PARAMETER_RE.as_ref() else {
        return serde_json::json!({});
    };

    let mut parameters = serde_json::Map::new();
    for cap in parameter_re.captures_iter(body) {
        let key = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map_or("", |m| m.as_str().trim());
        if key.is_empty() {
            continue;
        }
        let raw = cap.get(3).map_or("", |m| m.as_str());
        parameters.insert(key.to_string(), scalar_value(raw));
    }

    if !parameters.is_empty() {
        // DSML's `<parameter name="arguments">{json}</parameter>` envelope.
        if parameters.len() == 1
            && let Some(envelope @ serde_json::Value::Object(_)) = parameters.get("arguments")
        {
            return envelope.clone();
        }
        return serde_json::Value::Object(parameters);
    }

    let stripped = ORPHAN_PARAMETER_CLOSE_RE.as_ref().map_or_else(
        || body.to_string(),
        |re| re.replace_all(body, "").into_owned(),
    );
    let stripped = stripped.trim();
    if stripped.is_empty() {
        return serde_json::json!({});
    }
    if let Some(object) = recover_object(stripped) {
        return object;
    }
    serde_json::json!({ "input": stripped })
}

/// A parameter value: JSON when it parses as a number, bool, null, array or
/// object; otherwise the trimmed text.
fn scalar_value(raw: &str) -> serde_json::Value {
    let trimmed = raw.trim();
    match serde_json::from_str::<serde_json::Value>(trimmed) {
        Ok(
            value @ (serde_json::Value::Number(_)
            | serde_json::Value::Bool(_)
            | serde_json::Value::Null
            | serde_json::Value::Array(_)
            | serde_json::Value::Object(_)),
        ) => value,
        _ => serde_json::Value::String(trimmed.to_string()),
    }
}
