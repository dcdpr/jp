# RFD D31: Transitional JP Protocol Bridge for MCP Tools

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-15
- **Requires**: [RFD 028](../028-structured-inquiry-system-for-tool-questions.md)

## Summary

Bridge MCP tools into JP's tool execution semantics by (a) speculatively
unwrapping `jp_tool::Outcome` JSON when an MCP tool emits it as a single text
content, and (b) attaching JP's tool execution context (arguments, answers,
options, root) to MCP requests under `_meta."computer.jp/tool"` and
`_meta."computer.jp/context"`. This is an explicit stopgap until [RFD 058]
lands, intended to give MCP tools access to JP-specific features (tool-driven
inquiries via `Outcome::NeedsInput`, transient-error retry semantics,
per-tool options) months earlier than the typed content-block migration
allows.

## Motivation

`jp_cli` runs MCP tools and local tools through fundamentally different code
paths. The local-tool path parses stdout as `jp_tool::Outcome` and
maps the variants onto JP's `ExecutionOutcome`, so local tools can return
`Success`, `Error { transient, trace }`, or `NeedsInput { question }`. The
MCP-tool path concatenates all `Content` items into a single string, uses
MCP's `is_error` flag as the only success/failure signal, and produces only
`Completed`. There is no path for an MCP tool to declare a transient error,
return a structured question, or read accumulated answers on retry.

JP also has no mechanism to pass execution context to an MCP tool. [RFD 042]
explicitly excluded MCP tools from the per-tool `options` mechanism because
"JP has no way to pass out-of-band options to an external server." That
constraint was correct at the time but is removable: MCP's `_meta` field
([SEP-1319]) exists for exactly this kind of protocol-level signaling, and
`rmcp 1.1` already exposes it via the `RequestParamsMeta` trait.

The capability gap matters now because two in-tree MCP servers (`bookworm`
for crate documentation, `grizzly` for Bear-note search) ship with a `--jp`
flag that already emits `jp_tool::Outcome` JSON envelopes — but `jp_cli`
treats them as opaque text. The advertised "JP tool protocol" only half-works.
Closing the gap unblocks tool-driven inquiries (`Outcome::NeedsInput`),
transient retries, and JP options for MCP tools today, instead of waiting for
[RFD 058]'s broader content-block migration.

## Design

### Scope discipline

This RFD covers two narrow protocol additions and nothing else. It does **not**
introduce typed content blocks, resource URIs, mimeType formatting, or
content-block-shaped questions. Those belong to [RFD 058].

### Response side: speculative `Outcome` unwrap

`execute_mcp` in `crates/jp_llm/src/tool.rs` adds an `Outcome` parsing step
guarded by a content-shape check:

1. After receiving `CallToolResult`, inspect the `content` vector.
2. If `content.len() == 1` and the single item is `RawContent::Text` and
   `serde_json::from_str::<Outcome>(&text)` succeeds, route through `Outcome`
   semantics (the same `CommandResult` → `ExecutionOutcome` mapping used by
   `execute_local`).
3. Otherwise, fall back to the current MCP-native behavior (concatenate
   content, map `is_error`).

The single-text-content guard rules out two failure modes:

- **Lost multi-resource semantics.** A tool returning `n` resource items keeps
  working as a list of resources, even if one of them happens to contain JSON
  that parses as `Outcome`.
- **Mixed content collisions.** A tool returning a text block followed by an
  image block can't accidentally trigger Outcome-unwrapping on the text alone.

When `is_error: true` and the content parses as `Outcome::Success`, the MCP
flag wins — the response is treated as an error and the parsed `Outcome` is
discarded with a `warn!` log. This case shouldn't arise in practice (a tool
emitting `Outcome` semantics shouldn't disagree with itself) but the tiebreak
needs to be defined.

### Request side: unified tool context via `_meta`

The local-tool path builds a JSON context for each tool invocation:

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

This blob is rendered into the local tool's command template as
`tool.arguments`, `tool.answers`, `tool.options`, `context.root`, etc.

The same blob is serialized into the MCP request's `_meta` field, split across
two reverse-DNS-prefixed keys:

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

The two keys are independently readable. A tool that only cares about answers
reads `_meta["computer.jp/tool"].answers`; a tool that only needs the workspace
path reads `_meta["computer.jp/context"].root`. The split matches [RFD 058]'s
established `computer.jp/error` and `computer.jp/status` pattern.

**`tool.arguments` is duplicated** between the standard MCP `arguments` field
and `_meta["computer.jp/tool"].arguments`. The local-tool path makes the same
trade — arguments are both rendered into the command line and visible in the
template context as `tool.arguments`. Same-shape symmetry between transports is
worth the duplicated bytes.

### Shared context builder

A `tool_context(name, arguments, answers, config, root, action) -> Value`
helper extracts the JSON construction currently inlined in `execute_local`.
Both `execute_local` (template context) and `execute_mcp` (`_meta` payload)
call it. This removes the only place the two flows could drift.

### MCP client changes

`jp_mcp::Client::call_tool` gains an optional `meta: Option<JsonObject>`
parameter. When provided, the client calls `RequestParamsMeta::set_meta` on
the `CallToolRequestParams` before dispatch. When `None` (or empty), no
`_meta` is sent.

The execution flow:

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

The `ToolCoordinator` retry loop is unchanged. The local-tool, MCP-tool, and
builtin-tool paths all return the same `ExecutionOutcome` shape, so any
caller that handles `NeedsInput` for one transport handles it for all three.

### Transitional markers

Every user-visible surface that mentions the protocol carries an explicit
transitional notice:

- The `--jp` flag's `--help` output: *"Enable transitional JP tool protocol.
  Will be replaced by typed content blocks per RFD 058."*
- `crates/contrib/bookworm/README.md` and `crates/contrib/grizzly/README.md`:
  same notice, with a link to this RFD.
- A new `docs/architecture/jp-aware-mcp-tools.md` page documenting the
  `_meta."computer.jp/*"` namespace and the `Outcome` envelope shape,
  prefixed with a `> [!IMPORTANT]` block stating the protocol is
  transitional and naming [RFD 058] as the successor.

If we don't mark it loudly, adopters treat it as stable regardless of intent.

## Drawbacks

**Two protocols to maintain for MCP tools.** Until [RFD 058] lands and external
tools migrate, `execute_mcp` carries both the speculative-Outcome path and the
MCP-native path. The branches are small (~50 LOC) but they're real maintenance
surface.

**Hyrum's Law on a transitional protocol.** Even with explicit transitional
markers, external tool authors who adopt the `Outcome` envelope and the
`_meta."computer.jp/*"` keys treat them as a contract. If [RFD 058] slips for
12+ months and several external tools adopt this protocol, migrating becomes a
coordinated ecosystem move rather than a quiet internal one. Shipping this RFD
commits the project to keeping [RFD 058] on the active roadmap; deprioritizing
058 in favor of this stopgap would make the stopgap permanent by inertia.

**Reverses an explicit decision in [RFD 042].** [RFD 042] excluded MCP tools
from the per-tool `options` mechanism with a specific rationale ("JP has no way
to pass out-of-band options to an external server"). This RFD makes the
opposite call. The reversal is deliberate: the constraint that justified 042's
decision (no `_meta` plumbing) is removable, and `rmcp` 1.1's first-class
`_meta` support makes the option-passing path cheap and idiomatic.

**Argument duplication on the wire.** `tool.arguments` appears both in MCP's
top-level `arguments` field and inside `_meta."computer.jp/tool".arguments`.
For tools with large argument payloads (e.g. embedded file contents) this is
wasteful. Accepted because same-shape symmetry between local and MCP transports
matters more than wire size for the cases we care about.

## Alternatives

### Wait for [RFD 058]

The clean alternative is to ship nothing transitional, accept that MCP tools
have second-class access to JP semantics until typed content blocks land, and
push [RFD 058] through faster.

Rejected because the bookworm/grizzly use case is real today and the [RFD 058]
implementation surface (typed content blocks + inquiry rework + resource
URIs + mimeType formatting + stateful tool status) is large enough that even
with focus it's a multi-month effort. Boyd's Law applies: a transitional
solution shipped this week is more valuable than the perfect solution in six
months, provided the transition path is bounded.

### Speculative `Outcome` parse without the content-shape guard

Drop the "exactly one `Content::Text`" guard from the response side. Try to
parse `Outcome` from any MCP response, including concatenated multi-content
responses.

Rejected because it conflicts with MCP-native multi-resource tool responses.
A tool emitting `n` `Content::Resource` items legitimately wants those treated
as separate resources, not collapsed and re-interpreted as a JSON envelope.
The guard preserves MCP-native semantics for tools that use them.

### Per-tool opt-in via JP-side config

User declares `outcome_protocol = true` for specific MCP tools in `.jp/config`.
The speculative parse only fires for opted-in tools.

Rejected because configuration burden falls on the user installing the tool
rather than the tool author. Tool authors who want `NeedsInput` semantics
can't unilaterally enable them; they need every user to flip a flag. The
speculative-with-guard approach gives tool authors the affordance directly.

### Use `_meta` exclusively (skip `Outcome` envelope unwrap)

Send request-side metadata via `_meta` but rely entirely on MCP-native
response semantics. MCP tools that want `NeedsInput` would have to define a
custom MCP extension for it.

Rejected because there's no MCP-native equivalent of `Outcome::NeedsInput`.
MCP's elicitation mechanism is server-initiated and out-of-band; it doesn't
fit JP's "tool returns a question alongside its response" model. Skipping the
Outcome unwrap would leave the most valuable JP feature (tool-driven
inquiries) inaccessible to MCP tools.

### Reverse-DNS namespace: alternative shapes

- `_meta.jp` (no reverse-DNS prefix): shorter but inconsistent with [RFD 058]
  and not aligned with MCP community conventions for vendor metadata.
- `_meta["computer.jp/tool-context"]` (single nested key): one key holds the
  full context blob. Slightly fewer bytes but mixes two semantically distinct
  concepts (tool identity/inputs vs execution environment) under one key,
  making partial reads awkward.

Rejected in favor of two keys (`computer.jp/tool` and `computer.jp/context`)
that mirror the local-tool template context shape and match [RFD 058]'s
existing pattern.

## Non-Goals

- **Typed content blocks for tool responses.** Out of scope; that's [RFD 058].
  This RFD intentionally keeps `Outcome` as the response envelope.
- **Resource URIs, mimeType formatting, resource deduplication.** Out of scope;
  see [RFD 058], [RFD 065], [RFD 066], [RFD 067].
- **Stateful tool protocol status.** Out of scope; see [RFD 009] and the
  `computer.jp/status` field defined in [RFD 058].
- **Restructuring the three-way dispatch in `ToolDefinition::execute`.** Out
  of scope; see [RFD D10]. This RFD modifies `execute_mcp` in place; if
  [RFD D10] lands first the same logic moves into `McpRuntime::execute`.
- **Promoting the transitional protocol to a permanent JP feature.** This
  RFD is explicitly transitional. If [RFD 058] is later rejected and the
  project decides to keep `Outcome` as the long-term protocol, that's a
  separate decision recorded in a separate RFD.

## Risks and Open Questions

### `is_error` / `Outcome` disagreement

When an MCP response has `is_error: true` and its single text content parses
as `Outcome::Success { content }`, the design above lets the MCP flag win.
Alternative tiebreaks (Outcome wins, fail loudly with an error) are defensible.
The current choice errs on the side of preserving MCP semantics, since
`is_error` is the older, more widely-implemented signal.

### Migration cost when [RFD 058] lands

The response side will need to migrate: tools emitting `Outcome` envelopes
rewrite their response builders to emit content blocks. The request side
mostly survives — `_meta."computer.jp/*"` is already aligned with [RFD 058]'s
namespace conventions. The hope is that the request-side plumbing in this
RFD is a forward-compatible foundation, but [RFD 058] may yet decide on a
different blob shape (e.g. nested differently, or split across more keys).
This RFD does not bind [RFD 058]'s design.

### External tool adoption

If only bookworm and grizzly (in-tree) adopt the transitional protocol,
migration when [RFD 058] lands is internal and cheap. If external tool
authors adopt it widely, migration becomes a coordinated ecosystem change.
The transitional markers (loud `--jp` `--help`, README notices, architecture
doc disclaimer) are the primary mitigation, but they're not enforceable.
Whether this is a problem depends on how aggressively external authors adopt
the protocol before [RFD 058] is ready.

### Interaction with [RFD D10]

[RFD D10] proposes extracting the three execute paths into a `ToolRuntime`
trait. This RFD modifies `execute_mcp` directly. Sequencing options:

- This RFD lands first; [RFD D10] moves the logic into `McpRuntime::execute`.
- [RFD D10] lands first; this RFD adds the logic to the new
  `McpRuntime::execute`.
- Both land in parallel; whoever merges second pays a small merge cost.

None of these is harmful; they just need coordination in the implementation
plan if both are active.

## Implementation Plan

### Phase 1: shared tool context builder

Extract a `tool_context(name, arguments, answers, config, root, action) ->
Value` function in `jp_llm::tool`. Replace the inline `json!({...})` in
`execute_local` with a call to it. No behavior change.

**Depends on:** nothing. **Mergeable:** yes.

### Phase 2: `_meta` support in `jp_mcp::Client::call_tool`

Add an optional `meta: Option<JsonObject>` parameter (or a builder method) to
`call_tool`. When set, attach via `RequestParamsMeta::set_meta` before
dispatch. Unit-test that the `_meta` keys round-trip correctly.

**Depends on:** nothing. **Mergeable:** yes.

### Phase 3: request-side metadata in `execute_mcp`

`ToolDefinition::execute` already receives `answers`, `config`, and `root`;
extend `execute_mcp`'s parameter list to receive the same. Serialize the
tool context (Phase 1) into `_meta["computer.jp/tool"]` and
`_meta["computer.jp/context"]` and pass it to `call_tool` (Phase 2). Skip
the metadata entirely when `answers`, `options`, and config-derived fields
are all empty/default — keep first-call payloads clean.

**Depends on:** Phases 1 and 2. **Mergeable:** yes.

### Phase 4: response-side `Outcome` unwrap in `execute_mcp`

Add the speculative-parse-with-guard logic. Route `Outcome::NeedsInput`,
`Outcome::Error { transient }`, etc. through the existing `CommandResult`
mapping. Add unit tests covering: single-text-content with valid Outcome,
single-text-content with invalid JSON, multi-content response, `is_error`
+ Outcome::Success collision.

**Depends on:** Phase 3 (request side must work for retries to function).
**Mergeable:** yes.

### Phase 5: transitional documentation

Update `--jp` `--help` text in `bookworm` and `grizzly`. Update both READMEs
with the transitional notice. Add `docs/architecture/jp-aware-mcp-tools.md`
specifying the `Outcome` envelope shape and `_meta."computer.jp/*"`
namespace, with a prominent `> [!IMPORTANT]` block declaring the protocol
transitional and naming [RFD 058] as the successor.

**Depends on:** Phases 3 and 4 (document the actual shipped behavior).
**Mergeable:** yes.

### Phase 6: bookworm/grizzly dogfooding

Migrate at least one bookworm tool to emit `Outcome::NeedsInput` and verify
the round-trip works end-to-end (tool returns NeedsInput → JP CLI prompts
or runs structured-output inquiry → answer flows back via `_meta` → tool
proceeds). This is the dogfooding check that proves the protocol is wired
correctly.

**Depends on:** Phases 3–5. **Mergeable:** yes.

## References

- [RFD 028]: Structured Inquiry System for Tool Questions (Implemented) —
  established the `NeedsInput` + answers-accumulation loop that this RFD
  extends to MCP tools.
- [RFD 042]: Tool Options (Implemented) — explicitly excluded MCP tools
  from per-tool options; this RFD reverses that decision.
- [RFD 058]: Typed Content Blocks for Tool Responses (Discussion) — the
  long-term replacement. This RFD's scope is bounded by the intent to be
  superseded by [RFD 058].
- [RFD 065]: Typed Resource Model for Attachments (Discussion) — depends on
  [RFD 058]; out of scope here.
- [RFD 066]: Content-Addressable Blob Store (Discussion) — depends on
  [RFD 058]; out of scope here.
- [RFD 067]: Resource Deduplication for Token Efficiency (Discussion) —
  depends on [RFD 058]; out of scope here.
- [RFD 009]: Stateful Tool Protocol (Accepted) — the stateful tool lifecycle
  is layered above the single-execution model this RFD touches.
- [RFD D10]: Unified Tool Execution Model (Draft) — structural refactor at
  the dispatch layer; coordination noted in Risks.
- [SEP-1319]: MCP request-params `_meta` field — the protocol surface this
  RFD attaches metadata to.

[RFD 009]: 009-stateful-tool-protocol.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 042]: 042-tool-options.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD D10]: D10-unified-tool-execution-model.md
[SEP-1319]: https://github.com/modelcontextprotocol/modelcontextprotocol/pull/1319
