# RFD 111: Transitional JP Protocol Bridge for MCP Tools

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-15
- **Summary**: Bridges MCP tools to `jp_tool::Outcome` and passes JP's execution
  context via `_meta` until typed content blocks land.

## Summary

This RFD bridges MCP tools into JP's tool execution semantics by speculatively
unwrapping `jp_tool::Outcome` JSON when an MCP tool returns a single text
content block, and by attaching JP's tool execution context (`arguments`,
`answers`, `options`, and `root`) to MCP requests under
`_meta."computer.jp/tool"` and `_meta."computer.jp/context"`.
This bridge is an explicit stopgap until [RFD 058] lands, giving MCP tools
access to tool-driven inquiries (`Outcome::NeedsInput`), transient-error
retries, and per-tool options without waiting for the typed content-block
migration.

## Motivation

`jp_cli` runs local tools and MCP tools through separate code paths.
The local-tool path parses stdout as `jp_tool::Outcome` and maps its variants
onto `ExecutionOutcome`, allowing local tools to return `Success`, `Error {
transient, trace }`, or `NeedsInput { question }`.
The MCP-tool path concatenates all `Content` items into a single string, relies
on MCP's `is_error` flag as the sole success or failure signal, and always
produces `ExecutionOutcome::Completed`.
As a result, an MCP tool cannot declare a transient error or pause execution
with a structured question.

JP also has no way to pass execution context, such as accumulated inquiry
answers or per-tool `options`, to an MCP server.
[RFD 042] excluded MCP tools from per-tool `options` because JP had "no way to
pass out-of-band options to an external server."
MCP's `_meta` field ([SEP-1319]) provides a standard channel for this
protocol-level metadata, and `rmcp 1.1` exposes it through the
`RequestParamsMeta` trait.

This gap affects existing tools today.
Two in-tree MCP servers, `bookworm` (crate documentation) and `grizzly` (Bear
note search), already ship with a `--jp` flag that emits `jp_tool::Outcome` JSON
envelopes, but `jp_cli` treats those envelopes as opaque text.
Supporting `Outcome` decoding and `_meta` context on the MCP path makes `--jp`
work end to end without waiting for [RFD 058].

## Design

This design adds two narrow protocol bridges to the MCP tool path: speculative
`Outcome` unwrapping on responses and unified tool context under `_meta` on
requests.
Typed content blocks, resource URIs, `mimeType` formatting, and
content-block-shaped questions remain in [RFD 058].

### Response side: speculative `Outcome` unwrap

`execute_mcp` in `crates/jp_llm/src/tool.rs` adds an `Outcome` parsing step
guarded by a content-shape check:

1. After receiving `CallToolResult`, inspect the `content` vector.
2. If `content.len() == 1`, the sole item is `RawContent::Text`, and
   `serde_json::from_str::<Outcome>(&text)` succeeds, map the parsed `Outcome`
   to `ExecutionOutcome` through the same `CommandResult` conversion used by
   `execute_local`.
3. Otherwise, fall back to standard MCP handling: concatenate the content items
   and map `is_error`.

Requiring a single text block prevents two failure modes:

- **Lost multi-resource semantics.** A tool returning `n` resource items is
  still handled as a list of resources, even if one item contains JSON that
  matches the `Outcome` schema.
- **Mixed-content collisions.** A tool returning a text block alongside an image
  or resource block never triggers `Outcome` unwrapping on the text block alone.

If `is_error` is `true` while the text parses as `Outcome::Success`, the MCP
`is_error` flag takes precedence: `execute_mcp` logs a `warn!` diagnostic,
discards the parsed `Outcome`, and treats the response as an error.

### Request side: unified tool context via `_meta`

For each local tool invocation, `execute_local` builds a JSON context object and
exposes its fields (`tool.arguments`, `tool.answers`, `tool.options`,
`context.root`) to the command template:

```json
{
  "tool": {
    "name": "search_crate_type_definitions",
    "arguments": { "crate_name": "serde_json", "query": "Value::pointer" },
    "answers": { "confirm-fetch": true },
    "options": { ... }
  },
  "context": {
    "action": "Run",
    "root": "/path/to/workspace"
  }
}
```

A new `tool_context(name, arguments, answers, config, root, action) -> Value`
helper extracts this JSON construction so both `execute_local` and `execute_mcp`
build the same structure from a single source.
On the MCP path, `execute_mcp` serializes the two top-level objects into the
request's `_meta` field under reverse-DNS keys:

```json
{
  "_meta": {
    "computer.jp/tool": {
      "name": "search_crate_type_definitions",
      "arguments": { "crate_name": "serde_json", "query": "Value::pointer" },
      "answers": { "confirm-fetch": true },
      "options": { ... }
    },
    "computer.jp/context": {
      "action": "Run",
      "root": "/path/to/workspace"
    }
  }
}
```

Splitting the payload across two keys lets a server read only the half it needs
(`_meta["computer.jp/tool"].answers` for inquiry answers or
`_meta["computer.jp/context"].root` for the workspace path) and matches the
`computer.jp/error` and `computer.jp/status` namespace convention in [RFD 058].

`tool.arguments` appears in both the top-level MCP `arguments` field and
`_meta["computer.jp/tool"].arguments`.
The local-tool path makes the same trade-off by passing arguments on the command
line while also exposing `tool.arguments` in the template context; keeping the
context shape identical across transports is worth the extra bytes on the wire.

### MCP client and coordinator flow

`jp_mcp::Client::call_tool` gains a `meta: Option<JsonObject>` parameter.
When `meta` is non-empty, `call_tool` attaches it to `CallToolRequestParams` via
`RequestParamsMeta::set_meta` before dispatching the request; when `None` or
empty, no `_meta` field is sent.

The end-to-end inquiry flow across `ToolCoordinator`, `execute_mcp`, and the MCP
server is:

```text
ToolCoordinator                        execute_mcp                       MCP server
─────────────────                      ───────────                       ──────────
[Running]
  execute(id, args, answers,    ─►  build tool_context(...)
          config, root, ...)         build CallToolRequestParams
                                     set_meta(_meta.computer.jp.*)  ─►   read meta
                                                                          ...decide
                                     parse CallToolResult           ◄─   respond
                                       single-Text-Outcome path?
                                       else: MCP-native path
  ◄─ ExecutionOutcome::NeedsInput
[AwaitingInput]
  collect answer
  answers[question.id] = ...
  execute(id, args, answers',   ─►  same flow, augmented answers
          config, root, ...)
```

The `ToolCoordinator` retry loop does not change.
Because the local-tool, MCP-tool, and builtin-tool paths all return
`ExecutionOutcome`, any caller that handles `NeedsInput` for one transport
handles it for all three.

### Transitional notices

Every user-facing surface that exposes this bridge marks it as transitional so
tool authors do not mistake it for a permanent contract:

- **CLI help text:** the `--jp` flag's `--help` output in `bookworm` and
  `grizzly` states: *"Enable transitional JP tool protocol. Will be replaced by
  typed content blocks per RFD 058."*
- **Server READMEs:** `crates/contrib/bookworm/README.md` and
  `crates/contrib/grizzly/README.md` carry the same notice with a link to this
  RFD.
- **Architecture documentation:** a new
  `docs/architecture/jp-aware-mcp-tools.md` page documents the
  `_meta."computer.jp/*"` namespace and the `Outcome` envelope shape, opening
  with a `> [!IMPORTANT]` callout that marks the protocol as transitional and
  names [RFD 058] as its replacement.

## Drawbacks

- **Two response paths in `execute_mcp`.** Until [RFD 058] lands and tools
  migrate, `execute_mcp` must maintain both speculative `Outcome` unwrapping and
  standard MCP response handling (roughly 50 lines of branching).
- **Hyrum's Law on a stopgap protocol.** Even with explicit notices, external
  tool authors who adopt the `Outcome` envelope and `_meta."computer.jp/*"` keys
  will depend on them.
  Shipping this bridge commits JP to keeping [RFD 058] on the active roadmap so
  the stopgap does not become permanent by inertia.
- **Reversal of [RFD 042].** [RFD 042] excluded MCP tools from per-tool
  `options` because JP lacked out-of-band request metadata plumbing.
  Using `_meta` reverses that decision now that `rmcp 1.1` supports request
  metadata directly.
- **Argument duplication on the wire.** Sending `tool.arguments` in both MCP's
  top-level `arguments` field and `_meta."computer.jp/tool".arguments`
  duplicates large payloads such as embedded file contents.
  Keeping context structure identical across local and MCP transports outweighs
  the extra wire size.

## Alternatives

### Wait for [RFD 058]

JP could skip a transitional bridge, leave MCP tools without access to `Outcome`
semantics or execution context until typed content blocks land, and focus effort
on finishing [RFD 058].

This approach leaves `bookworm` and `grizzly` incomplete for too long.
[RFD 058] spans typed content blocks, inquiry rework, resource URIs, `mimeType`
formatting, and stateful tool status, making it a multi-month effort even with
dedicated focus.
A bounded stopgap delivers inquiry and retry support immediately while [RFD 058]
is in progress.

### Parse `Outcome` without the content-shape guard

`execute_mcp` could drop the single-`RawContent::Text` guard and attempt to
parse `Outcome` from any MCP response, including concatenated multi-content
responses.

This breaks MCP-native multi-resource responses.
A tool emitting `n` `Content::Resource` items expects JP to treat them as
separate resources rather than collapsing and re-interpreting them as a JSON
envelope.
The single-text-block guard preserves standard MCP semantics for multi-block
responses.

### Opt in per tool via JP configuration

Users could enable `Outcome` parsing per tool by setting `outcome_protocol =
true` in `.jp/config`.

This shifts configuration burden onto every user who installs a JP-aware MCP
tool.
Tool authors could not rely on `NeedsInput` semantics working out of the box
unless each user enabled the flag.
Speculative parsing guarded by content shape lets tool authors opt in directly
from the server response.

### Use `_meta` without `Outcome` unwrapping

JP could send request metadata in `_meta` while relying exclusively on standard
MCP response semantics, requiring tools that need `NeedsInput` to use a custom
MCP response extension.

MCP has no response-side equivalent of `Outcome::NeedsInput`.
MCP elicitation is server-initiated and out-of-band, which does not fit JP's
model where a tool call returns a structured question as its result and is
re-invoked with the accumulated answers.
Without `Outcome` unwrapping, MCP tools still could not trigger JP inquiries.

### Alternative `_meta` key layouts

Two other key layouts were considered:

- **`_meta.jp` (no reverse-DNS prefix):** shorter, but inconsistent with [RFD
  058] and with MCP conventions for vendor-scoped metadata.
- **`_meta["computer.jp/tool-context"]` (single nested key):** saves a few bytes
  by placing the entire context object under one key, but combines tool inputs
  and execution environment under a single namespace and makes partial reads
  more awkward.

Using two keys (`computer.jp/tool` and `computer.jp/context`) mirrors the
top-level keys of the local-tool template context and matches [RFD 058]'s
namespace pattern.

## Non-Goals

- **Typed content blocks for tool responses.** Covered by [RFD 058]; this RFD
  keeps `Outcome` as the response envelope.
- **Resource URIs, `mimeType` formatting, and resource deduplication.** Covered
  by [RFD 058], [RFD 065], [RFD 066], and [RFD 067].
- **Stateful tool protocol status.** Covered by [RFD 009] and the
  `computer.jp/status` field in [RFD 058].
- **Restructuring the three-way dispatch in `ToolDefinition::execute`.** This
  RFD modifies `execute_mcp` in place.
- **Making the bridge a permanent protocol.** Keeping `Outcome` as a long-term
  MCP protocol if [RFD 058] were ever rejected would require a separate RFD.

## Risks and Open Questions

### `is_error` and `Outcome` disagreement

When an MCP response sets `is_error: true` while its single text block parses as
`Outcome::Success { content }`, `execute_mcp` lets `is_error` take precedence.
Other tiebreakers, such as trusting `Outcome` or failing the call with a
protocol error, are also defensible, but honoring `is_error` preserves standard
MCP semantics when a server or proxy sets the error flag outside the `Outcome`
payload.

### Migration cost when [RFD 058] lands

When [RFD 058] lands, tools that emit `Outcome` envelopes will need to update
their response builders to emit typed content blocks.
The request side should require little or no change because
`_meta."computer.jp/*"` already follows [RFD 058]'s namespace convention, though
[RFD 058] remains free to revise the exact payload layout.

### External tool adoption

If only the in-tree `bookworm` and `grizzly` servers adopt this bridge,
migrating to [RFD 058] is a self-contained internal change.
If third-party MCP servers adopt it widely before [RFD 058] ships, retiring the
`Outcome` unwrap will require coordinating with external authors.
The transitional notices in `--jp` `--help`, the server READMEs, and
`docs/architecture/jp-aware-mcp-tools.md` set expectations early, though they
cannot prevent external adoption.

## Implementation Plan

### Phase 1: shared tool context builder

Extract `tool_context(name, arguments, answers, config, root, action) -> Value`
in `jp_llm::tool` and replace the inline `json!({...})` construction in
`execute_local` with a call to it, without changing behavior.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 2: `_meta` support in `jp_mcp::Client::call_tool`

Add an optional `meta: Option<JsonObject>` parameter (or builder method) to
`call_tool` that attaches metadata via `RequestParamsMeta::set_meta` before
dispatch.
Add unit tests verifying that `_meta` keys round-trip cleanly.

**Depends on:** nothing.
**Mergeable:** yes.

### Phase 3: request-side metadata in `execute_mcp`

Pass `answers`, `config`, and `root` from `ToolDefinition::execute` into
`execute_mcp`.
Serialize the output of `tool_context` (Phase 1) into
`_meta["computer.jp/tool"]` and `_meta["computer.jp/context"]` and pass it to
`call_tool` (Phase 2).
Omit `_meta` when `answers`, `options`, and config-derived fields are all empty
or at their defaults so initial call payloads stay minimal.

**Depends on:** Phases 1 and 2.
**Mergeable:** yes.

### Phase 4: response-side `Outcome` unwrap in `execute_mcp`

Add the single-text-block guard and speculative `Outcome` parser to
`execute_mcp`, routing `Outcome::NeedsInput`, `Outcome::Error { transient }`,
and `Outcome::Success` through the existing `CommandResult` mapping.
Add unit tests covering a single text block with a valid `Outcome`, a single
text block with non-`Outcome` text or invalid JSON, a multi-content response,
and a collision between `is_error: true` and `Outcome::Success`.

**Depends on:** Phase 3 (request-side metadata is required for inquiry retries).
**Mergeable:** yes.

### Phase 5: transitional documentation

Update the `--jp` `--help` text and README notices in `bookworm` and `grizzly`.
Add `docs/architecture/jp-aware-mcp-tools.md` specifying the `Outcome` envelope
shape and the `_meta."computer.jp/*"` namespace, headed by a `> [!IMPORTANT]`
callout marking the protocol as transitional and naming [RFD 058] as its
replacement.

**Depends on:** Phases 3 and 4.
**Mergeable:** yes.

### Phase 6: `bookworm` and `grizzly` dogfooding

Migrate at least one `bookworm` tool to emit `Outcome::NeedsInput` and verify
the full inquiry round-trip end to end: the tool returns `NeedsInput`, `jp_cli`
prompts the user or runs a structured-output inquiry, the answer flows back via
`_meta`, and the re-invoked tool completes.

**Depends on:** Phases 3 through 5.
**Mergeable:** yes.

## References

- [RFD 028]: Structured Inquiry System for Tool Questions (Implemented).
  Establishes the `NeedsInput` and answer-accumulation loop that this RFD
  extends to MCP tools.
- [RFD 042]: Tool Options (Implemented).
  Excluded MCP tools from per-tool `options`, a decision this RFD reverses.
- [RFD 058]: Typed Content Blocks for Tool Responses (Discussion).
  The long-term replacement designed to supersede this bridge.
- [RFD 065]: Typed Resource Model for Attachments (Discussion).
  Builds on [RFD 058]; out of scope here.
- [RFD 066]: Content-Addressable Blob Store (Discussion).
  Builds on [RFD 058]; out of scope here.
- [RFD 067]: Resource Deduplication for Token Efficiency (Discussion).
  Builds on [RFD 058]; out of scope here.
- [RFD 009]: Stateful Tool Protocol (Accepted).
  Layers the stateful tool lifecycle above the single-execution model modified
  here.
- [SEP-1319]: MCP request-params `_meta` field.
  Defines the protocol surface used to carry JP request metadata.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 042]: 042-tool-options.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[SEP-1319]: https://github.com/modelcontextprotocol/modelcontextprotocol/pull/1319
