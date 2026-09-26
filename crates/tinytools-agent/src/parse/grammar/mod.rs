//! The surface syntaxes a tool call can arrive in, one module each.
//!
//! A grammar answers one question: *starting at a byte offset, where is the
//! next block I recognise, and what calls does it hold?* It does not know
//! about protected ranges, about other grammars, or about how the narrative
//! text is assembled — that is the scan engine in [`crate::parse`]. Keeping
//! grammars this narrow is what makes adding one a local change: a new file
//! here, a line in [`GRAMMARS`], and every caller — batch, streaming, every
//! dialect — sees it.
//!
//! Order in [`GRAMMARS`] only breaks ties between grammars whose openers sit
//! at the same byte; the engine otherwise takes the earliest opener.

pub(crate) mod bare_json;
pub(crate) mod function_call;
pub(crate) mod glm;
pub(crate) mod harmony;
pub(crate) mod invoke_xml;
pub(crate) mod mistral;
pub(crate) mod sentinel;
pub(crate) mod tagged;

use crate::types::{CallSource, ParseOptions, ParsedToolCall};

/// Whether the text may still grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScanMode {
    /// The response is complete: an opener with no closer is recovered where
    /// the grammar can (a balanced JSON body) and otherwise kept as text.
    Batch,
    /// More fragments may arrive: an opener with no closer is reported as
    /// pending so the caller holds it back.
    Stream,
}

/// What a recognised block decoded to.
#[derive(Debug)]
pub(crate) enum Decoded {
    /// One or more calls; the block is removed from the narrative.
    Calls(Vec<ParsedToolCall>),
    /// A recognised block whose body is not a call; removed from the
    /// narrative and reported.
    Malformed {
        /// Length of the body in characters.
        body_chars: usize,
    },
    /// Protocol furniture with no call of its own (a `<tool_calls>` wrapper
    /// tag, a stray closing tag); removed from the narrative silently.
    Noise,
    /// A recognised opener whose block cannot be decoded; kept in the
    /// narrative verbatim.
    Verbatim,
}

/// A block a grammar recognised.
#[derive(Debug)]
pub(crate) struct Block {
    /// Byte offset of the block's first byte.
    pub(crate) start: usize,
    /// Byte offset just past the block.
    pub(crate) end: usize,
    /// What it decoded to.
    pub(crate) decoded: Decoded,
}

/// The result of asking a grammar for its next block.
#[derive(Debug)]
pub(crate) enum Probe {
    /// Nothing recognised at or after the offset.
    None,
    /// A complete block.
    Found(Block),
    /// An opener at `start` with no closer yet (streaming only).
    Pending {
        /// Byte offset of the opener.
        start: usize,
    },
}

/// One surface syntax.
pub(crate) trait Grammar: Sync {
    /// Which [`CallSource`] this grammar produces.
    fn source(&self) -> CallSource;

    /// The next block at or after `from`.
    fn probe(&self, text: &str, from: usize, options: &ParseOptions<'_>, mode: ScanMode) -> Probe;

    /// Literal prefixes that open one of this grammar's blocks, used by the
    /// stream scrubber to hold back a partially received opener. Compared
    /// ASCII-case-insensitively.
    fn openers(&self) -> &'static [&'static str];
}

/// Every scan grammar, in tie-break order.
pub(crate) static GRAMMARS: &[&dyn Grammar] = &[
    &function_call::FunctionCall,
    &invoke_xml::InvokeXml,
    &sentinel::Sentinel,
    &harmony::Harmony,
    &mistral::Mistral,
    &tagged::Tagged,
];

/// Every opener prefix across all scan grammars.
pub(crate) fn all_openers() -> impl Iterator<Item = &'static str> {
    GRAMMARS
        .iter()
        .flat_map(|grammar| grammar.openers().iter().copied())
}

/// Case-insensitive `find` for an ASCII needle.
pub(crate) fn find_ci(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    let hay = haystack.as_bytes();
    let nee = needle.as_bytes();
    if nee.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - nee.len())
        .filter(|&i| haystack.is_char_boundary(i))
        .find(|&i| hay[i..i + nee.len()].eq_ignore_ascii_case(nee))
}

/// In [`ScanMode::Stream`], the earliest occurrence at or after `from` of one
/// of `literals` that cannot be decided yet: it is not the prefix of a longer
/// word and no `terminator` follows it anywhere in the text, so its block
/// cannot possibly be complete. `None` in batch mode.
pub(crate) fn pending_opener(
    text: &str,
    from: usize,
    literals: &[&str],
    terminator: &str,
    mode: ScanMode,
) -> Option<usize> {
    if mode != ScanMode::Stream {
        return None;
    }
    let mut earliest: Option<usize> = None;
    for literal in literals {
        let mut cursor = from;
        while let Some(idx) = find_ci(text, literal, cursor) {
            let after = idx + literal.len();
            let word_continues = text[after..]
                .chars()
                .next()
                .is_some_and(|c| c.is_ascii_alphanumeric());
            if !word_continues && !text[after..].contains(terminator) {
                if earliest.is_none_or(|e| idx < e) {
                    earliest = Some(idx);
                }
                break;
            }
            cursor = after;
        }
    }
    earliest
}

/// Prefers a pending opener over a later (or absent) decided block.
pub(crate) fn prefer_pending(probe: Probe, pending: Option<usize>) -> Probe {
    match (probe, pending) {
        (Probe::Found(block), Some(start)) if start < block.start => Probe::Pending { start },
        (Probe::None, Some(start)) => Probe::Pending { start },
        (probe, _) => probe,
    }
}
