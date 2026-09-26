//! `function_call:{...}` envelopes narrated in ordinary assistant text.
//!
//! Some native-tool models put the call in the visible message as a compact
//! JSON envelope instead of filling the provider's structured field. The
//! explicit prefix is the marker: unlike the bare-JSON path, this grammar may
//! remove only the marked object from surrounding prose.

use super::{Block, Decoded, Grammar, Probe, ScanMode, find_ci};
use crate::parse::json_values::find_json_end;
use crate::repair::args;
use crate::types::{CallSource, ParseOptions, ParsedToolCall};

/// The marker used by the affected native-model response shape.
const PREFIX: &str = "function_call:";

/// Grammar for a marked JSON call in ordinary text.
#[derive(Debug)]
pub(crate) struct FunctionCall;

impl Grammar for FunctionCall {
    fn source(&self) -> CallSource {
        // This is another explicit JSON marker, so it shares the tagged
        // source classification used by the existing JSON call grammars.
        CallSource::TaggedJson
    }

    fn probe(&self, text: &str, from: usize, _options: &ParseOptions<'_>, mode: ScanMode) -> Probe {
        let mut cursor = from;
        while let Some(start) = find_ci(text, PREFIX, cursor) {
            let after = &text[start + PREFIX.len()..];
            if !after.trim_start().starts_with('{') {
                if mode == ScanMode::Stream && after.trim_start().is_empty() {
                    return Probe::Pending { start };
                }
                cursor = start + PREFIX.len();
                continue;
            }

            let Some(end) = find_json_end(after) else {
                if mode == ScanMode::Stream {
                    return Probe::Pending { start };
                }
                // Batch input may contain an abandoned marker before a real
                // call. There is no balanced boundary to skip to, so resume
                // at the marker's payload and let the next explicit marker
                // make progress.
                cursor = start + PREFIX.len();
                continue;
            };
            let block_end = start + PREFIX.len() + end;
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&after[..end]) else {
                // Skip a balanced but invalid object as one unit. This
                // prevents a marker in its payload from becoming a nested
                // call while still allowing a later real marker in the
                // response to be considered.
                cursor = block_end;
                continue;
            };
            let Some(call) = decode_call(&value) else {
                cursor = block_end;
                continue;
            };

            return Probe::Found(Block {
                start,
                end: block_end,
                decoded: Decoded::Calls(vec![call]),
            });
        }

        Probe::None
    }

    fn openers(&self) -> &'static [&'static str] {
        &[PREFIX]
    }
}

/// Reads the two observed name spellings without treating arbitrary JSON as a
/// call. The marker already establishes the call context, so the usual tagged
/// argument aliases remain safe here.
fn decode_call(value: &serde_json::Value) -> Option<ParsedToolCall> {
    let object = value.as_object()?;
    let name = object
        .get("call")
        .or_else(|| object.get("name"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())?;
    Some(ParsedToolCall::new(
        name,
        args::from_call_object(value),
        CallSource::TaggedJson,
    ))
}
