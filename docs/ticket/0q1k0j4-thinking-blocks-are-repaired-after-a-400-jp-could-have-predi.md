# Thinking blocks are repaired after a 400 JP could have predicted

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-22
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=enhancement

Anthropic validates replayed `thinking` blocks against the request that would
have produced them.
A request whose `system`, `tools`, or earlier messages changed since the blocks
were generated is rejected with `thinking blocks ... cannot be modified`, and a
block whose signature does not validate for the model being called is rejected
with `Invalid 'signature' in 'thinking' block`.
For accounts created on or after 2026-08-31 the first of these is the default
rather than an opt-in.

JP handles both, in `anthropic.rs`: `classify_thinking_rejection` names the
rejection, `build_thinking_patches` rewrites the whole assistant turn holding
the offending block as `<think>` text, and the stream yields
`FinishReason::Retry` so `turn_loop` reissues the request.
It is tested and it converges.

It is also entirely reactive.
Every repair costs a round trip that ends in a 400, and it pays for that round
trip by discarding that turn's native reasoning — the API keeps the text, but
the model no longer sees the blocks as its own.
On a turn-scoped rejection that is one round and one turn's reasoning.
On a signature rejection spanning a long conversation it is one round per turn
that carries thinking.

## What JP already knows

The conversation's config is an ordered series of deltas in the stream
(`jp_conversation::stream::config_delta`), so the model, the tools, and the
system prompt in force at every point are recoverable by folding forward.
When JP is about to issue a request it can compare the config that produced the
stored thinking blocks against the config it is about to send, and strip the
provider metadata itself when the two disagree in a way the API rejects.
That turns a failed request plus a repair into no failed request at all, and
lets JP say what it dropped and why instead of logging a warning about someone
else's 400.

## Why this surfaces now

Two of JP's own features are the trigger.
A `--cfg` delta applied mid-conversation changes `system` or `tools` between one
turn and the next, which is the exact edit the append-only requirement forbids.
Changing `assistant.model.id` mid-conversation moves the conversation to a model
that may not read the stored blocks at all.

Claude Opus 5.5 makes both more likely: thinking cannot be turned off, so every
assistant turn carries blocks, and its blocks are read only by Opus 5.5, Fable
5.1, and Mythos 5.1.

## To settle first

Whether a cross-model replay is a 400 or a silent drop.
The migration guide's wording — "runs those turns without them" — reads like
the blocks are ignored rather than rejected, which would make the model-switch
half of this a silent context loss rather than a failed request.
If so it still deserves a notice, but it is a different fix from the
tools-and-system half, and possibly a different ticket.

## Severity

Low and self-correcting today.
The repair works, and the user sees a slower turn rather than a broken one.
It gets worse the longer a conversation runs and the more often its config
changes, and it is silent about the reasoning it discards to get unstuck.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-29T18:58:28Z

Claude Sonnet 5.5 joins Fable 5.1 and Opus 5.5 under the same prefix check, and
its docs settle the open question above: a replayed block the current model
can't read is **dropped silently**, not rejected.
The request succeeds and the model runs that turn without the reasoning.
Only a prefix edit (`system`, `tools`, or an earlier message) produces the 400,
and its message (`Invalid 'signature' in 'thinking' block. The block is bound to
a different conversation...`) is one `classify_thinking_rejection` already
matches as `InvalidSignature`.

Sonnet 5.5 narrows the options for fixing this:

- The documented mitigation is the `thinking-binding-controls-2026-08-01` beta
  plus `thinking.block_binding.prefix_mismatch_behavior = "drop_block"`.
  On Sonnet 5.5 that field is accepted only with `thinking: adaptive`.
  JP sends `thinking: between_tools` there when `reasoning = off`, and sending
  `block_binding` with it is a 400.
  So with reasoning off, the only fixes are an append-only history or JP
  stripping the stale blocks itself, which is the proactive approach this ticket
  proposes.
- Sonnet 5.5 blocks are also bound to the account that produced them.
  Blocks replayed through a different credential are dropped silently
  (`organization_binding_mismatch`).
  JP's credential fallback can move a conversation between accounts, so that is
  a second way reasoning gets lost without any notice.

Source: https://platform.claude.com/docs/en/build-with-claude/preserved-thinking
