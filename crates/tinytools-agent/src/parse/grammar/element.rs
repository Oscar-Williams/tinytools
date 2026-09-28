//! `<NAME><param>value</param></NAME>` — a call written as plain XML
//! elements: the tool name as the tag, each parameter as a child.
//!
//! `DeepSeek` V4 Flash writes `todo` calls this way under the Python code
//! dialect, `<todo>\n<todos>\n[{…}]\n</todos>\n</todo>`, alongside proper
//! `<tool_call>` blocks. Any tag could be a tool name, so the grammar is
//! gated hard to keep prose markup from dispatching:
//!
//! * the tag is an offered tool **with a registry entry** — the registry is
//!   the only place parameter names are known;
//! * the matching `</NAME>` is present;
//! * the body is child elements and whitespace, nothing else.
//!
//! A block passing all three is claimed. It decodes to a call when every
//! child is a parameter of the tool and every `[`/`{` value is valid JSON;
//! otherwise it is malformed, so the caller hears about the dropped call. A
//! block failing the gate is left in the text untouched.
//!
//! ponytail: registry-gated, so the Xml dialect (no registry) never sees an
//! element call; accept known-tool + child-only there if its models start
//! writing the form.

use std::sync::LazyLock;

use regex::Regex;

use super::{Block, Decoded, Grammar, Probe, ScanMode, prefer_pending};
use crate::pformat::PFormatRegistry;
use crate::types::{CallSource, ParseOptions, ParsedToolCall};

/// The element grammar.
#[derive(Debug)]
pub(crate) struct Element;

/// An attribute-less opening tag.
static OPEN_RE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"<([A-Za-z_][\w.-]*)>").ok());

/// Tag names other grammars own; never element calls.
const RESERVED: &[&str] = &[
    "tool_call",
    "toolcall",
    "tool-call",
    "tool_calls",
    "function_calls",
    "calls",
    "invoke",
    "function",
    "parameter",
];

impl Grammar for Element {
    fn source(&self) -> CallSource {
        CallSource::Element
    }

    fn probe(&self, text: &str, from: usize, options: &ParseOptions<'_>, mode: ScanMode) -> Probe {
        let (Some(open_re), Some(registry)) = (OPEN_RE.as_ref(), options.registry) else {
            return Probe::None;
        };
        let eligible = |name: &str| {
            registry.contains_key(name) && options.knows(name) && !RESERVED.contains(&name)
        };
        prefer_pending(
            Self::probe_decided(text, from, mode, open_re, registry, &eligible),
            (mode == ScanMode::Stream)
                .then(|| partial_opener(text, from, registry, &eligible))
                .flatten(),
        )
    }

    fn openers(&self) -> &'static [&'static str] {
        &[]
    }
}

impl Element {
    /// The next claimed block at or after `from`.
    fn probe_decided(
        text: &str,
        from: usize,
        mode: ScanMode,
        open_re: &Regex,
        registry: &PFormatRegistry,
        eligible: &dyn Fn(&str) -> bool,
    ) -> Probe {
        for open in open_re.captures_iter(&text[from..]) {
            let (Some(tag), Some(name)) = (open.get(0), open.get(1)) else {
                continue;
            };
            let name = name.as_str();
            if !eligible(name) {
                continue;
            }
            let Some(params) = registry.get(name) else {
                continue;
            };
            let start = from + tag.start();
            let body_start = from + tag.end();
            let closer = format!("</{name}>");
            let Some(body_len) = text[body_start..].find(&closer) else {
                if mode == ScanMode::Stream {
                    return Probe::Pending { start };
                }
                continue;
            };
            let body = &text[body_start..body_start + body_len];
            let Some(children) = children(body) else {
                continue;
            };
            let decoded = decode(name, &params.names, &children).map_or(
                Decoded::Malformed {
                    body_chars: body.chars().count(),
                },
                |call| Decoded::Calls(vec![call]),
            );
            return Probe::Found(Block {
                start,
                end: body_start + body_len + closer.len(),
                decoded,
            });
        }
        Probe::None
    }
}

/// In a stream, a trailing `<na` that could still grow into an eligible
/// tool's opener. The shared scrubber holds back only the static openers
/// grammars list, and a tool name is not one, so without this the fragment
/// would be released as text before its `me>` arrived.
fn partial_opener(
    text: &str,
    from: usize,
    registry: &PFormatRegistry,
    eligible: &dyn Fn(&str) -> bool,
) -> Option<usize> {
    let start = from + text[from..].rfind('<')?;
    let partial = &text[start + 1..];
    if partial.contains('>') {
        return None;
    }
    registry
        .keys()
        .any(|name| name.starts_with(partial) && eligible(name))
        .then_some(start)
}

/// `(name, raw value)` for each child element, or `None` when the body holds
/// anything else — prose, an unclosed child — or no child at all.
fn children(body: &str) -> Option<Vec<(&str, &str)>> {
    let mut out = Vec::new();
    let mut rest = body.trim_start();
    while !rest.is_empty() {
        let inner = rest.strip_prefix('<')?;
        let name_end = inner.find('>')?;
        let name = &inner[..name_end];
        if name.is_empty()
            || !name
                .chars()
                .all(|c| c.is_alphanumeric() || "_.-".contains(c))
        {
            return None;
        }
        let after = &inner[name_end + 1..];
        let closer = format!("</{name}>");
        let value_end = after.find(&closer)?;
        out.push((name, &after[..value_end]));
        rest = after[value_end + closer.len()..].trim_start();
    }
    (!out.is_empty()).then_some(out)
}

/// The call, or `None` when a child is not a parameter or a JSON-looking
/// value does not parse.
fn decode(name: &str, params: &[String], children: &[(&str, &str)]) -> Option<ParsedToolCall> {
    let mut arguments = serde_json::Map::new();
    for (key, raw) in children {
        if !params.iter().any(|param| param == key) {
            return None;
        }
        let trimmed = raw.trim();
        let value = if trimmed.starts_with(['[', '{']) {
            serde_json::from_str(trimmed).ok()?
        } else {
            super::invoke_xml::scalar_value(trimmed)
        };
        arguments.insert((*key).to_string(), value);
    }
    Some(ParsedToolCall::new(
        name,
        serde_json::Value::Object(arguments),
        CallSource::Element,
    ))
}
