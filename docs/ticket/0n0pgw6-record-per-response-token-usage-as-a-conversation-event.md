# Record per-response token usage as a conversation event

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=llm
- **Label**: package=jp_conversation
- **Label**: package=jp_llm
- **Label**: type=feature

Providers report token counts on every response and some report cost.
JP carries none of it into anything durable.
`jp_openrouter::responses::Usage` and `jp_openrouter::types::response::Usage`
exist as wire types with `input_tokens`, `output_tokens` and `cost`; nothing
lifts them into `jp_llm::Event` or the conversation stream.
No other provider's usage is read at all.

## What it costs

JP cannot answer "did that change help?" for any change to the harness.
Every other proposal in T-0n0nwz3 is unfalsifiable without this: there is no way
to tune a compaction threshold, judge whether a turn budget fires too early, or
tell whether moving a plan out of history saved anything.

The harness study's whole contribution is that it measured.
It reports cost per task, peak context as a fraction of the window, turns, and
tool calls per task, and every one of its findings is a comparison between those
numbers.
JP has the richer substrate, a durable event stream with stable event IDs (RFD
097), and none of the measurements.

Day to day, a user also has no way to see what a conversation has cost.

## Shape

A `Usage` event kind on the conversation stream, appended per provider response.
As an event it inherits durability, provider-invisibility under projection, and
the whole `jp conversation` read surface without new machinery.

Fields worth carrying: input, output, cache-read and cache-write token counts,
the resolved model ID, and cost where the provider reports it.

Where the pieces belong:

- Normalization in `jp_llm::Event`, because providers disagree on shape and the
  disagreement should not leak past the provider boundary.
- The event type in `jp_conversation::event`.
- Recording in the turn loop, alongside the existing mid-turn flush.

T-0fedmkq needs the same plumbing from the other end: it wants the served
service tier recorded alongside the turn, and notes that the OpenRouter provider
never sets the request's `usage` flag, so `Usage.cost` arrives unpopulated.
Worth doing together.

## Caveat

This is a signal, not a target.
Goodhart applies the moment a number like "tokens per turn" becomes something to
optimize directly.

Findings and the rest of the proposals: T-0n0nwz3.
