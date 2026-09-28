//! Unit tests for the parse pipeline, one file per grammar plus the engine.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod bare_json;
mod element;
mod engine;
mod glm;
mod harmony_mistral;
mod invoke_xml;
mod sentinel;
mod tagged;

use crate::types::{ParseOptions, ParsedToolCall};

/// Parses with default options.
pub(super) fn parse(text: &str) -> (String, Vec<ParsedToolCall>) {
    crate::parse::parse_tool_calls(text)
}

/// Parses with the given known tools.
pub(super) fn parse_known(text: &str, known: &[&str]) -> crate::types::ParseOutcome {
    let known: Vec<String> = known.iter().map(ToString::to_string).collect();
    crate::parse::parse_text(text, &ParseOptions::new().with_known_tools(&known))
}
