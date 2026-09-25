# RFD D31: Provider Response Artifacts

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Required by**: [RFD D71]

## Summary

LLM providers return response artifacts JP cannot represent: activity executed
on the provider's own infrastructure, citations attached to assistant text, and
usage counters for that activity.
This RFD adds a normalized **hosted activity** event, a typed **provider
payload** envelope for exact same-provider replay, typed citations on assistant
content, and hosted-activity usage.

It adds no user-visible feature on its own.
It is the response-model prerequisite for exposing any provider-hosted tool.

## Motivation

### JP currently discards these artifacts

The OpenAI adapter drops three output-item kinds outright — `FileSearch`,
`WebSearchResults`, and `ComputerToolCall` — in three places
(`crates/jp_llm/src/provider/openai.rs:282`, `:1445`, `:1482`), along with the
`WebSearchCallInitiated` / `Searching` / `Completed` stream events.
The comment above the last of these is explicit that the variants are listed
rather than wildcarded "so a new variant in the upstream event enum fails to
compile until someone decides whether JP needs it."
This RFD is that decision.

The Anthropic path cannot even parse the artifacts.
`async_anthropic::types::MessageContent` is an internally-tagged enum over six
variants with no catch-all, so a `server_tool_use` content block fails
deserialization and takes the stream down with it.

### The response model cannot carry attribution

`ChatResponse::Message { message: String }` is a bare string.
Anthropic, OpenAI, and Google all impose attribution or display obligations on
search-backed output, and Anthropic's citation payload carries a URL, title,
cited text, and an `encrypted_index` that must round-trip.
None of that fits in a `String`.

This is a deeper gap than any configuration question: a feature that renders
search-backed text without its citations is incomplete and potentially
non-compliant, and the shape of `ChatResponse` is what makes it impossible.

### Waiting makes it more expensive

[RFD D45] proposes publishing the normalized streaming event model as the public
API of a standalone `elelem` crate, and names hosted tools as a reason to prefer
OpenAI's Responses API.
Freezing an event model that cannot represent hosted activity or attribution
means paying for this twice.

### If nothing is done

Provider-hosted tools cannot be exposed at all, because there is nowhere to put
what comes back.
The dropped OpenAI artifacts stay dropped, and the next provider adds a fourth
site that discards them.

## Design

### Hosted activity is not a Tool Call

JP already has a precise concept for "a callable operation whose request JP must
fulfill": the Tool Call.
Its contract is that JP resolves policy, dispatches a runtime, records a
`ToolCallResponse`, and returns the result to the provider.
Every consumer relies on it — `turn_loop.rs` prepares each committed
`ToolCallRequest`, `ConversationStream::sanitize_orphaned_tool_calls` injects a
response for any request lacking one, and the restart path scans for unresponded
requests.

Provider-hosted activity satisfies none of that.
The provider runs the work inside its own agent loop and continues generating.
JP *observes* it.

Recording it as a `ToolCallRequest` would make the durable stream assert that JP
dispatched work it never dispatched, and would put every one of those consumers
in the position of needing an exception.
It gets its own event kind instead.

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

State changes are **append-only transitions** linked by `id`, not one event
mutated in place.
"Started with no terminal transition" therefore has a defined meaning: the
activity began and never finished, which a reader can detect and a provider
adapter can act on.

Append-only is the established principle ([RFD 064]); the one existing deviation
(`jp_llm::event::apply_patches`) documents itself as a deviation.
This RFD does not add a second.

`sanitize_orphaned_tool_calls` matches on `EventKind::ToolCallResponse` and
`as_tool_call_request()`, so a distinct kind is invisible to it.
Orphan repair stays exclusive to JP-dispatched Tool Calls with no new condition
to remember.

### `ProviderPayload`

A typed envelope, not scattered metadata keys:

```text
ProviderPayload
    provider    ProviderId of the originating provider
    format      provider-defined payload discriminator
    version     format version selected when the activity started
    value       the native payload
```

Contract:

- **Only the originating provider interprets it.** Every other adapter preserves
  it and ignores it.
- **Immutable.** No consumer rewrites it.
  Compaction is non-destructive ([RFD 064]) and adds overlays, so this holds by
  construction.
- **Unknown versions round-trip unmodified.** A payload written by a newer JP is
  carried, not rejected.
- **`version` records the wire contract selected at start.** Anthropic's web
  search alone has three versions with different fields and defaults; without a
  recorded selection, a JP upgrade continues an activity under a contract it did
  not begin under.
- **Untrusted content.** The payload may contain arbitrary web content and is
  never rendered or logged directly (see [RFD 096]).
- **A present-but-undecodable payload is an error**, not a silent downgrade.
  This is distinct from a payload whose *continuation requirements* cannot be
  met, which is the consuming RFD's concern.

`value` is inline for now.
Anthropic's search payloads are large, so the envelope leaves room for a blob
reference once [RFD 066] exists; nothing here depends on that.

### Citations on assistant content

Citations annotate assistant text, so they attach to the assistant content
event, not to hosted activity.
A typed attribution carries the source URL, title, the cited span or excerpt,
and the provider-specific index required for replay.

Citations arrive when a text block completes, so the adapter attaches them
through the existing `Event::Flush { index, metadata }` path — the same
mechanism thinking signatures already use — and `EventBuilder` merges them into
the event on flush.

### Usage

Provider usage gains hosted-activity counters (Anthropic reports
`usage.server_tool_use.web_search_requests`).
These bill separately from tokens, so a user who cannot see them cannot predict
their spend.

Usage attaches to the terminal event of each provider response batch.
That is deliberately less precise than per-response attribution; see Non-Goals.

### Two projections

The result is that a conversation carrying hosted activity has two readings:

- **Portable** — activity name, state, input, summary, plus assistant text and
  citations.
  Survives export, search, rendering, and switching provider.
- **Provider-native** — the payload, meaningful only to the originating
  provider, used for exact replay.

## Drawbacks

**A new `EventKind` variant touches every exhaustive match.** At least seven
that I verified: `jp_attachment_internal/src/lib.rs`,
`jp_cli/src/cmd/query/tool/inquiry.rs`, `jp_cli/src/editor.rs`,
`jp_cli/src/render/turn.rs`, two in `jp_cli/src/shared/search.rs`, and
`jp_conversation/src/storage.rs`.
Provider `convert_event` implementations and the `serve-web` plugin's renderer
also need arms.
This is the cost of not overloading Tool Call, and it is paid once.

**Citations change a type every consumer of assistant text reads.** Adding
attribution to `ChatResponse::Message` means every reader that pattern-matches
message content sees a changed shape, even readers with no interest in
citations.

**The portable projection is lossy by design.** Switching provider
mid-conversation preserves the final assistant text, citations, and normalized
activity, but not the native state.
That is a deliberate limit, not an oversight — see Alternatives.

**Nothing user-visible ships.** This RFD cannot be validated by a user-facing
behavior on its own; its first real test is its consumer.

## Alternatives

**Record hosted activity as a `ToolCallRequest` / `ToolCallResponse` pair, with
a marker.** Rejected.
It requires JP to fabricate a response for work it never dispatched, and needs
coordinated exceptions in the turn loop, the execution plan, and orphan
sanitization.
Explored at length before this RFD; every iteration added another exception,
which is the signal that the model was wrong rather than incomplete.

**Produce a portable textual digest of native results so they survive a provider
switch.** Rejected.
Anthropic's `encrypted_content` is useful only to Anthropic, so any digest is
fabricated rather than translated: it cannot reproduce the original context, and
it risks surfacing content the provider deliberately kept opaque.
Preserve the final assistant text and normalized activity instead.

**Store the native payload as loose metadata keys**, as thinking signatures do
today.
Rejected for a payload that must carry provider identity, format, and version —
three fields whose relationship matters.
The signature precedent is a single opaque string; this is a structured envelope
with a contract.

**Extend `ChatResponse` with a hosted-activity variant** rather than adding an
`EventKind`.
Rejected: hosted activity is not assistant-authored content, and folding it in
means every consumer of assistant text handles a variant that has no text.

## Non-Goals

**Provider-response identity.** A single `ChatRequest` produces many provider
responses (max-token chaining, retries, paused-turn continuations, tool cycles),
and JP has no way to say which response produced which events.
That is a real gap, and it is deferred deliberately: [RFD 097] provides the
stable-identifier primitive that a provider-response record should build on, and
097 is not yet Accepted.
Depending on it here would gate this RFD's acceptance on 097's, for a capability
the first provider integration does not need.
Usage attaches to a response batch's terminal event until a consumer forces the
issue.

**Rendering.** This RFD defines the data a renderer needs.
Terminal and web rendering, including the citation-display obligations, belong
to the consuming RFD, where they gate a user-visible feature.

**Any specific provider's hosted tools.** No configuration surface, no request
translation, no provider capability list.

**Portable native state.** See Alternatives.

## Risks and Open Questions

- **This constrains [RFD D45].** D45 should not publish `elelem::Event` as
  public API before accounting for hosted activity and attribution, or the new
  crate breaks immediately or routes hosted artifacts through generic metadata
  — recreating the problem this RFD removes.
- **`async-anthropic` needs extending before any of this is observable.** The
  fork's `MessageContent` cannot parse `server_tool_use`, `Text` has no
  `citations`, and `Usage` has no `server_tool_use` counters.
  Whether serde can express a catch-all arm on that internally-tagged enum needs
  verifying — `#[serde(other)]` covers unit variants only, so an untagged
  `Unknown(Map)` arm is the likely shape, mirroring what `types::Tool` already
  does.
- **The normalized activity shape is derived from two providers.** Anthropic
  pairs a call block with a result block; OpenAI emits a call item plus an
  annotated message; Google may expose only completed grounding metadata with no
  call/result pair at all.
  `started | completed | failed` may not survive contact with Google.
  Validating against a second provider is the consuming RFD's job.
- **Citation shape across providers is unverified.** Anthropic's
  `web_search_result_location` is the only one modelled here.

## Implementation Plan

### Phase 1: `async-anthropic` support

Extend the fork: `server_tool_use` and `web_search_tool_result` content blocks,
an unknown-block arm, `Text::citations`, `Usage::server_tool_use`, and the
`pause_turn` stop reason.
No JP changes.
Independently mergeable, and a prerequisite for observing anything below.

### Phase 2: `ProviderPayload` and `EventKind::HostedActivity`

Add both types in `jp_conversation`, with serialization, `sanitize` behavior
(none), and the exhaustive-match arms listed under Drawbacks.
Includes the append-only transition rule.
Independently mergeable; nothing produces the event yet.

### Phase 3: Streaming path

An `EventPart` variant carrying hosted activity, plus `EventBuilder`
accumulation and flush.
Independently mergeable; no provider emits it yet.

### Phase 4: Citations and usage

Typed attribution on assistant content, attached via flush metadata.
Hosted-usage counters on the provider usage type.
Independently mergeable.

Phases 2 through 4 are all inert until a provider adapter produces the
artifacts, which is the consuming RFD's first phase.

## References

- `crates/jp_llm/src/provider/openai.rs` — the discarded output items
- `crates/jp_conversation/src/event.rs` — `EventKind`, `ChatResponse`
- `crates/jp_llm/src/event_builder.rs` — flush-metadata accumulation
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D45]: D45-elelem-a-standalone-llm-provider-streaming-crate.md
[RFD D71]: D71-provider-hosted-tools.md
