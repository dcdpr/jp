# Cerebras silently shortens answers as a conversation fills the context window

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-04

Cerebras clamps a request's completion budget to the room left in the context
window rather than rejecting the request.
Measured against `gpt-oss-120b` (131,000-token window): a ~128,000-token prompt
asking for 40,960 completion tokens returned cleanly after roughly 2,900,
terminating in `finish_reason: length`.

JP maps that to `FinishReason::MaxTokens` and handles it structurally, so
nothing breaks.
But the user is told nothing.
As a conversation grows, answers get progressively shorter for a reason
invisible from the transcript, and the shape of the resulting report is
"Cerebras gives short answers".

The fix is an explanation, not a mechanism.
JP knows the model's context window (`ModelDetails::context_window`) and can see
`finish_reason: length`, so it can say the answer was cut short because the
conversation left little room, and point at compaction.

## Scope worth settling

Whether this belongs in the Cerebras provider or in the shared handling of
`MaxTokens`.
Any provider that clamps rather than rejects produces the same experience, and a
provider that rejects instead produces a `ContextWindowExceeded` that already
says so.
Putting it in the shared path means deciding what the message says when JP does
*not* know the context window, which is the common case for a model absent from
a provider's table.

Distinguishing "hit the user's configured `max_tokens`" from "hit the window"
matters here too: the first is the user's own ceiling and needs no explanation,
the second does.
Both arrive as the same `finish_reason`.

Found while investigating \#1069; not caused by it.
