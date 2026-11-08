<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D45: Elelem: A Standalone LLM Provider Streaming Crate

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-30
- **Extends**: [RFD 012]
- **Summary**: Extracts LLM provider SSE streaming and event normalization into
  a standalone, feature-gated `elelem` crate.

## Summary

This RFD introduces `elelem`, a standalone, feature-gated crate that owns the
LLM streaming pipeline: a shared SSE driver, a normalized event model, and a
`ChunkParser` trait implemented per API shape.
It replaces the external provider SDKs and forks wrapped by `jp_llm`.

## Motivation

The `cerebras` and `llamacpp` SSE adapters recently had a bug where
`take_while(is_ok)` silently swallowed stream errors.
This exposed a structural problem: JP has a single streaming contract, but each
provider implements it separately without a shared type or test suite to enforce
it.
Each provider wraps a different external crate that bundles serde wire types
with SSE stream processing, and JP forks and maintains several of those crates.

Bundling wire types with stream processing conflates two concerns:

- **Generic streaming plumbing**: connecting, driving the SSE stream, surfacing
  transport errors, and emitting normalized events with consistent flush and
  finish semantics.
  This logic is identical across providers and should be implemented and tested
  once.
- **Provider-specific logic**: building requests, parsing chunks, and handling
  provider quirks.
  This logic is irreducible and belongs in each provider or wire shape.

Because the generic plumbing is re-implemented per provider, individual adapters
drift and introduce silent failures while JP carries the maintenance cost of
multiple forked SDKs.

## Design

### Crate interface and features

Consumers depend on a single crate and enable providers through Cargo features:

```toml
elelem = { version = "0", features = ["anthropic"] }
```

The core types (`Event`, `StreamError`, and the `ChunkParser` trait) are always
available.
Each provider feature enables the corresponding API shape's wire request and
response types and its chunk parser.
The `transport` feature (enabled by default) gates the SSE driver and the
`reqwest` client builder.
Consumers that only need wire types and parsers, such as chunk validators or
callers with a custom HTTP stack, can set `default-features = false` to avoid
pulling in HTTP dependencies.

Provider names are the only other public Cargo features.
Shared shape modules are enabled internally via `#[cfg(any(feature = "cerebras",
…))]` rather than exposed as separate feature flags:

```toml
[features]
default = ["transport"]
transport = ["dep:reqwest", "dep:reqwest-eventsource", "dep:tokio"]
cerebras   = []
llamacpp   = []
ollama     = []
openrouter = []
anthropic  = []
# ...
```

### Boundary between elelem and JP

`elelem` owns the wire layer: the typed request and response schemas for each
API shape, the shape parsers, the SSE driver, and the `Event` and `StreamError`
types.
It does not build requests.

JP constructs `elelem`'s exported request types from a `ChatQuery`, applying all
provider quirks (cache control, thinking budgets, reasoning effort, schema
transforms, and Ollama's forced-tool system message), and passes the typed
request body, base URL, and auth headers to `elelem`.
`elelem` alone builds the `reqwest` client and the `EventSource`.
No public API accepts a prebuilt client or stream, which guarantees that every
request enforces the connect timeout and `Never` retry policy.

Request construction stays in JP because it has not been a source of streaming
bugs, and a provider-neutral input model expressive enough for every provider
quirk would add complexity without improving stream reliability.
This boundary makes `elelem` a typed wire client and streaming engine rather
than a high-level `chat()` abstraction: it shares the bug-prone stream driving
and event normalization without imposing unified request ergonomics.

### Normalized event model

`Event` and `StreamError` move from `jp_llm` into `elelem`'s public API, and
`jp_llm` re-exports them so JP uses a single event type:

```rust
enum Event {
    // a typed delta: message, reasoning, structured, or tool-call chunk
    Part { index: usize, part: EventPart, metadata: Map },
    // commit the parts grouped under `index`
    Flush { index: usize, metadata: Map },
    // emitted exactly once
    Finished(FinishReason),
}
```

As defined in [RFD 012], `index` is an opaque grouping key rather than a
semantic slot.
`Part` events sharing an `index` accumulate until a matching `Flush` commits
them, and parsers emit flushes in stream order.
Shapes assign indices differently: chat completions use `0/1/2+`, while other
shapes use provider-native indices (Anthropic content blocks, OpenAI
`output_index`, Google virtual indices).
Callers must not attach meaning to the number beyond grouping parts for a flush.

### Shared SSE driver

A single driver owns the stream lifecycle across all providers:

- **Client construction**: The driver is built exclusively through the shared
  client builder, which sets the connect timeout and
  `EventSource::set_retry_policy(Never)`.
  A new provider cannot omit either setting.
- **Stream-idle timeout**: The driver enforces a stream-idle timeout from a
  caller-supplied duration and emits a retryable `StreamError` when it fires.
  JP passes `assistant.request.stream_idle_timeout_secs` (where `0` disables the
  timeout), so `elelem` owns the timeout mechanism while JP owns the configured
  value.
- **Completion and error handling**: The driver surfaces transport errors before
  completion as retryable `StreamError`s, converts a stream that ends before
  emitting `Finished` into a retryable error, and ignores the normal stream
  close that follows `Finished`.
  Each shape parser translates its protocol termination signal (`[DONE]` for
  chat completions, or a named event elsewhere) into `Finished`, so the driver
  keys off `Finished` rather than any protocol literal.
- **No internal retries**: Retry budgets, backoff, and user notifications stay
  in JP.
  Neither `elelem`'s driver nor any shape parser retries on its own, and the
  contract suite asserts this invariant.

### Chunk parser and wire shapes

An API *shape* implements `ChunkParser`, not an individual provider.
The driver feeds lifecycle frames into the parser, and the parser emits
normalized events:

```rust
enum Frame<'a> {
    Open,
    Message { event_name: Option<&'a str>, data: &'a str },
    Eof,
}

trait ChunkParser {
    fn parse(&mut self, frame: Frame<'_>) -> Vec<Result<Event, StreamError>>;
}
```

`Frame` makes stream lifecycle transitions explicit to the parser.
On `Eof`, the parser flushes any trailing state, while the driver checks whether
`Finished` was emitted to distinguish a clean post-`[DONE]` close from a
premature `Eof`.
`Frame::Message` carries `event_name` (already exposed by `reqwest_eventsource`)
because Anthropic sends named SSE events (`event: content_block_delta`) whereas
OpenAI-style streams send anonymous `data:` frames.

Each *provider* selects one of four shapes and supplies its request construction
and quirks.
All four chat-completions providers share one shape parser:

| Shape                           | Providers                              |
| ------------------------------- | -------------------------------------- |
| Chat Completions (OpenAI-style) | cerebras, llamacpp, ollama, openrouter |
| Responses (OpenAI)              | openai                                 |
| Messages (Anthropic)            | anthropic                              |
| Gemini                          | google                                 |

### Request recovery on the error channel

Some providers reject an otherwise-valid request because of stale metadata; for
example, Anthropic and Google invalidate old thinking and thought signatures.
Recovering requires stripping the offending metadata from the conversation
history and retrying.
Because this is error recovery rather than response content, it belongs on the
error channel rather than the event stream.
This replaces `Event::Patch` and `FinishReason::Retry`, which routed history
mutations through the content stream, with a recoverable error variant:

```rust
enum StreamError {
    Timeout, Connect, RateLimit, Transient, /* ... */
    Recoverable { patches: Vec<Patch> },
}

struct Patch  { matcher: Match, action: Action }   // generic, content-addressed
enum   Match  { MetadataValue { key: String, value: String } }
enum   Action { RemoveMetadata(String) }
```

Because the shape owns the metadata keys emitted on the success path, it builds
the `Patch` from the wire request and the rejection error and surfaces it as
`StreamError::Recoverable`.
Constructing a patch requires the request and rejection error rather than stream
frames, so it is a separate function on the shape alongside `ChunkParser`.
JP then finds every persisted event where `metadata[key] == value`, removes
`key`, rebuilds the request, and retries.
Matching by value keeps JP a generic patch applier with no provider-specific
recovery logic.

Unlike a transient transport failure, `Recoverable` requires caller action.
JP applies the patches and retries immediately, without backoff and without
spending the transient-error retry budget.
`Recoverable` also carries the underlying provider error: if a caller does not
apply patches, or if no stored metadata matches, the caller surfaces that error
as an ordinary failure rather than looping.
Because each applied patch removes metadata and an unmatched patch terminates,
recovery is bounded.

### Uniform SSE transport

Every provider streams SSE through `reqwest_eventsource`.
Two endpoint choices keep transport uniform across providers:

- **Ollama** uses `/v1/chat/completions`, sharing the chat-completions shape
  with no new parser.
  That endpoint supports streaming, tools, vision, structured output, and
  reasoning via `reasoning_content` and `reasoning_effort`.
  Ollama's `/v1/responses` endpoint is stateless only and drops vision and
  structured output while adding nothing JP uses, so it is not used.
  Because `/v1/chat/completions` has no `tool_choice` field, JP keeps its
  forced-tool system-message workaround when building requests.
  See [Ollama OpenAI compatibility] and [Ollama Anthropic compatibility].
- **Gemini** uses `?alt=sse`.

### Integration in jp_llm

JP's `Provider` trait is unchanged for callers such as `jp_cli`: it still
returns a stream of `Event` (re-exported from `elelem`).
Below the trait, each provider implementation builds the wire request, calls
`elelem` to drive and parse the stream, and owns everything outside
single-request wire streaming:

- **Request construction and quirks**: translating `ChatQuery` into the shape's
  request type.
- **Retries and idle timeout**: managing retry budgets and backoff, and passing
  the idle-timeout duration to `elelem` per request (replacing JP's
  `with_idle_timeout` stream wrapper).
- **Recovery**: applying `Recoverable` patches to conversation history and
  retrying.
- **Multi-request orchestration**: Anthropic max-token chaining and forced-tool
  fallback, and Google unexpected-tool-call retries.
- **Event accumulation**: `EventBuilder` stays in `jp_llm` because it translates
  `EventPart` into JP's persistence type `ConversationEvent`.
- **Non-streaming fallback**: `elelem` owns the OpenAI Responses wire types for
  both streaming and non-streaming responses, but only provides streaming
  transport.
  For `streaming_unsupported` models, JP performs the single HTTP call and maps
  `elelem`'s response type into `Event`s.

## Drawbacks

- **Replacing forked SDKs can expose hidden edge cases.** Forked crates may
  quietly encode provider edge cases; vendoring them first (Phase 0) and gating
  each migration on the contract suite and cassettes mitigates this risk.
- **Public API commitment.** Publishing `elelem` makes its `Event` model,
  `StreamError`, and wire types a semver commitment.
- **Asymmetric migration effort.** The chat-completions shape is nearly free
  because `cerebras` and `llamacpp` are already hand-rolled, whereas OpenAI
  Responses, Anthropic Messages, and Gemini require full request and response
  schema definitions.
- **Ongoing crate maintenance.** Maintaining a standalone multi-provider crate
  alongside existing libraries (`genai`, `async-openai`, and others) carries
  long-term maintenance cost.

## Alternatives

- **Extract a shared driver for `cerebras` and `llamacpp` only, and keep the
  SDKs.** This fixes the immediate duplication between those two providers, but
  leaves the fork-maintenance burden, does not enforce the streaming contract on
  SDK-wrapped providers, and provides no foundation for external plugins.
- **Split into a core crate, shape crates, and provider crates.** Finer crate
  granularity hurts ergonomics for consumers, who want a single dependency with
  feature flags rather than multiple crates to assemble.
- **Use one driver and one parser for all providers.** The wire protocols differ
  substantially, and forcing Anthropic Messages and Gemini through a
  chat-completions parser would trade correctness for uniformity.
- **Route every provider through the Responses API.** The Responses API is
  worthwhile for OpenAI proper (encrypted reasoning continuity, hosted tools,
  and o-series models), but a chat-completions backend gains nothing from a
  Responses compatibility shim.

## Non-Goals

- **Re-implementing `reqwest_eventsource`.** The silent-error bug was in JP's
  stream handling, not in `reqwest_eventsource`.
- **Adding new providers.**
- **Stateful Responses API support.** JP does not use stateful Responses for
  OpenAI either.

## Risks and Open Questions

- **Quirk parity.** The Anthropic and Google signature-strip-and-retry behaviors
  must survive the migration unchanged, guarded by the contract suite and
  re-recorded cassettes.
- **Cassette re-recording needs live endpoints.** `llamacpp` and `ollama`
  require local servers; the stale `llamacpp` cassettes surfaced by the original
  fix are the first to re-record.
- **Publication timing.** Publishing `elelem` overrides the workspace `publish =
  false` setting and freezes its feature names, `Event`, `StreamError`, and
  per-shape request types as public API.
  `elelem` should not be published until at least two non-chat shapes are
  migrated so the public surface is not biased toward chat completions.

## Implementation Plan

- **Phase 0: Vendor.** Pull the forked external crates into the workspace as
  plain members, keep every cassette green, and drop the upstream forks.
  This phase is independently reviewable and does not block Phase 1, which
  extracts from the hand-rolled `cerebras` and `llamacpp` adapters and needs no
  vendored SDK.
- **Phase 1: Core plus chat shape.** Create `crates/contrib/elelem`.
  Extract the generic core (types, `ChunkParser`, driver, and client builder)
  from `cerebras` and `llamacpp`, implement the chat-completions shape, seed the
  stream-contract test suite with the error-surfacing and premature-EOF cases,
  and migrate `cerebras` and `llamacpp` onto `elelem`.
- **Phase 2: Fold the chat family.** Switch `ollama` to `/v1/chat/completions`
  and migrate `ollama` and `openrouter` onto the chat shape.
  Delete `ollama-rs` and `jp_openrouter`, including `openrouter`'s internal
  `backon` retry loop (since JP owns retries).
- **Phase 3: Anthropic Messages.** Implement the Messages shape, migrate
  `anthropic`, and drop `async_anthropic`.
  This phase is the first to exercise named SSE events and the
  `StreamError::Recoverable` recovery path.
- **Phase 4: OpenAI Responses.** Implement the Responses shape, migrate
  `openai`, and drop `openai_responses`.
- **Phase 5: Gemini.** Implement the Gemini shape over `alt=sse`, migrate
  `google`, and drop `gemini_client_rs`.

Each phase is gated by the contract suite and re-recorded cassettes, and each
provider cuts over independently so the migration stays reviewable and
reversible.

## References

- [RFD 012] defines the typed `Event` and `EventPart` streaming model that
  `elelem` relocates out of `jp_llm`.
- [Ollama OpenAI compatibility] and [Ollama Anthropic compatibility] document
  the endpoints behind the Ollama transport decision.
- [RFD 043] (Discussion) stays in `jp_llm`: `elelem` emits raw tool-call chunks,
  while `EventBuilder` owns their accumulation and UI progress.
- [RFD 064] (Discussion) may later move patch application from in-place history
  mutation to stored events applied at projection time.

[Ollama Anthropic compatibility]: https://docs.ollama.com/api/anthropic-compatibility
[Ollama OpenAI compatibility]: https://docs.ollama.com/api/openai-compatibility
[RFD 012]: ../012-typed-llm-streaming-events.md
[RFD 043]: ../043-incremental-tool-call-argument-streaming.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
