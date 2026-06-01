<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D45: Elelem: a standalone LLM provider streaming crate

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-30
- **Extends**: [RFD 012](../012-typed-llm-streaming-events.md)

## Summary

`elelem` is a standalone, feature-gated crate that owns the LLM streaming
pipeline: one SSE driver, one normalized event model, and a per-shape parser
seam, with each provider behind a Cargo feature. It replaces the external
provider SDKs JP currently wraps and in several cases forks.

## Motivation

A recent bug, in which the `cerebras` and `llamacpp` SSE adapters silently
swallowed stream errors via `take_while(is_ok)`, exposed a structural problem:
JP has one streaming contract with several divergent implementations, and the
contract is written down nowhere that a compiler or test enforces it. Each
provider wraps a different external crate, and those crates do two things: serde
types and SSE stream processing. Several of them JP forks and maintains.

This conflates two concerns. Generic streaming plumbing (connect, drive the SSE
stream, surface errors, emit a normalized event stream with consistent
flush and finish semantics) should be DRY, owned, and tested once.
Provider-specific logic (request building, chunk parsing, quirks) is irreducible
and stays per provider. Today the generic part is copied and re-derived per
provider, which is how one copy drifted into a silent-error bug.

Doing nothing means continued divergence, more latent bugs of this class, and
ongoing fork maintenance across several crates.

## Design

### What a consumer sees

One crate, one dependency, providers behind features:

```toml
elelem = { version = "0", features = ["anthropic"] }
```

The generic core is always available: the `Event` and `StreamError` types and
the `ChunkParser` trait. Each provider feature adds that shape's typed wire
request and response types and its parser. The SSE driver and the `reqwest`
client builder sit behind the `transport` feature, on by default, so a consumer
that only wants the wire types and parsers (to validate or transform chunks, or
drive its own HTTP stack) can depend on `elelem` with `default-features = false`
and pull in no HTTP dependencies. Provider features are the only public Cargo
feature switches; the shared shape module is enabled internally via
`#[cfg(any(feature = "cerebras", …))]`, not separately feature-gated:

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

### What elelem owns, and what JP keeps

elelem owns the wire: the typed request and response types for each API shape,
the shape parsers, the SSE driver, and the `Event` and `StreamError` types. It
does not own request *building*. JP populates elelem's exported request types
from a `ChatQuery`, including every provider quirk (cache control, thinking
budgets, reasoning effort, schema transforms, Ollama's forced-tool system
message), and hands that to elelem as data: the typed body, the base URL, and
auth headers. elelem alone builds the `reqwest` client and the `EventSource`; no
public API accepts a prebuilt client or stream, which is what makes the
connect-timeout and `Never` guarantees unforgeable. Request construction has
never been a source of streaming bugs, and a provider-neutral input model
expressive enough for every quirk would cost far more than it saves, so it stays
in JP.

This makes elelem a typed wire client plus a streaming engine, a lower tier than
a high-level `chat()` abstraction. The reusable, bug-prone part (driving and
normalizing the stream) is shared; the request ergonomics are not.

### The normalized event model is the contract

`Event` and `StreamError` move out of `jp_llm` and become `elelem`'s public
API; `jp_llm` re-exports them so JP keeps one event type, not a parallel copy.
The model is small:

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

`index` is an opaque grouping key, not a fixed slot: parts sharing an index
accumulate until their `Flush`, and parsers emit flushes in stream order. Some
shapes number them `0/1/2+` (chat completions), others use provider-native
indices (Anthropic content blocks, OpenAI `output_index`, Google virtual
indices). Callers must not attach meaning to the number. This matches
[RFD 012], which defined the index as a grouping key, not a semantic slot.

### One SSE driver

A single driver owns the stream lifecycle, so it cannot diverge per provider:

- It is built only through the shared client builder, the one place that sets
  the connect timeout and `EventSource::set_retry_policy(Never)`. A new provider
  cannot forget either, which is the exact failure mode behind the original bug.
- It enforces a stream-idle timeout from a value the caller passes in (JP
  supplies `assistant.request.stream_idle_timeout_secs`, where `0` disables).
  elelem owns the timeout *mechanism* and emits a retryable `StreamError` when
  it fires; JP owns the *value*.
- It surfaces transport errors before completion as a retryable `StreamError`,
  converts a stream that ends without a terminal `Finished` into a retryable
  error, and drops the benign close that follows that `Finished`. The terminal
  signal is the shape's own (`[DONE]` for chat completions, a named event
  elsewhere); the driver keys off `Finished`, not any protocol literal. These
  rules are the regression guard the whole crate exists for.
- Retry budget, backoff, and user notification stay in JP. elelem never retries
  on its own, and the contract suite asserts no shape or driver does.

### The parser seam and the four shapes

A *shape* implements the parser, not a provider. The driver feeds it lifecycle
frames and the parser emits normalized events; provider-specific parsers exist
only when a wire shape is genuinely unique.

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

The `Frame` makes the lifecycle explicit, which is the point: the parser flushes
any trailing state on `Eof`, and the driver, tracking whether a terminal
`Finished` was emitted, suppresses the benign post-`[DONE]` close and converts a
premature `Eof` into a retryable error. `event_name` is carried because
Anthropic sends named SSE events (`event: content_block_delta`) while
OpenAI-style streams are anonymous `data:` frames; `reqwest_eventsource` already
exposes the name.

A *provider* selects a shape and supplies its request construction and quirks.
The four chat-completions providers share one shape parser:

| Shape                            | Providers                                |
| -------------------------------- | ---------------------------------------- |
| Chat Completions (OpenAI-style)  | cerebras, llamacpp, ollama, openrouter   |
| Responses (OpenAI)               | openai                                   |
| Messages (Anthropic)             | anthropic                                |
| Gemini                           | google                                   |

### Recovery: request rejection without a side channel

Some providers reject an otherwise-valid request because of stale metadata:
Anthropic and Google invalidate old thinking and thought signatures. The fix is
to strip the offending metadata from the conversation history and retry. That is
recovery, not response content, so it rides the error channel, not the event
stream. `Event` has no `Patch` variant and `FinishReason` has no `Retry`; both
are deleted.

```rust
enum StreamError {
    Timeout, Connect, RateLimit, Transient, /* ... */
    Recoverable { patches: Vec<Patch> },
}

struct Patch  { matcher: Match, action: Action }   // generic, content-addressed
enum   Match  { MetadataValue { key: String, value: String } }
enum   Action { RemoveMetadata(String) }
```

The shape builds the patch from the wire request and the rejection error (it
owns the metadata-key vocabulary it emitted on the success path) and surfaces it
as `StreamError::Recoverable`. Because that needs the request and error, not
stream frames, it is a separate function on the shape from the frame-oriented
`ChunkParser`; the shape owns both seams. JP applies it: find the persisted
event whose `metadata[key] == value`, remove the key, rebuild the request, and
retry. The match is by value, so JP needs no provider knowledge and stays a
generic applier.
`Event::Patch` was a transport hack that routed a history mutation through the
content channel; moving it to the error channel removes both the conflation and
any second `Event` type.

`Recoverable` is caller-action-required, not a transient transport failure, so
JP applies the patches and retries immediately, without backoff and without
spending the transient-error retry budget. It carries the underlying provider
error: a caller that will not patch, or where no stored metadata matches,
surfaces that error as an ordinary failure rather than looping. Each applied
patch makes progress and an unmatched patch terminates, so recovery is bounded.

### Transport is uniform SSE

Every provider streams SSE through `reqwest_eventsource`. Two decisions make
that hold:

- **Ollama** uses `/v1/chat/completions`, folding it into the chat shape with
  no new parser. That endpoint supports streaming, tools, vision, structured
  output, and reasoning via `reasoning_content` and `reasoning_effort`.
  Ollama's `/v1/responses` endpoint is non-stateful only and drops vision and
  structured output while adding nothing JP uses, so it is rejected. The chat
  endpoint has no `tool_choice`; JP keeps its existing forced-tool
  system-message workaround when it builds the request. See
  [Ollama OpenAI compatibility] and [Ollama Anthropic compatibility].
- **Gemini** uses `?alt=sse`.

### JP side

JP's `Provider` trait is unchanged from JP CLI's point of view: it still returns
a stream of `Event` (now elelem's, re-exported). Below the trait, each provider
implementation builds the wire request, calls elelem to drive and parse it, and
owns everything that is not single-request wire handling: request construction
and quirks, retry budget and backoff, the idle-timeout value (passed to elelem
per request, so JP no longer wraps the stream with `with_idle_timeout`),
recovery (applying `Recoverable` patches and retrying), the multi-request
orchestration some providers need (Anthropic max-token chaining and forced-tool
fallback, Google unexpected-tool-call retry), and `EventBuilder`, which stays
in `jp_llm` because it translates `EventPart` into the persistence type
`ConversationEvent`. elelem owns the OpenAI Responses wire types for both
streaming and non-streaming responses but only the streaming transport, so the
non-streaming fallback for `streaming_unsupported` models is JP's HTTP call
mapping elelem's response type into events.

## Drawbacks

- Re-inlining forked crates risks re-discovering edge cases they quietly
  encode. Chesterton's Fence applies per crate; the vendor-first step and the
  contract suite are the mitigations.
- `elelem`'s event model becomes a semver commitment once published. That is
  the price of "standalone, reusable."
- The effort is wide and asymmetric: chat-completions is nearly free (already
  hand-rolled), but Responses, Messages, and Gemini carry real request-schema
  work.
- A standalone multi-provider client enters a populated space (genai,
  async-openai, and others). Maintaining a public crate is its own ongoing
  cost.

## Alternatives

- **Extract a shared driver for cerebras/llamacpp only, keep the SDKs.** Fixes
  the immediate duplication but leaves the fork-maintenance burden, does
  nothing for the SDK-wrapped providers, and gives the plugin future no
  foundation.
- **A core crate plus N shape crates plus N provider crates.** More granular,
  but worse ergonomics for a reusable standalone library; consumers want one
  dependency and a feature, not a crate-assembly job.
- **One driver and one parser for all providers, dropping the SDKs entirely.**
  A Golden Hammer: the wire shapes genuinely differ, and forcing Anthropic and
  Gemini through a chat-completions parser would trade correctness for
  uniformity.
- **Route everything through the Responses API.** Responses earns its place for
  OpenAI proper (encrypted reasoning continuity, hosted tools, the o-series),
  but a chat-completions-compatible backend gains nothing from its Responses
  shim.

## Non-Goals

- Re-implementing `reqwest_eventsource`. The bug was JP's usage, not the crate;
  it stays.
- Adding new providers.
- Stateful Responses API support (JP does not use it for OpenAI either).

## Risks and Open Questions

- **Quirk parity.** The Anthropic and Google signature-strip-and-retry
  behaviors must survive the migration unchanged. The contract suite plus
  re-recorded cassettes are the guard.
- **Cassette re-recording needs live endpoints.** `llamacpp` and `ollama`
  require local servers; the stale `llamacpp` cassettes surfaced by the
  original fix are the first to re-record.
- **Publication timing.** Publishing elelem overrides the workspace
  `publish = false` and freezes its feature names, `Event`, `StreamError`, and
  per-shape request types as public API. Don't publish until at least two
  non-chat shapes are migrated, so the surface isn't frozen chat-biased.

## Implementation Plan

- **Phase 0: Vendor.** Pull the forked external crates into the workspace as
  plain members, keep every cassette green, and drop the upstream forks.
  Reviewable on its own, and independent of Phase 1, which extracts from the
  hand-rolled `cerebras` / `llamacpp` and needs no vendored SDK.
- **Phase 1: Core plus chat shape.** Create `crates/contrib/elelem`. Extract
  the generic core (types, `ChunkParser`, driver, client builder) from
  `cerebras` / `llamacpp`, implement the chat-completions shape, and seed the
  stream-contract test suite (the surface/swallow cases already written). Move
  `cerebras` and `llamacpp` onto it.
- **Phase 2: Fold the chat family.** Move `ollama` (switched to
  `/v1/chat/completions`) and `openrouter` onto the chat shape. Delete
  `ollama-rs` and `jp_openrouter`, including `openrouter`'s internal `backon`
  retry loop, since JP owns retry.
- **Phase 3: Anthropic Messages.** Add the shape, drop `async_anthropic`. First
  real exercise of the named-event seam and the `StreamError::Recoverable` path.
- **Phase 4: OpenAI Responses.** Add the shape, drop `openai_responses`.
- **Phase 5: Gemini.** Add the shape on `alt=sse`, drop `gemini_client_rs`.

Each phase is gated by the contract suite and re-recorded cassettes, and each
provider cuts over independently, so the migration stays reviewable and
reversible throughout.

## References

- [RFD 012], which defined the typed `Event` / `EventPart` streaming model that
  `elelem` relocates out of `jp_llm` and owns.
- The Ollama compatibility docs behind the transport decision.
- [RFD 043] (Discussion) stays JP-side: elelem emits raw tool-call chunks;
  `EventBuilder` owns their accumulation and any UI progress.
- [RFD 064] (Discussion) may later move patch application from in-place mutation
  to stored events applied at projection time.

[RFD 012]: ../012-typed-llm-streaming-events.md
[RFD 043]: ../043-incremental-tool-call-argument-streaming.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[Ollama OpenAI compatibility]: https://docs.ollama.com/api/openai-compatibility
[Ollama Anthropic compatibility]: https://docs.ollama.com/api/anthropic-compatibility
