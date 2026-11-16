# RFD 118: Provider Response Artifacts

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Required by**: [RFD 119]
- **Summary**: Adds hosted-activity events, provider replay envelopes, and typed
  citations to `ConversationStream` for provider-executed tool output.

## Summary

This RFD extends JP's response and stream models to represent provider-executed
activity, exact provider replay payloads, and typed citations on assistant
messages.
Provider adapters parse usage counters for hosted activity and surface them as
diagnostics without persisting them.
These types provide the response model required before exposing provider-hosted
tools ([RFD 119]).

## Motivation

### Adapters discard or fail to parse hosted-tool artifacts

The OpenAI adapter drops three output-item kinds (`FileSearch`,
`WebSearchResults`, and `ComputerToolCall`) in three places
(`crates/jp_llm/src/provider/openai.rs:1175`, `:2952`, `:2990`), along with the
`WebSearchCallInitiated`, `Searching`, and `Completed` stream events.
The comment at `:2990` notes that the variants are listed explicitly rather than
matched with a wildcard "so a new variant in the upstream event enum fails to
compile until someone decides whether JP needs it."
This RFD makes that decision.

The Anthropic adapter cannot parse hosted-tool artifacts at all.
`async_anthropic::types::MessageContent` is an internally tagged enum over six
variants with no catch-all, so a `server_tool_use` content block fails
deserialization and aborts the stream.
The ACP subscription flow decodes the same stream types from Claude Code's raw
SDK messages, where an undecodable message ends the connection.

### The response model cannot carry attribution

`ChatResponse::Message { message: String }` is a bare `String` and cannot hold
structured attribution.
Anthropic, OpenAI, and Google all impose attribution or display obligations on
search-backed output, and Anthropic's citation payload carries a URL, title,
cited text, and an `encrypted_index` that Anthropic validates when the citation
is sent back.
Without structured attribution on `ChatResponse`, JP cannot render search-backed
text with the required citations.

### If nothing is done

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

Hosted activity is work done on the provider's side of JP's provider boundary:
on the vendor's servers, or in a runtime the provider adapter drives, such as
Claude Code on Anthropic's ACP subscription flow.
What makes it hosted is that JP never dispatches it, not where it runs.

Recording hosted activity as a `ToolCallRequest` would falsely assert in the
durable stream that JP dispatched the work and would force every
`ToolCallRequest` consumer to add an exception.
Hosted activity therefore uses a dedicated event kind.

### `EventKind::HostedActivity`

```text
HostedActivity
    id          correlation key: the provider's id, or one the adapter derives
    name        JP-facing capability name (e.g. "web_search")
    state       started | completed | failed
    input       normalized invocation input (e.g. the search queries)
    summary     short human-readable description for display
    sources     URLs and titles the activity returned, optional
    payload     ProviderPayload (see below), optional
```

`sources` is portable: a renderer shows it when the provider returns no
citations anchored in the assistant text.

State changes are recorded as **append-only transitions** linked by `id` rather
than mutating an event in place.
This matches the append-only stream principle ([RFD 064]) that provider-issued
repairs already follow, where `jp_llm::event::record_patches` appends an overlay
rather than rewriting target events.
An activity with a `started` transition and no terminal (`completed` or
`failed`) transition has a defined meaning: the activity began and never
finished, which a stream reader can detect and a provider adapter can handle on
continuation.

One activity can span two provider responses.
When Claude calls web search and a client tool in the same step, the response
ends with the search unrun; the next response, sent with the client tool's
result, opens with the search result, linked to the call by `id`.

An activity's first transition can also be its terminal one.
Anthropic and OpenAI identify each search and report it while it runs, so their
adapters record `started` and then `completed` or `failed`.
Google's `generateContent` reports search only as grounding metadata in the
response's last chunk, with no id and several queries per response.
Its adapter records one `completed` activity per response, with every query as
its input and an id derived from the response's `responseId`.

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

- **Interpreted only by its writer.** The adapter that produced a `format`
  replays it; every other adapter preserves the payload in the stream and
  ignores it.
  This includes other flows of the same provider: Anthropic's HTTP and ACP flows
  write different formats and replay only their own.
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
  If a payload in a format the active adapter wrote fails to decode, the adapter
  returns an error.
  Handling valid payloads whose *continuation requirements* cannot be met
  belongs to the consuming RFD ([RFD 119]).

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
OpenAI's `url_citation` and Google's grounding supports carry offsets into the
response text, which populate `range`.
A Google support can cite several sources, which becomes one attribution per
source with the same `range`.
`excerpt` always holds text quoted from the source and never substitutes for
`range`, so the answer text Google repeats in each support is not an excerpt.
Only Anthropic supplies an excerpt.

`range` indexes into the event's final stored text, counted in Unicode code
points.
OpenAI counts in code points, and Google in UTF-8 bytes, which its adapter
converts.
A code point is a property of the text rather than of an encoding, so a reader
in Rust, in the web client's JavaScript, or in Swift applies a range to the
string it decoded without re-encoding it first.
The cost falls on Rust, where finding a code-point offset walks the string.
An adapter that modifies text after citations arrive (such as Anthropic's
max-token continuation trimming overlapping text) must remap or drop affected
ranges so stale offsets are never persisted.

#### Streaming lifecycle

Citations arrive before the text they annotate is complete.
Anthropic opens a cited block with an empty `citations` list and sends its
`citations_delta` before the block's text.
OpenAI sends each `response.output_text.annotation.added` after the text it
covers and before `response.output_text.done`.
Google sends all grounding in the response's last chunk, after the text, so its
adapter holds the text event's flush until that chunk arrives.
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
tokens and reported in the final `message_delta` of the response that ran the
searches.
A search left unrun by one response is counted by the next response, which runs
it.

JP does not persist token usage: no HTTP provider adapter reads its usage
counters, and the ACP adapter keeps usage out of event metadata and logs it as a
`debug` diagnostic (`anthropic/acp/usage.rs`).
Hosted-activity counters follow the same path: the adapter parses and logs them
without writing them to the conversation stream.

Consumers can still count searches from the `HostedActivity` events in the
durable stream.
On Anthropic's HTTP flows, the number of `completed` activities equals the
number of billed searches: each completed search is billed once, and a search
that ends in an error, such as `max_uses_exceeded`, is not billed.
The count is not a billing figure on every flow: one activity on the ACP flow
can run several backend searches.
Persisting usage counters requires provider-response identity; see Non-Goals.

### Two projections

A conversation carrying hosted activity supports two projections:

- **Portable**: the activity name, state, input, summary, and sources, together
  with the assistant text and citations.
  This projection survives export, search, rendering, and switches of provider
  or flow.
- **Provider-native**: the `ProviderPayload`, which only the adapter that wrote
  it reads for exact multi-turn replay.

Anthropic and OpenAI accept either projection when the history is sent back.
Anthropic's native blocks are valid as long as they are unmodified: it rejects a
changed `encrypted_content` or `encrypted_index`.
The portable projection, with search blocks and citations removed, is valid too,
and so is any mix: citations without the search blocks they came from, or search
blocks without citations.
OpenAI, with `store: false`, accepts its `web_search_call` items replayed or
dropped, and annotations kept or cleared.
Dropping native data is safe; rewriting it is not.

Dropping it is not free.
In requests made against both APIs while checking this design, every replay that
kept the search's native record (Anthropic's search blocks, OpenAI's
`web_search_call` item) was answered without a new search, and every replay that
dropped it searched again: two of each on Anthropic, one kept and three dropped
on OpenAI.
The payload is what saves a repeat search, not only what makes replay exact, so
an OpenAI adapter has reason to keep its `web_search_call` items as payloads
even though OpenAI does not require them.

## Drawbacks

**Adding an `EventKind` variant updates every exhaustive match.** At least seven
call sites match exhaustively on `EventKind`:
`jp_attachment_internal/src/lib.rs`, `jp_cli/src/editor.rs`,
`jp_cli/src/render/turn.rs`, two in `jp_cli/src/shared/search.rs`,
`jp_conversation/src/storage.rs`, and `jp_llm/src/turn_summary.rs`.
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
through user-facing behavior until a provider adapter emits them in [RFD 119].

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
consumer ([RFD 119]) needs neither.
[RFD 097] is Implemented and supplies the stable-identifier primitive a
provider-response record would build on, so a later RFD can add both without a
dependency gate.

**Rendering.** This RFD defines the stream data structures only.
Terminal and web rendering, including citation-display obligations, belong to
the consuming RFD ([RFD 119]), where they gate a user-visible feature.

**Provider-specific hosted tools.** This RFD adds no configuration surface,
request translation, or provider capability list.

**Portable native state.** See Alternatives.

**Provider-mandated display content.** Google requires a grounded answer to be
shown with its Search Suggestions, delivered as HTML and CSS in
`searchEntryPoint.renderedContent`.
Nothing here models content a provider requires a client to render, and a
payload is never rendered.
The RFD that adds Google search adds that field, and decides whether a terminal
client can meet the requirement at all.

## Risks and Open Questions

- **The model is fitted to web search.** `HostedActivity` and `Attribution` are
  checked against the search responses of Anthropic, OpenAI, and Google.
  Other hosted tools return other artifacts: code execution, for example,
  returns files and a container to continue in.
  The RFD that exposes one of them checks it against this model first.

## Implementation Plan

### Phase 1: `async-anthropic` support

Extend the `async-anthropic` fork with `server_tool_use` and
`web_search_tool_result` content blocks, `Text::citations`, the
`citations_delta` stream delta, and `Usage::server_tool_use`.
The fork's `MessageContent` and `ContentBlockDelta` have no catch-all, so each
gains a final `#[serde(untagged)] Unknown(serde_json::Value)` variant, the
mechanism `types::Tool` already uses for its untagged variants.
A block the fork does not model is then carried and sent back unchanged instead
of failing the stream.
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
artifacts in [RFD 119].

## References

- `crates/jp_llm/src/provider/openai.rs`: the discarded output items
- `crates/jp_conversation/src/event.rs`: `EventKind`, `ChatResponse`
- `crates/jp_llm/src/event_builder.rs`: flush-metadata accumulation and partial
  commits
- `crates/jp_llm/src/provider/anthropic/acp/usage.rs`: usage reported as a
  diagnostic
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>

[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 097]: 097-stable-event-identifiers.md
[RFD 119]: 119-provider-hosted-tools.md
