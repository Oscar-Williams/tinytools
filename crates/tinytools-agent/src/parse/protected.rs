//! Spans of a response in which nothing is a tool call.
//!
//! A model explaining a tool protocol, quoting a transcript, or writing a
//! shell script that happens to contain `<tool_call>` puts that text inside a
//! fenced code block. Dispatching a call from there would execute an
//! *example*. So a fence with a language tag protects its contents from every
//! grammar in [`crate::parse`].
//!
//! Two deliberate exceptions keep real calls parseable:
//!
//! * a fence whose language *is* a tool-call marker (```` ```tool_call ````),
//!   or whose info string opens with a call tag (```` ```<tool_call> ````),
//!   is a call, not an example, and is handled by the grammars;
//! * a fence with **no** language tag is not protected. Small models wrap a
//!   genuine call in a bare fence far more often than they quote one, and a
//!   quoted example almost always carries a language.
//!
//! An unclosed fence protects to the end of the text.

use std::ops::Range;

#[cfg(test)]
mod test;

/// Info-string languages that mark a fence as a tool call rather than a code
/// example.
pub const TOOL_CALL_LANGUAGES: &[&str] =
    &["tool_call", "toolcall", "tool-call", "invoke", "tool_calls"];

/// Byte ranges of protected fenced blocks, in order, non-overlapping. An
/// unclosed trailing fence is included, protecting to the end of `text`.
#[must_use]
pub fn fence_ranges(text: &str) -> Vec<Range<usize>> {
    scan_fences(text).0
}

/// The start of a fence that is still open at the end of `text`, if any.
///
/// Used by the stream scrubber: a fence opener with no closing fence yet
/// must not be released as safe narrative text, because the fragments that
/// close it — and the protected content in between — have not arrived. A
/// fence that has already closed returns `None`, even though its content is
/// still reported by [`fence_ranges`].
#[must_use]
pub fn open_fence_start(text: &str) -> Option<usize> {
    scan_fences(text).1
}

/// One pass over `text` tracking fence state. Returns every closed fenced
/// range plus the start of a trailing fence still open when the text ends.
fn scan_fences(text: &str) -> (Vec<Range<usize>>, Option<usize>) {
    let mut ranges = Vec::new();
    let mut open: Option<(usize, char, usize)> = None; // (start, fence char, fence len)
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let line_start = offset;
        offset += line.len();
        let stripped = line.trim_start_matches(' ');
        if line.len() - stripped.len() > 3 {
            continue;
        }
        let Some(fence_char) = stripped.chars().next().filter(|c| *c == '`' || *c == '~') else {
            continue;
        };
        let fence_len = stripped.chars().take_while(|c| *c == fence_char).count();
        if fence_len < 3 {
            continue;
        }
        let info = stripped[fence_len..].trim();
        match open {
            None => {
                let language = info.split_whitespace().next().unwrap_or("");
                let is_tool_call = TOOL_CALL_LANGUAGES
                    .iter()
                    .any(|lang| lang.eq_ignore_ascii_case(language))
                    || super::grammar::tagged::opens_with_call_tag(info);
                if !language.is_empty() && !is_tool_call {
                    open = Some((line_start, fence_char, fence_len));
                }
            }
            Some((start, open_char, open_len)) => {
                // CommonMark forbids an info string on a closing fence; here
                // it is allowed, because a model closing a fenced argument
                // block often puts the next protocol marker on the same
                // line (```` ```<｜tool▁call▁end｜> ````).
                if fence_char == open_char && fence_len >= open_len {
                    // The range ends at the closing backticks, not the line
                    // end, so a marker on the same line is scanned.
                    let indent = line.len() - stripped.len();
                    let fence_bytes: usize =
                        stripped.chars().take(fence_len).map(char::len_utf8).sum();
                    ranges.push(start..line_start + indent + fence_bytes);
                    open = None;
                }
            }
        }
    }
    if let Some((start, _, _)) = open {
        ranges.push(start..text.len());
    }
    (ranges, open.map(|(start, _, _)| start))
}

/// Whether `position` falls inside any of `ranges`.
#[must_use]
pub fn is_protected(ranges: &[Range<usize>], position: usize) -> bool {
    ranges.iter().any(|range| range.contains(&position))
}

/// The end of the protected range containing `position`, if any.
#[must_use]
pub fn protected_end(ranges: &[Range<usize>], position: usize) -> Option<usize> {
    ranges
        .iter()
        .find(|range| range.contains(&position))
        .map(|range| range.end)
}
