# A tool call truncated by max_tokens is discarded without a record

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-07

When a response runs out of output tokens while the model is still assembling a
tool call, the partial call is dropped and nothing durable records that it
existed.

`TurnCoordinator::handle_streaming_event`
(`crates/jp_cli/src/cmd/query/turn/coordinator.rs:322-324`) collects the names
via `event_builder.incomplete_tool_calls()`, with the comment "Capture tool-call
buffers about to be discarded", purely so `finish_notice` can name them in a
chrome line.
The call is never executed and never persisted.

The reason itself is dropped too: `transition_from_streaming` takes it as
`_reason` (`coordinator.rs:364-367`) and ignores it, and `FinishReason` does not
appear anywhere in `jp_conversation`.

So the surviving record is the prose the model got through, which reads as if
the assistant considered the action and chose not to take it.
Replaying the conversation later shows the same thing, because there is nothing
else to show.

## What it costs

A caller asked for an edit, got an answer, and exited 0.
The edit never happened and no artifact says so.
`jp conversation print` cannot show it, and neither can the macOS app, since
both read the stream.

## Proposal

Record the finish reason on the turn, and the discarded tool call names with it.
That gives the fact a second home, which is what a rare-but-consequential event
wants: the user is not watching for it, so a durable record they can find
afterwards serves them better than a line that scrolls past.

It also removes the reason `finish_notice` is currently the only witness to
anything, which keeps `--quiet` free of exemptions.

## Note on frequency

Rarer than it looks on Anthropic, where `chain_on_max_tokens` defaults to true
and `should_chain` (`crates/jp_llm/src/provider/anthropic.rs:447-451`) swallows
a `MaxTokens` finish to issue a continuation request.
Chaining is Anthropic-only, so `openai`, `google`, `cerebras`, `llamacpp`, and
`openrouter` reach this path normally, as does Anthropic once `max_tokens` is
set explicitly.

## Severity

Hidden.
The dropped action is invisible in stdout, in the exit status, and in the
conversation.
