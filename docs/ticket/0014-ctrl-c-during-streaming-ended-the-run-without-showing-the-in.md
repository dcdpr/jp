# T0014: Ctrl-C during streaming ended the run without showing the interrupt menu

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-11

A single `^C` during an Opus reasoning stream ended the turn immediately.
No interrupt menu appeared, no message was printed, and the process exited 0.
Not reproduced since.

## What the trace log shows

Thinking deltas were still arriving 1.6s before the signal, so the stream was
live:

```json
{
  "timestamp": "2026-08-11T10:23:49.150307Z",
  "level": "TRACE",
  "fields": {
    "message": "Received event from Anthropic API.",
    "event": "...thinking_delta..."
  }
}
{
  "timestamp": "2026-08-11T10:23:50.788483Z",
  "level": "INFO",
  "fields": {
    "message": "Signal received.",
    "signal": "SIGINT"
  }
}
{
  "timestamp": "2026-08-11T10:23:50.788522Z",
  "level": "DEBUG",
  "fields": {
    "message": "Routed OS signal.",
    "signal": "Interrupt",
    "routed": "Handler"
  }
}
{
  "timestamp": "2026-08-11T10:23:50.788671Z",
  "level": "INFO",
  "fields": {
    "message": "Interrupt received during streaming."
  }
}
{
  "timestamp": "2026-08-11T10:23:50.805940Z",
  "level": "INFO",
  "fields": {
    "message": "Flushed conversation to disk.",
    "id": "jp-c17860012287"
  }
}
{
  "timestamp": "2026-08-11T10:23:50.849953Z",
  "level": "DEBUG",
  "fields": {
    "message": "RunningService dropped..."
  }
}
```

The router picked the right handler and `handle_streaming_interrupt`
(`crates/jp_cli/src/cmd/query/interrupt/signals.rs:70`) ran. 17ms later the
conversation was flushed and teardown began, so
`InterruptHandler::handle_streaming_interrupt` returned a decision without ever
blocking for input.

On the terminal, the buffered reasoning appeared right after the echoed `^C`
(consistent with the `flush_renderer()` + `flush_instant()` at the top of that
function) and nothing else.
No menu, no `Interrupted`.

## The evidence does not add up

Two ways out of that function in 17ms, and neither fits cleanly.

`config.action != Prompt` (`handler.rs:192`) skips `inline_select` entirely and
hardcodes the choice.
`stop` and `abort` both end the turn with `Ok(())`, which matches the exit
status.
But the action was not set anywhere: not in `~/Library/Application
Support/jp/config.toml`, not in the user-global `config/` directory, not in a
user-workspace config (no such file for workspace `otvo8`), not in
`JP_CFG_INTERRUPT_*`, and not as a `config_delta` on the conversation.
So it should have resolved to the `Prompt` default.

`inline_select` returning `Err` maps to `InterruptAction::Escalate`
(`handler.rs:215`), which cancels the shutdown token and returns
`cmd::Error::interrupted()`.
That propagates unchanged through `query.rs:942` and `run_inner`'s select, so it
should have printed `Interrupted` on stderr and exited 130.
It did neither.

One of those two readings has to give.
The likeliest weak link is the exit status, which was recalled from a shell
prompt rather than read from `$?`.
If it was really 130, this is `inline_select` failing instantly and the question
becomes why.

## Why it can't be timing

The menu decision does not consult stream state.
`stream_alive` is only used inside the `'c'` branch to pick `Resume` over
`Continue`; it never suppresses the menu.
A `^C` landing between the thinking block and the first text delta, or after the
stream died, still prompts.

## Next time

`Err(_)` at `handler.rs:215` and `handler.rs:296` discards the `InquireError`,
which is exactly the value that would have settled this.
Same for the `Ok(ReplyOutcome::Cancelled) | Err(_)` arm in
`collect_reply_inline`.
Nothing logs the resolved `interrupt.streaming.action` either, so ruling the
config in or out took five greps across four layers instead of one grep in the
trace log.

Adding that logging is the immediate work; the bug itself stays open until it
reproduces with the extra data.

## Related

RFD 060 (Config Explain) would have answered the config half directly.

## Comments

-----

- **From**: jp
- **Date**: 2026-08-11T11:15:05Z

Logging is in place.

`handler.rs` now logs the resolved action (`Handling streaming interrupt.` with
`action` and `menu`), and `log_prompt_failure` records the `InquireError` that
the menu call sites previously threw away as `Err(_)`.

`signals.rs` logs the chosen `InterruptAction` and the returned result.

Still open until it reproduces.

-----

- **From**: jp
- **Date**: 2026-08-17T16:35:48Z

Fixed one proven cause of this symptom, though not the evidence recorded above.

The escalation ladder was counting presses it had already answered. `EscalationState::bump` only reset on elapsed time, so answering a menu and interrupting again inside the 2s cooldown counted as press two and routed straight to `Shutdown`, bypassing the handler stack — no menu, no message. A delivered press is now an `InterruptNotice` the handler resolves: `handled()` clears the count, `decline()` and dropping leave it intact. Only unanswered presses escalate.

Also split a user-cancelled menu from one that could not run. The latter returns `PromptFailed` and leaves the press on the ladder rather than escalating on a decision nobody made.

Confirmed live: eight presses, five inside the cooldown of an answered one, all reached the handler.

Staying open because the traces in the description showed `routed=Handler` with a ~20ms return — a different signature from this bug — and those files are gone.
