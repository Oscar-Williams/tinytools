//! The vocabulary shared by every parser, repair, and renderer in this crate.

use serde_json::Value;

use crate::PFormatRegistry;

/// Which grammar a call was recovered through.
///
/// Carried on every [`ParsedToolCall`] so a host can log *how* a call reached
/// it — a run that only ever dispatches [`CallSource::Native`] calls behaves
/// very differently from one living on [`CallSource::Sentinel`] recoveries,
/// and the difference is invisible without this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CallSource {
    /// The provider's structured tool-call channel.
    Native,
    /// `<tool_call>{json}</tool_call>` and its spelling variants, including
    /// fenced ```` ```tool_call ```` blocks.
    TaggedJson,
    /// `<invoke name="…"><parameter name="…">` XML: Claude, `DeepSeek` DSML,
    /// namespaced variants, and `<function=…>` forms.
    InvokeXml,
    /// Chat-template sentinel tokens leaked verbatim: `DeepSeek`-R1
    /// `<｜tool▁call▁begin｜>` and Kimi `<|tool_call_begin|>`.
    Sentinel,
    /// gpt-oss Harmony `<|channel|>commentary to=…<|message|>…<|call|>`.
    Harmony,
    /// Mistral `[TOOL_CALLS]` blocks.
    Mistral,
    /// GLM `tool/param>value` lines.
    Glm,
    /// A whole response that is one JSON object or `tool_calls` envelope.
    BareJson,
    /// P-Format `name[index|value]` inside a tag.
    PFormat,
    /// A code-style call `name(arg="value")` inside a tag, registry-gated.
    Code,
    /// `<NAME><param>value</param></NAME>` elements, registry-gated.
    Element,
}

/// One model-requested tool invocation recovered from text or structured data.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedToolCall {
    /// Tool name as the model wrote it, after name repair when a known-tool
    /// set was supplied.
    pub name: String,
    /// Parsed tool arguments, defaulting to an empty object when absent.
    pub arguments: Value,
    /// Provider-assigned call id when the call came from a native
    /// tool-use response. `None` for every text-recovered call — this crate
    /// never mints ids, the host does, so two turns of one run can never
    /// collide on a per-response counter.
    pub id: Option<String>,
    /// The grammar the call was recovered through.
    pub source: CallSource,
}

impl ParsedToolCall {
    /// A text-recovered call with no provider id.
    #[must_use]
    pub fn new(name: impl Into<String>, arguments: Value, source: CallSource) -> Self {
        Self {
            name: name.into(),
            arguments,
            id: None,
            source,
        }
    }

    /// A call the provider reported natively, with its id.
    #[must_use]
    pub fn native(id: impl Into<String>, name: impl Into<String>, arguments: Value) -> Self {
        Self {
            name: name.into(),
            arguments,
            id: Some(id.into()),
            source: CallSource::Native,
        }
    }
}

/// What the caller knows that makes parsing safer.
///
/// Every field widens or narrows recovery. The default — no known tools, no
/// registry, bare JSON allowed — is what a caller with no context gets and is
/// safe, because the anti-phantom rules in [`crate::parse`] do not depend on
/// any of it. Supplying `known_tools` is what unlocks name repair and the
/// alias-tolerant bare-object path.
#[derive(Debug, Clone, Copy)]
pub struct ParseOptions<'a> {
    /// The tools the model was actually offered this turn. Enables name
    /// repair (`terminal" parameter=…` → `terminal`) and lets a bare JSON
    /// object naming a known tool use the argument-key aliases.
    pub known_tools: &'a [String],
    /// Positional layouts for the P-Format grammar. `None` disables it.
    pub registry: Option<&'a PFormatRegistry>,
    /// Whether a response that is *entirely* one JSON object or array may be
    /// read as a call. Off for callers whose model legitimately answers in
    /// JSON.
    pub allow_bare_json: bool,
}

impl Default for ParseOptions<'_> {
    /// Matches [`Self::new`]: a derived `Default` would leave `allow_bare_json`
    /// at `bool`'s `false`, contradicting the documented default above and
    /// silently disabling bare-JSON parsing for any caller that writes
    /// `ParseOptions::default()` instead of `ParseOptions::new()`.
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> ParseOptions<'a> {
    /// The permissive default: bare JSON allowed, no known tools, no registry.
    #[must_use]
    pub fn new() -> Self {
        Self {
            known_tools: &[],
            registry: None,
            allow_bare_json: true,
        }
    }

    /// Sets the tools the model was offered.
    #[must_use]
    pub fn with_known_tools(mut self, tools: &'a [String]) -> Self {
        self.known_tools = tools;
        self
    }

    /// Sets the P-Format registry.
    #[must_use]
    pub fn with_registry(mut self, registry: &'a PFormatRegistry) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Forbids the whole-response JSON path.
    #[must_use]
    pub fn without_bare_json(mut self) -> Self {
        self.allow_bare_json = false;
        self
    }

    /// Whether `name` is one of the offered tools. Always `false` when no
    /// tools were supplied, so callers can tell "unknown" from "unchecked" via
    /// [`Self::has_known_tools`].
    #[must_use]
    pub fn knows(&self, name: &str) -> bool {
        self.known_tools.iter().any(|known| known == name)
    }

    /// Whether a known-tool set was supplied at all.
    #[must_use]
    pub fn has_known_tools(&self) -> bool {
        !self.known_tools.is_empty()
    }
}

/// Why a span of model output was *not* turned into a call, or was changed on
/// the way.
///
/// Diagnostics never carry model output — only lengths and names — so a host
/// can log them at any level without leaking tool arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseDiagnostic {
    /// A recognised block whose body did not decode into a call and was
    /// dropped from the narrative.
    MalformedBlock {
        /// The grammar that recognised the block.
        source: CallSource,
        /// Length of the dropped body in characters.
        body_chars: usize,
    },
    /// A recognised opener with no closer; the span was kept as text.
    UnterminatedBlock {
        /// The grammar that recognised the opener.
        source: CallSource,
    },
    /// The model's tool name was rewritten to a known tool.
    NameRepaired {
        /// What the model wrote.
        from: String,
        /// What it was resolved to.
        to: String,
    },
    /// The tool name does not match any offered tool and could not be
    /// repaired. The call is still returned; the host's unknown-tool policy
    /// decides what happens to it.
    UnknownTool {
        /// The unresolved name.
        name: String,
    },
    /// Arguments were recovered from relaxed or damaged JSON.
    ArgumentsRepaired {
        /// The tool the arguments belong to.
        tool: String,
    },
}

/// The result of parsing one model response.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParseOutcome {
    /// The narrative text with every recognised block removed.
    pub text: String,
    /// The calls, in source order.
    pub calls: Vec<ParsedToolCall>,
    /// What was dropped, repaired, or left unresolved.
    pub diagnostics: Vec<ParseDiagnostic>,
}

impl ParseOutcome {
    /// Splits into the `(text, calls)` pair the dialect API speaks.
    #[must_use]
    pub fn into_parts(self) -> (String, Vec<ParsedToolCall>) {
        (self.text, self.calls)
    }
}
