# RFD D31: Provider Response Artifacts

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Required by**: [RFD D71]
- **Summary**: Adds hosted-activity events, provider replay envelopes, and typed
  citations to `ConversationStream` for provider-executed tool output.

## Summary

This RFD extends JP's response and stream models to represent provider-executed
activity, exact provider replay payloads, and typed citations on assistant
messages.
Provider adapters parse usage counters for hosted activity and surface them as
diagnostics without persisting them.
These types provide the response model required before exposing provider-hosted
tools ([RFD D71]).

## Motivation

### Adapters discard or fail to parse hosted-tool artifacts

The OpenAI adapter drops three output-item kinds (`FileSearch`,
`WebSearchResults`, and `ComputerToolCall`) in three places
(`crates/jp_llm/src/provider/openai.rs:282`, `:1445`, `:1482`), along with the
`WebSearchCallInitiated`, `Searching`, and `Completed` stream events.
The comment at `:1482` notes that the variants are listed explicitly rather than
matched with a wildcard "so a new variant in the upstream event enum fails to
compile until someone decides whether JP needs it."
This RFD makes that decision.

The Anthropic adapter cannot parse hosted-tool artifacts at all.
`async_anthropic::types::MessageContent` is an internally tagged enum over six
variants with no catch-all, so a `server_tool_use` content block fails
deserialization and aborts the stream.

### The response model cannot carry attribution

`ChatResponse::Message { message: String }` is a bare `String` and cannot hold
structured attribution.
Anthropic, OpenAI, and Google all impose attribution or display obligations on
search-backed output, and Anthropic's citation payload carries a URL, title,
cited text, and an `encrypted_index` that must round-trip.
Without structured attribution on `ChatResponse`, JP cannot render search-backed
text with the required citations.

### Stabilizing `elelem` before modeling hosted output causes churn

[RFD D45] proposes publishing the normalized streaming event model as the public
API of a standalone `elelem` crate, and cites hosted tools as a reason to prefer
OpenAI's Responses API.
Freezing that event model before it can represent hosted activity and
attribution would force a breaking change once provider-hosted tools are added.

Without a stream representation for hosted-tool output and citations,
provider-hosted tools cannot be exposed, the OpenAI adapter continues dropping
these artifacts, and each new provider adapter adds another discard site.

## Design

### Hosted activity is not a Tool Call

In JP, a **Tool Call** is an operation whose request JP itself must fulfill: JP
resolves execution policy, dispatches a runtime, records a `ToolCallResponse`,
and returns the result to the provider.
Every consumer of tool-call events relies on this contract: `turn_loop.rs`
prepares each committed `ToolCallRequest`,
`ConversationStream::sanitize_orphaned_tool_calls` injects a synthetic response
for any request lacking one, and the restart path scans for unresponded
requests.

Provider-hosted activity does not follow this contract.
The provider executes the tool inside its own agent loop and continues
generating while JP only observes the resulting events.
Recording hosted activity as a `ToolCallRequest` would falsely assert in the
durable stream that JP dispatched the work and would force every
`ToolCallRequest` consumer to add an exception.
Hosted activity therefore uses a dedicated event kind.

### `EventKind::HostedActivity`

```text
HostedActivity
    id          provider-assigned activity id (correlation key)
    name        JP-facing capability name (e.g. "web_search")
    state       started | completed | failed
    input       normalized invocation input (e.g. the search query)
    summary     short human-readable description for display
    payload     ProviderPayload (see below), optional
```

State changes are recorded as **append-only transitions** linked by `id` rather
than mutating an event in place.
This matches the append-only stream principle ([RFD 064]) that provider-issued
repairs already follow, where `jp_llm::event::record_patches` appends an overlay
rather than rewriting target events.
An activity with a `started` transition and no terminal (`completed` or
`failed`) transition has a defined meaning: the activity began and never
finished, which a stream reader can detect and a provider adapter can handle on
continuation.

Because `sanitize_orphaned_tool_calls` matches on `EventKind::ToolCallResponse`
and `as_tool_call_request()`, `HostedActivity` is ignored during orphan repair
without any special-case check.

### `ProviderPayload`

`ProviderPayload` wraps provider-native replay data in a typed envelope rather
than scattering loose keys across event metadata:

```text
ProviderPayload
    provider    ProviderId of the originating provider
    format      provider-defined payload discriminator
    version     format version selected when the activity started
    value       the native payload
```

The envelope enforces six rules:

- **Interpreted only by the originating provider.** Every other adapter
  preserves the payload in the stream and ignores it.
- **Immutable.** No consumer rewrites a stored payload.
  Because compaction is non-destructive ([RFD 064]) and applies overlays rather
  than mutating events, payloads remain unchanged by construction.
- **Unknown versions round-trip unmodified.** Older JP versions preserve and
  forward payloads written with newer format versions rather than rejecting
  them.
- **`version` records the wire contract selected at start.** Anthropic's web
  search tool has three versions with different fields and defaults; recording
  `version` prevents a JP upgrade from continuing an in-progress activity under
  a different wire contract.
- **Untrusted content.** Because `value` may contain arbitrary web content, JP
  never renders or logs it directly (see [RFD 096]).
- **A present-but-undecodable payload is an error**, not a silent downgrade.
  If a payload for the active provider fails to decode, the adapter returns an
  error.
  Handling valid payloads whose *continuation requirements* cannot be met
  belongs to the consuming RFD ([RFD D71]).

`value` is stored inline in the event stream.
Because Anthropic's search payloads are large, the envelope can later hold a
blob reference once [RFD 066] is implemented without changing the surrounding
event model.

### Citations on assistant content

Citations annotate assistant text, so they attach to the assistant content event
(`ChatResponse::Message`) rather than to `HostedActivity`.
Each `Attribution` separates the source metadata, the quoted excerpt, the text
span within the assistant message, and the provider replay payload:

```text
Attribution
    source      URL and title of the cited source
    excerpt     text quoted from the source, optional
    range       span within the event's stored text, optional
    replay      ProviderPayload carrying the provider's opaque replay value
```

The content event itself is the answer-side anchor.
Anthropic emits each cited passage as its own text block (and therefore its own
event), so the block is the anchor and `range` is absent.
OpenAI's `url_citation` carries offsets into the response text, which populate
`range`.
`excerpt` always holds text quoted from the source and never substitutes for
`range`.

`range` indexes into the event's final stored text.
An adapter that modifies text after citations arrive (such as Anthropic's
max-token continuation trimming overlapping text) must remap or drop affected
ranges so stale offsets are never persisted.

#### Streaming lifecycle

Citations stream inside a text block: Anthropic sends one `citations_delta` per
citation before `content_block_stop`.
As each citation arrives, the adapter forwards it with its block index,
`EventBuilder` appends it to that block's attribution list, and `Flush`
finalizes the event.

Citations are also preserved when a stream is interrupted or retried.
`EventBuilder::peek_partial_events` commits unflushed message text as its own
event and attaches the citations received so far, marked incomplete, so a reader
can distinguish an uncited block from a block truncated before its attribution
finished streaming.
Partial commits drop all other accumulated metadata, including thinking
signatures, because partial reasoning is resent unsigned.

### Usage

Anthropic reports hosted-activity counters
(`usage.server_tool_use.web_search_requests`), which are billed separately from
tokens.

JP does not persist token usage: no HTTP provider adapter reads its usage
counters, and the ACP adapter keeps usage out of event metadata and logs it as a
`debug` diagnostic (`anthropic/acp/usage.rs`).
Hosted-activity counters follow the same path: the adapter parses and logs them
without writing them to the conversation stream.

Consumers can still approximate billed requests by counting `HostedActivity`
events in the durable stream, though whether providers bill for failed searches
is unverified.
Persisting usage counters requires provider-response identity; see Non-Goals.

### Two projections

A conversation carrying hosted activity supports two projections:

- **Portable**: the activity name, state, input, and summary, together with the
  assistant text and citations.
  This projection survives export, search, rendering, and provider switches.
- **Provider-native**: the `ProviderPayload`, which only the originating
  provider reads for exact multi-turn replay.

## Drawbacks

**Adding an `EventKind` variant updates every exhaustive match.** At least seven
call sites match exhaustively on `EventKind`:
`jp_attachment_internal/src/lib.rs`, `jp_cli/src/cmd/query/tool/inquiry.rs`,
`jp_cli/src/editor.rs`, `jp_cli/src/render/turn.rs`, two in
`jp_cli/src/shared/search.rs`, and `jp_conversation/src/storage.rs`.
Provider `convert_event` implementations and the `serve-web` plugin's renderer
also require new match arms.
This is a one-time cost of keeping `HostedActivity` distinct from Tool Call.

**Adding citations changes the shape of `ChatResponse::Message`.** Every
consumer that pattern-matches on message content must handle the attribution
field, even when it does not use citations.

**The portable projection is lossy across provider switches.** Switching
providers mid-conversation preserves the final assistant text, citations, and
normalized activity, but drops the provider-native replay state (see
Alternatives).

**No user-visible behavior ships in this RFD.** These types cannot be validated
through user-facing behavior until a provider adapter emits them in [RFD D71].

## Alternatives

**Record hosted activity as a `ToolCallRequest` / `ToolCallResponse` pair with a
marker.** This approach requires JP to fabricate a response for work it never
dispatched and requires coordinated exceptions across the turn loop, the
execution plan, and orphan sanitization.
Earlier iterations took this approach, and each required another special case.

**Produce a portable textual digest of native results so they survive a provider
switch.** Anthropic's `encrypted_content` is opaque to other providers, so any
digest would be fabricated rather than translated: it cannot reproduce the
original context and risks surfacing content the provider deliberately kept
opaque.
Preserving the final assistant text and normalized activity is sufficient.

**Store the native payload as loose metadata keys.** Thinking signatures use a
flat metadata key because a signature is a single opaque string.
A replay payload must carry provider identity, format, and version as a single
validated unit, which requires the structured `ProviderPayload` envelope.

**Extend `ChatResponse` with a hosted-activity variant instead of adding an
`EventKind`.** Hosted activity is provider-executed tool state, not
assistant-authored content, and placing it on `ChatResponse` would force every
consumer of assistant text to handle a variant with no text.

## Non-Goals

**Provider-response identity and durable usage.** A single `ChatRequest` can
produce multiple provider responses across max-token chaining, retries,
paused-turn continuations, and tool cycles, and JP does not record which
response produced which events.
Because usage is a property of a provider response, attaching counters to the
last event in a sequence would misattribute them.
Both provider-response identity and durable usage are deferred because the first
consumer ([RFD D71]) needs neither.
[RFD 097] is Implemented and supplies the stable-identifier primitive a
provider-response record would build on, so a later RFD can add both without a
dependency gate.

**Rendering.** This RFD defines the stream data structures only.
Terminal and web rendering, including citation-display obligations, belong to
the consuming RFD ([RFD D71]), where they gate a user-visible feature.

**Provider-specific hosted tools.** This RFD adds no configuration surface,
request translation, or provider capability list.

**Portable native state.** See Alternatives.

## Risks and Open Questions

- **Sequencing with [RFD D45].** [RFD D45] should not publish `elelem::Event` as
  a public API before accounting for hosted activity and attribution; otherwise
  the new crate will either break immediately or route hosted artifacts through
  generic metadata, recreating the problem this RFD solves.
- **Extending `async-anthropic` deserialization.** The fork's `MessageContent`
  cannot parse `server_tool_use`, `ContentBlockDelta` has no `citations_delta`,
  `Text` has no `citations`, and `Usage` has no `server_tool_use` counters.
  Whether serde can express a catch-all arm on the internally tagged
  `MessageContent` enum also requires verification: `#[serde(other)]` covers
  unit variants only, so an untagged `Unknown(Map)` variant is the likely shape,
  mirroring `types::Tool`.
- **Generalizing the activity lifecycle beyond two providers.** Anthropic pairs
  a call block with a result block, and OpenAI emits a call item plus an
  annotated message, whereas Google may expose only completed grounding metadata
  with no call/result pair.
  The `started | completed | failed` state model may need adjustment when
  validated against Google in the consuming RFD.
- **Citation shapes across providers.** Anthropic's `web_search_result_location`
  is the only citation payload modeled in detail here; other providers' citation
  structures remain to be verified.
- **Coordinate unit for `range`.** OpenAI's `url_citation` offsets are the only
  source of ranges, and whether they count bytes or code points is unverified.
  The coordinate unit stored in `range` will be fixed once that is verified.

## Implementation Plan

### Phase 1: `async-anthropic` support

Extend the `async-anthropic` fork with `server_tool_use` and
`web_search_tool_result` content blocks, an unknown-block fallback arm,
`Text::citations`, the `citations_delta` stream delta, `Usage::server_tool_use`,
and the `pause_turn` stop reason.
This phase makes no changes to JP crates, is independently mergeable, and is a
prerequisite for observing the artifacts below.

### Phase 2: `ProviderPayload` and `EventKind::HostedActivity`

Add both types in `jp_conversation`, with serialization, no-op `sanitize`
behavior, the append-only transition rule, and the exhaustive-match arms listed
under Drawbacks.

### Phase 3: Streaming path

Add an `EventPart` variant carrying hosted activity, along with `EventBuilder`
accumulation and flush support.

### Phase 4: Citations

Add typed `Attribution` to assistant content, accumulate citations while a block
streams, finalize them on `Flush`, and preserve incomplete citations on partial
commits.

Each phase is independently mergeable.
Phases 2 through 4 remain inert until a provider adapter produces these
artifacts in [RFD D71].

## References

- `crates/jp_llm/src/provider/openai.rs`: the discarded output items
- `crates/jp_conversation/src/event.rs`: `EventKind`, `ChatResponse`
- `crates/jp_llm/src/event_builder.rs`: flush-metadata accumulation and partial
  commits
- `crates/jp_llm/src/provider/anthropic/acp/usage.rs`: usage reported as a
  diagnostic
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D45]: D45-elelem-a-standalone-llm-provider-streaming-crate.md
[RFD D71]: D71-provider-hosted-tools.md
