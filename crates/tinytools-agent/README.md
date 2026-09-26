# TinyTools Agent Protocols

`tinytools-agent` owns the model-facing protocol around a tool declaration,
**once**, for every consumer: how a model is told to call a tool, how its
answer is read back — in every surface syntax a model has been seen to use —
how damaged names and arguments are repaired, how results are rendered, and
how a transcript is replayed.

It consumes `tinytools::ToolSpec` and never executes a tool. Permission
checks, approvals, sandboxing, timeouts, provider requests, call-id minting,
and the unknown-tool policy remain the consuming harness's or host's.

## Modules

| Module | Owns |
| --- | --- |
| `parse` | `parse_text(text, &ParseOptions) -> ParseOutcome`: the scan engine, one grammar per surface syntax, code-fence protection |
| `repair` | `json::recover_object` (relaxed / damaged JSON), `name::resolve` (damaged tool names against the offered set), `args` (aliases, envelopes, schema-guided coercion) |
| `stream` | `StreamScrubber`: the same grammars applied to a live text stream, releasing safe text and completed calls as they arrive |
| `render` | the catalogue, the protocol block for each dialect, the `<tool_result>` envelope and transcript replay |
| `codecall` | `parse_calls(body, &registry)`: Python / TypeScript function calls, and `render_code_signature` for the catalogue |
| `dialect` | `ToolDialect` binding one rendering to one parser: `XmlDialect`, `PFormatDialect`, `CodeDialect`, `NativeDialect` |
| `types` | `ParsedToolCall`, `CallSource`, `ParseOptions`, `ParseOutcome`, `ParseDiagnostic` |

## Grammars

| `CallSource` | Shape | Seen from |
| --- | --- | --- |
| `TaggedJson` | `<tool_call>{json}</tool_call>`, `<toolcall>`, `<tool-call>`, bare `<invoke>`, attribute form `<tool_call id="…">`, garbled `<\|tool_call>…<tool_call\|>`, `call:` and `function_call:` prefixes, fenced ```` ```tool_call ````, Kimi `NAME{…}` bodies | Hermes / Qwen templates, OpenRouter, Composio sub-agents, Kimi K2 |
| `InvokeXml` | `<invoke name><parameter name>`, DeepSeek DSML `<｜DSML｜invoke …>`, namespaced `<atem:invoke>`, `<function=NAME><parameter=k>`, `<function name>` | Claude, DeepSeek V3/V4, muse-spark, Llama / Qwen / Gemma |
| `Sentinel` | `<｜tool▁call▁begin｜>…<｜tool▁call▁end｜>`, `<\|tool_call_begin\|>…<\|tool_call_end\|>` | DeepSeek R1 / V3, Kimi K2 |
| `Harmony` | `<\|channel\|>commentary to=NAME<\|message\|>{json}<\|call\|>` | gpt-oss |
| `Mistral` | `[TOOL_CALLS][{…}]`, `[TOOL_CALLS]NAME[ARGS]{…}` | Mistral |
| `Glm` | `tool/param>value` lines | GLM |
| `BareJson` | the whole response is one object / `tool_calls` envelope | Minimax gateways, `llama3.2:3b` under `tool_choice: required` |
| `PFormat` | `name[0\|value]` inside a tag, registry-gated | any prompted model |
| `Code` | `name(arg="value")` / `name({arg: "value"})` inside a tag, registry-gated; literals only, all-or-nothing per body | any code-trained model |

Adding a grammar is one file under `src/parse/grammar/` and one entry in
`GRAMMARS`; batch parsing, streaming, and every dialect pick it up.

## Bounds

Recovery is forgiving because every accommodation was a real capture, and
bounded because the alternative is a phantom call:

- a call needs a marker; only the bare-JSON path runs without one, and it
  requires the entire response to be the value;
- argument keys are aliased, tool names are not; a bare object needs the
  canonical `arguments` key or a name the caller offered;
- names are repaired only to a **unique** offered tool, never invented;
- a fenced code block with a language is an example, not a call;
- an open string in truncated JSON is never closed by guessing;
- this crate never mints call ids.

Diagnostics carry lengths and names, never model output.

The crate has no dependency on TinyAgents or an inference/provider runtime.
The optional `tracing` feature emits diagnostics for parser recovery and
transcript repair decisions.
