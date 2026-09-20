# The query path never fits a conversation to the model's context window

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-18
- **Label**: domain=conversation
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=enhancement

`jp query` builds a `Thread` from the full conversation stream and hands it to
the provider.
Nothing between the stream and the wire checks it against
`ModelDetails::context_window`.

`jp_llm::window::truncate_to_fit` exists and documents itself as the entry point
for "fitting a conversation into a model's context window".
It has two production call sites: `jp_llm::title` for title generation and
`jp_cli::cmd::query::tool::inquiry` for inquiry sub-requests.
Neither is the query path.
It was introduced for inquiries in #441 and reused for titles in #895; nothing
removed it from `query`, it was never there.

## What it costs

The common case is recoverable and clearly signalled: the provider returns
`ContextWindowExceeded` and the user runs `jp conversation compact`.
That is why this is a gap rather than a defect.

Two cases are worse:

- A provider that clamps instead of rejecting produces no signal at all.
  See T-0de0hry: Cerebras shortens completions as the window fills and the user
  is told nothing.
- RFD D50 (automatic compaction) delegates the single-turn overflow case back to
  this path: "A single turn that overflows on its own remains the domain of
  hard-fail and truncation."
  That backstop does not exist, so D50's own reasoning has a hole in it.

## Scope worth settling

Whether dropping the oldest events is the right shape here.
`truncate_to_fit` drops from the front, which on a long session removes the
original request and the early exploration while keeping the most recent tool
output.
The harness study in T-0n0nwz3 keeps the preamble and a verbatim recent window
and compacts only the middle, preferring to stub bulky tool observations before
dropping anything.
JP's compaction policies (RFD 064) already express that shape, so a query-path
backstop could reuse them rather than the blunt drop.

The related decision is ordering.
With an automatic compaction trigger in place this is a true backstop that
rarely runs; without one it is the only mechanism.
Landing them in either order works.
Landing neither leaves the provider error as the only guard.

Findings and the rest of the proposals: T-0n0nwz3.
