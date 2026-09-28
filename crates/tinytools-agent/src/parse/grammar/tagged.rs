//! `<tool_call>…</tool_call>` and everything models do to it.
//!
//! The baseline text protocol — JSON (or P-Format) inside a tag — arrives in
//! more spellings than any other grammar because every chat template has its
//! own, and gateways garble the tag markers themselves:
//!
//! * spelling variants `<toolcall>`, `<tool-call>`, and the bare `<invoke>`;
//! * an attribute form `<tool_call id="call_0">` (Hermes / `DeepSeek`
//!   templates);
//! * sentinel pipes leaked into the markers, in any position:
//!   `<|tool_call>…<tool_call|>`, `<|tool_call|>…<|tool_call|>`,
//!   `…</tool_call|>`, including the fullwidth `｜` those templates
//!   actually emit;
//! * a `DeepSeek` DSML marker on the tag itself,
//!   `<｜DSML｜tool_call>…</｜DSML｜tool_call>` — the same marker
//!   [`super::invoke_xml`] already accepts on `<invoke>`, which this family
//!   used to miss, so a `deepseek` turn that chose the tag form over the
//!   invoke form parsed as prose and the call was silently dropped;
//! * a `call:` prefix before the body;
//! * a fenced block instead of a tag, ```` ```tool_call … ``` ````, sometimes
//!   closed by a stray `</tool_call>`;
//! * a body that is a Kimi `NAME{…}` object with unquoted keys and `<|"|>`
//!   quote sentinels;
//! * a body wrapped in its own ```` ```json ```` fence.
//!
//! Tags are paired **positionally**: the first tag-family marker after the
//! cursor opens a block, the next one closes it. That is what makes the
//! garbled forms parse without a per-variant open/close table, and it is
//! safe because these tags never nest.

use std::sync::LazyLock;

use regex::Regex;

use super::{Block, Decoded, Grammar, Probe, ScanMode, find_ci, pending_opener, prefer_pending};
use crate::parse::call_object::{AliasPolicy, read_calls};
use crate::parse::json_values::{
    extract_first_json_value_with_end, extract_json_values, find_json_end,
};
use crate::repair::json::{recover_object, strip_code_fence};
use crate::types::{CallSource, ParseOptions, ParsedToolCall};

/// The tagged-JSON grammar.
#[derive(Debug)]
pub(crate) struct Tagged;

/// Any tag-family marker: `<tool_call>`, `<toolcall>`, `<tool-call>`, with
/// pipes (ASCII `|` or the fullwidth `｜` chat templates emit), a slash,
/// whitespace, or a `DeepSeek` DSML marker leaked in, and an optional
/// attribute list. `<tool_calls>` (plural, a JSON key, and the DSML wrapper
/// element) and `<tool_callable>` do not match: the name must end at a pipe,
/// slash, whitespace, or `>`.
static TAG_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?i)<[|\u{ff5c}/\s]*(?:DSML[|\u{ff5c}/\s]*)?tool[_-]?call(?:[|\u{ff5c}/\s]*|\s+[^>]*)>",
    )
    .ok()
});

/// The bare `<invoke>` literal — no attributes — with the optional `DeepSeek`
/// DSML marker or XML namespace the named form already tolerates
/// ([`super::invoke_xml`]'s `PREFIX`).
///
/// The prefix used to be absent here: the opener was a literal `"<invoke>"`
/// match and the closer a literal `"</invoke>"`, so `<｜DSML｜ invoke>` — a
/// `deepseek` turn that emitted the invoke form *without* a `name` attribute,
/// carrying the name in the JSON body instead — opened no block and the call
/// was dropped as prose. The named spelling `<｜DSML｜invoke name="x">` parsed
/// fine, and so did the unprefixed bare `<invoke>`; only the combination of
/// the two accommodations was missing.
///
/// Attributes are excluded on purpose: `<invoke name="x">` belongs to
/// [`super::invoke_xml`], which reads the name off the tag. This matches only
/// the attribute-less form, whose name can come from the body.
static BARE_INVOKE_OPEN_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)<(?:[|\u{ff5c}]{1,2}\s*DSML\s*[|\u{ff5c}]{1,2}\s*|[a-z_][\w.-]*:)?invoke\s*>")
        .ok()
});

/// A bare invoke closer with the same permitted prefix shapes as its opener.
static BARE_INVOKE_CLOSE_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(r"(?i)</(?:[|\u{ff5c}]{1,2}\s*DSML\s*[|\u{ff5c}]{1,2}\s*|[a-z_][\w.-]*:)?invoke\s*>")
        .ok()
});

/// A complete named invoke or function opener accepted by `invoke_xml`.
///
/// This is used only as a recovery boundary after a complete JSON value. The
/// tagged grammar must leave that later call for `invoke_xml` to decode.
static NAMED_INVOKE_OPEN_RE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)<(?:[|\u{ff5c}]{1,2}\s*DSML\s*[|\u{ff5c}]{1,2}\s*|[a-z_][\w.-]*:)?(?:invoke|function)(?:\s+[^>]*?\bname\s*=\s*"[^"]*"[^>]*|\s*=\s*[^\s>,]+[^>]*)>"#,
    )
    .ok()
});

/// First match of `re` in `haystack`, as `(start, end)`.
fn find_re(re: &LazyLock<Option<Regex>>, haystack: &str) -> Option<(usize, usize)> {
    re.as_ref()
        .and_then(|re| re.find(haystack))
        .map(|m| (m.start(), m.end()))
}

/// Finds the closer that has the exact prefix and spelling of `opener`.
///
/// A bare `<invoke>` must not be closed by `</atem:invoke>` embedded in its
/// JSON body. When the body begins with valid JSON, skip that whole value too:
/// a matching-looking closer in a JSON string is data rather than markup.
fn matching_invoke_close(opener: &str, after: &str) -> Option<(usize, usize)> {
    let json_end = find_json_end(after)
        .filter(|&end| serde_json::from_str::<serde_json::Value>(&after[..end]).is_ok());
    let start = json_end.unwrap_or(0);
    let opener = normalized_invoke_marker(opener);
    BARE_INVOKE_CLOSE_RE.as_ref().and_then(|re| {
        re.find_iter(&after[start..])
            .find(|close| normalized_invoke_marker(close.as_str()) == opener)
            .map(|close| (start + close.start(), start + close.end()))
    })
}

/// Normalizes an invoke marker enough to compare its semantic prefix.
fn normalized_invoke_marker(marker: &str) -> String {
    marker
        .chars()
        .filter(|ch| !matches!(ch, '<' | '>' | '/') && !ch.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Finds a bare invoke's closer unless a complete named successor comes first.
fn invoke_close(opener: &str, after: &str) -> Option<(usize, usize)> {
    let close = matching_invoke_close(opener, after);
    let successor = named_invoke_boundary(after);
    if successor.is_some_and(|start| close.is_none_or(|(end, _)| start < end)) {
        None
    } else {
        close
    }
}

/// The start of a later block that is safe to parse after an unterminated,
/// undecodable tagged block.
///
/// A successfully decoded leading JSON value is the only reliable delimiter
/// available without a matching outer tag. It prevents an `<invoke …>` inside
/// a rejected JSON string from becoming an executable nested call.
fn recovery_boundary(text: &str, body_start: usize) -> Option<usize> {
    let after = &text[body_start..];
    let json_end = find_json_end(after)
        .filter(|&end| serde_json::from_str::<serde_json::Value>(&after[..end]).is_ok())?;
    let from = body_start + json_end;
    let tagged = next_opener(text, from).map(|opener| opener.start);
    let named = find_re(&NAMED_INVOKE_OPEN_RE, &text[from..]).map(|(start, _)| from + start);
    [tagged, named].into_iter().flatten().min()
}

/// The first named invoke after a valid leading JSON value in `text`.
fn named_invoke_boundary(text: &str) -> Option<usize> {
    let json_end = find_json_end(text)
        .filter(|&end| serde_json::from_str::<serde_json::Value>(&text[..end]).is_ok())?;
    find_re(&NAMED_INVOKE_OPEN_RE, &text[json_end..]).map(|(start, _)| json_end + start)
}

/// Openers a fenced block can carry. `` ```tool_calls `` (plural) is listed
/// separately from `` ```tool_call `` rather than relying on a prefix match:
/// `next_opener` requires the language to end exactly at the literal, so
/// without its own entry the plural spelling — which
/// [`crate::parse::protected::TOOL_CALL_LANGUAGES`] already classifies as a
/// call language, not a protected example — would never be recognized here.
const FENCE_OPENERS: &[&str] = &[
    "```tool_call",
    "```toolcall",
    "```tool-call",
    "```tool_calls",
    "```invoke",
];

/// Kimi-family argument-quote sentinel that leaks in place of `"`.
const ARG_QUOTE_SENTINEL: &str = "<|\"|>";

/// A located opener.
struct Opener {
    start: usize,
    body_start: usize,
    kind: OpenerKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenerKind {
    /// A tag-family marker; closed by the next tag-family marker.
    Tag,
    /// The bare `<invoke>` literal; closed by `</invoke>`.
    Invoke,
    /// A fenced block; closed by ```` ``` ```` or a stray closing tag.
    Fence,
}

impl Grammar for Tagged {
    fn source(&self) -> CallSource {
        CallSource::TaggedJson
    }

    fn probe(&self, text: &str, from: usize, options: &ParseOptions<'_>, mode: ScanMode) -> Probe {
        let pending = pending_opener(
            text,
            from,
            &["<tool_call", "<toolcall", "<tool-call", "<|tool_call"],
            ">",
            mode,
        );
        prefer_pending(Self::probe_decided(text, from, options, mode), pending)
    }

    fn openers(&self) -> &'static [&'static str] {
        &[
            "<tool_call",
            "<toolcall",
            "<tool-call",
            "<|tool_call",
            "<invoke>",
            "```tool_call",
            "```toolcall",
            "```tool-call",
            "```tool_calls",
            "```invoke",
        ]
    }
}

impl Tagged {
    /// The next block whose opener is fully present.
    fn probe_decided(text: &str, from: usize, options: &ParseOptions<'_>, mode: ScanMode) -> Probe {
        let opener = next_opener(text, from);
        if let Some(noise) = orphan_closer_noise(text, from, opener.as_ref()) {
            return noise;
        }

        let Some(opener) = opener else {
            return Probe::None;
        };
        let mut body_start = opener.body_start;

        let (close, skipped) = match opener.kind {
            OpenerKind::Tag => {
                let (close, skipped, new_body_start) =
                    tag_close_skipping_doubled_openers(text, body_start);
                body_start = new_body_start;
                (close, skipped)
            }
            OpenerKind::Invoke => {
                let after = &text[body_start..];
                (invoke_close(&text[opener.start..body_start], after), 0)
            }
            OpenerKind::Fence => (fence_close(&text[body_start..]), 0),
        };
        let after = &text[body_start..];

        if let Some((body_end, close_end)) = close {
            let body = &after[..body_end];
            let rest = &after[close_end..];
            // Swallow only the closers a doubled opener left behind — never
            // an unrelated closing tag such as `</div>` — so no stray
            // `</tool_call>` survives into the visible text while narrative
            // markup after a normal call is left untouched.
            let end = if opener.kind == OpenerKind::Tag && skipped > 0 {
                match swallow_extra_closers(rest, skipped, mode) {
                    Some(consumed) => body_start + close_end + consumed,
                    // Streaming: more input could still bring the matching
                    // closer, so the block is not safe to finalize yet.
                    None => {
                        return Probe::Pending {
                            start: opener.start,
                        };
                    }
                }
            } else {
                body_start + close_end
            };
            let calls = decode_body(body, options);
            let decoded = if calls.is_empty() {
                Decoded::Malformed {
                    body_chars: body.chars().count(),
                }
            } else {
                Decoded::Calls(calls)
            };
            return Probe::Found(Block {
                start: opener.start,
                end,
                decoded,
            });
        }

        if mode == ScanMode::Stream {
            return Probe::Pending {
                start: opener.start,
            };
        }

        // Batch: an opener with nothing after it is a call the model started
        // and never wrote (a truncated or abandoned block). There is nothing
        // to recover and nothing worth showing, so it is dropped rather than
        // left in the visible text as a bare `<tool_call>`.
        if opener.kind == OpenerKind::Tag && after.trim().is_empty() {
            return Probe::Found(Block {
                start: opener.start,
                end: text.len(),
                decoded: Decoded::Malformed { body_chars: 0 },
            });
        }

        // Batch: no closer. Recover a balanced JSON body if one starts here.
        let recovered = find_json_end(after)
            .and_then(|json_end| {
                serde_json::from_str::<serde_json::Value>(&after[..json_end])
                    .ok()
                    .map(|value| (value, json_end))
            })
            .or_else(|| extract_first_json_value_with_end(after));
        if let Some((value, consumed)) = recovered {
            let calls = read_calls(
                &value,
                AliasPolicy::Marked,
                &|name| options.knows(name),
                CallSource::TaggedJson,
            );
            if !calls.is_empty() {
                // No tag-family marker exists anywhere after this opener (the
                // TAG_RE scan above found none), so nothing here is protocol
                // furniture to clean up — stopping at the JSON boundary
                // leaves any trailing markup, tool-call-related or not, in
                // the narrative rather than guessing which closer it was.
                let end = body_start + consumed;
                return Probe::Found(Block {
                    start: opener.start,
                    end,
                    decoded: Decoded::Calls(calls),
                });
            }
        }
        // Nothing recoverable in this block. A complete JSON value establishes
        // a structural boundary after the malformed call. Only then may a
        // later opener start a new scan: markup inside the JSON value is data,
        // never a nested call to execute.
        //
        // That is not hypothetical. A `deepseek` turn emitted an unterminated
        // `<tool_call>` whose body carried `{"arguments":{…}}` with no name —
        // unrecoverable, correctly — immediately followed by a complete
        // `<｜DSML｜ invoke>` call. Swallowing to end-of-text dropped the good
        // call with the bad one and the whole response parsed as prose. A
        // block that failed to decode must not be allowed to bury its
        // successors.
        //
        let end = recovery_boundary(text, body_start).unwrap_or(text.len());
        Probe::Found(Block {
            start: opener.start,
            end,
            decoded: Decoded::Verbatim,
        })
    }
}

/// Whether a tag-family marker is a closer (`</tool_call>`, `<|/tool_call|>`).
/// Skips the same whitespace class `TAG_RE`'s `\s` does (not just space and
/// tab), so a marker like `<\n/tool_call>` — which the regex matches as one
/// marker — is still recognized as a closer here.
fn is_closing_marker(marker: &str) -> bool {
    marker[1..]
        .trim_start_matches(|c: char| c == '|' || c.is_whitespace())
        .starts_with('/')
}

/// Canonical closer spellings [`swallow_extra_closers`] holds a partial
/// match of in stream mode. Not exhaustive of everything `TAG_RE` accepts
/// (arbitrary interleaved pipes and whitespace) — the same practical
/// trade-off [`pending_opener`]'s literal list already makes for openers.
const CLOSER_PREFIXES: &[&str] = &["</tool_call", "</toolcall", "</tool-call", "<|/tool_call"];

/// Whether `trimmed` — which `TAG_RE` did not match as a complete marker —
/// could still grow into a tag-family closer once more input arrives: it has
/// no `>` yet and is a case-insensitive prefix of one of [`CLOSER_PREFIXES`].
fn could_still_become_a_closer(trimmed: &str) -> bool {
    if trimmed.contains('>') {
        return false;
    }
    CLOSER_PREFIXES.iter().any(|literal| {
        let n = trimmed.len().min(literal.len());
        trimmed.is_char_boundary(n) && trimmed[..n].eq_ignore_ascii_case(&literal[..n])
    })
}

/// Swallows up to `max` tag-family closers from the front of `rest`
/// (whitespace between them ignored), returning the byte count consumed.
/// Only a recognized closer — matched by [`TAG_RE`], the same grammar as
/// every opener — is ever eaten, so unrelated markup such as `</div>` is
/// left for the narrative. In [`ScanMode::Stream`], `None` means the text
/// ends, or breaks off mid-marker, before it is clear whether another closer
/// is still coming, so the caller must hold the block back rather than
/// finalize it early and let a partial marker such as `</tool_` leak out as
/// text before its `call>` tail arrives in a later fragment.
fn swallow_extra_closers(rest: &str, max: usize, mode: ScanMode) -> Option<usize> {
    let re = TAG_RE.as_ref()?;
    let mut consumed = 0usize;
    for _ in 0..max {
        let after = &rest[consumed..];
        let trimmed = after.trim_start();
        let skipped_ws = after.len() - trimmed.len();
        if trimmed.is_empty() {
            // Nothing here yet: in batch mode that is simply the end of the
            // response, in stream mode a closer could still be on its way.
            return if mode == ScanMode::Stream {
                None
            } else {
                Some(consumed)
            };
        }
        let Some(m) = re.find(trimmed) else {
            return if mode == ScanMode::Stream && could_still_become_a_closer(trimmed) {
                None
            } else {
                Some(consumed)
            };
        };
        if m.start() != 0 {
            return Some(consumed);
        }
        if !is_closing_marker(m.as_str()) {
            return Some(consumed);
        }
        consumed += skipped_ws + m.end();
    }
    Some(consumed)
}

/// A closing tag-family marker at or after `from` that sits before `opener`
/// (or has no opener after it at all) — it cannot be closing anything, since
/// [`next_opener`] never treats a closer as an opener to pair it with. A
/// model that made its actual call over a structured/native channel has no
/// `<tool_call>` opener in its text at all, but sometimes still types a
/// habitual closer in its narrative; this sweeps it as furniture rather than
/// leaving literal markup in the visible text.
fn orphan_closer_noise(text: &str, from: usize, opener: Option<&Opener>) -> Option<Probe> {
    let re = TAG_RE.as_ref()?;
    let hay = &text[from..];
    let m = re.find(hay)?;
    if !is_closing_marker(m.as_str()) {
        return None;
    }
    let start = from + m.start();
    if opener.is_some_and(|o| start >= o.start) {
        return None;
    }
    Some(Probe::Found(Block {
        start,
        end: from + m.end(),
        decoded: Decoded::Noise,
    }))
}

/// The closer for a tag opener at `body_start`, skipping past a doubled
/// opener first, and how many extra openers it skipped — so the matching
/// number of extra closers, never an unrelated closing tag such as `</div>`,
/// can be swallowed by the caller. `DeepSeek` V4 doubles both the opener and
/// the closer under a code dialect:
/// `<tool_call>\n<tool_call>\nNAME(...)\n</tool_call>\n</tool_call>`.
///
/// Positional pairing means a doubled opener would otherwise close the first
/// tag on an empty body and lose the call. An opener followed by nothing but
/// whitespace is the same block starting again, so the scan moves past it.
/// Returns the closer (if any), how many openers it skipped, and the
/// possibly-advanced body start.
fn tag_close_skipping_doubled_openers(
    text: &str,
    mut body_start: usize,
) -> (Option<(usize, usize)>, usize, usize) {
    let re = TAG_RE.as_ref();
    let mut skipped = 0usize;
    let close = loop {
        let after = &text[body_start..];
        let Some(m) = re.and_then(|re| re.find(after)) else {
            break None;
        };
        let is_opener = !is_closing_marker(m.as_str());
        if is_opener && after[..m.start()].trim().is_empty() {
            body_start += m.end();
            skipped += 1;
            continue;
        }
        break Some((m.start(), m.end()));
    };
    (close, skipped, body_start)
}

/// The earliest opener at or after `from`: a non-closing tag-family marker,
/// the bare `<invoke>` literal, or a fence opener.
fn next_opener(text: &str, from: usize) -> Option<Opener> {
    let mut best: Option<Opener> = None;
    let consider = |best: &mut Option<Opener>, candidate: Opener| {
        if best.as_ref().is_none_or(|b| candidate.start < b.start) {
            *best = Some(candidate);
        }
    };

    if let Some(re) = TAG_RE.as_ref() {
        for m in re.find_iter(&text[from..]) {
            // A marker with a slash is a closer, never an opener.
            if is_closing_marker(m.as_str()) {
                continue;
            }
            consider(
                &mut best,
                Opener {
                    start: from + m.start(),
                    body_start: from + m.end(),
                    kind: OpenerKind::Tag,
                },
            );
            break;
        }
    }

    if let Some((start, end)) = find_re(&BARE_INVOKE_OPEN_RE, &text[from..]) {
        consider(
            &mut best,
            Opener {
                start: from + start,
                body_start: from + end,
                kind: OpenerKind::Invoke,
            },
        );
    }

    for fence in FENCE_OPENERS {
        let mut cursor = from;
        while let Some(idx) = find_ci(text, fence, cursor) {
            let after = &text[idx + fence.len()..];
            // The language must end here (`tool_call` not `tool_calls`), and
            // the body starts on the next line.
            let rest = after.trim_start_matches([' ', '\t']);
            if let Some(nl) = rest
                .strip_prefix('\n')
                .or_else(|| rest.strip_prefix("\r\n"))
            {
                consider(
                    &mut best,
                    Opener {
                        start: idx,
                        body_start: text.len() - nl.len(),
                        kind: OpenerKind::Fence,
                    },
                );
                break;
            }
            cursor = idx + fence.len();
        }
    }

    best
}

/// The closer of a fenced block: a closing fence, a stray tag-family closer,
/// or bare invoke closer, whichever comes first.
fn fence_close(after: &str) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize)> = None;
    let mut consider = |candidate: Option<(usize, usize)>| {
        if let Some(c) = candidate
            && best.is_none_or(|b| c.0 < b.0)
        {
            best = Some(c);
        }
    };
    consider(after.find("```").map(|i| (i, i + 3)));
    consider(
        TAG_RE
            .as_ref()
            .and_then(|re| re.find(after))
            .filter(|m| {
                m.as_str()[1..]
                    .trim_start_matches(['|', ' '])
                    .starts_with('/')
            })
            .map(|m| (m.start(), m.end())),
    );
    consider(find_re(&BARE_INVOKE_CLOSE_RE, after));
    best
}

/// Everything a tag body can be, tried in order.
pub(crate) fn decode_body(body: &str, options: &ParseOptions<'_>) -> Vec<ParsedToolCall> {
    let body = strip_call_prefix(body);
    let is_known = |name: &str| options.knows(name);

    let calls = super::invoke_xml::decode_body(strip_code_fence(body));
    if !calls.is_empty() {
        return calls;
    }

    if let Some(registry) = options.registry {
        if let Some((name, arguments)) = crate::pformat::parse_call(body, registry) {
            return vec![ParsedToolCall::new(name, arguments, CallSource::PFormat)];
        }
        // Code-style calls, after P-Format and before the JSON paths. A
        // ```python fence *inside* the tag is unwrapped, the same courtesy the
        // JSON body gets below; a fence at top level never reaches here.
        let calls = crate::codecall::parse_calls(strip_code_fence(body), registry);
        if !calls.is_empty() {
            return calls
                .into_iter()
                .map(|(name, arguments)| ParsedToolCall::new(name, arguments, CallSource::Code))
                .collect();
        }
    }

    if let Some(recovered) = recover_sentinel_body(body)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(&recovered)
    {
        let calls = read_calls(
            &value,
            AliasPolicy::Marked,
            &is_known,
            CallSource::TaggedJson,
        );
        if !calls.is_empty() {
            return calls;
        }
    }

    let unfenced = strip_code_fence(body);
    let mut calls = Vec::new();
    for value in extract_json_values(unfenced) {
        calls.extend(read_calls(
            &value,
            AliasPolicy::Marked,
            &is_known,
            CallSource::TaggedJson,
        ));
    }
    if !calls.is_empty() {
        return calls;
    }

    if let Some(value) = recover_object(unfenced) {
        let calls = read_calls(
            &value,
            AliasPolicy::Marked,
            &is_known,
            CallSource::TaggedJson,
        );
        if !calls.is_empty() {
            return calls;
        }
    }

    super::glm::parse_lines(body)
}

/// Strips a leading `call:` some models emit right after the open tag.
fn strip_call_prefix(body: &str) -> &str {
    let trimmed = body.trim();
    trimmed
        .strip_prefix("call:")
        .map_or(trimmed, str::trim_start)
}

/// Recovers a Kimi-K2-family `NAME{…}` body — the action name before a
/// JSON-ish object with unquoted keys and `<|"|>` in place of string quotes —
/// into canonical `{"name":…,"arguments":…}` JSON. `None` when the shape does
/// not match: a body already starting with `{`, a P-Format `NAME[…]` body,
/// or trailing text after the object all fall through unchanged.
pub(crate) fn recover_sentinel_body(body: &str) -> Option<String> {
    let repaired = body.replace(ARG_QUOTE_SENTINEL, "\"");
    let trimmed = repaired.trim();
    let brace = trimmed.find('{')?;
    let name = trimmed[..brace].trim();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let object = trimmed[brace..].trim_end();
    if !object.starts_with('{') || !object.ends_with('}') {
        return None;
    }
    let arguments = recover_object(object)?;
    serde_json::to_string(&serde_json::json!({ "name": name, "arguments": arguments })).ok()
}
