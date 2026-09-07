# A model refusal discards the answer and reports success

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-07

When a provider ends a stream with `FinishReason::Refused`, JP throws away
everything the model streamed and then exits 0.

`TurnCoordinator::handle_streaming_event`
(`crates/jp_cli/src/cmd/query/turn/coordinator.rs:326-332`) clears the event
builder and pops the already-flushed responses back out of the conversation
stream, per the `Refused` contract that partial output must not be kept.
It then transitions to `Complete`, `turn_loop` returns `Ok(())`, and the run
ends as `RunOutcome::AsExpected`.

So the run produces no answer, records no answer, and tells the caller it
succeeded.
The only statement that anything happened is the chrome line from
`finish_notice` (`coordinator.rs:674`), e.g. `The model declined this request
(cyber): declined for safety`, which is commentary on a successful run by every
mechanical definition JP uses: printed mid-run through the printer, exit status
0, never reaching `parse_error`.

## The question to decide

Is a refusal a failed run?

If it is, the outcome belongs in the error path: return `Err`, let `parse_error`
render it, exit non-zero.
The message then reaches the user through the run's error report, which
`--quiet` deliberately keeps, and no separate channel or exemption is needed for
it.

If it is not, then a run that produces nothing while reporting success needs
some other justification, because a script cannot distinguish it from a model
that legitimately had nothing to say.

## Why it is filed rather than fixed in place

Raised while reviewing PR \#1088, which makes `--quiet` close the chrome
channel.
That PR made the refusal notice suppressible, which prompted the question, but
the defect predates it: the empty answer and the zero exit status are there
whatever `--quiet` does.
Changing `jp query`'s exit status is a contract change for every script around
it, so it wants its own decision rather than riding along.

## Severity

Hidden.
A refusal under `--quiet`, or with stderr redirected, is indistinguishable from
a short successful answer: empty stdout, exit 0, nothing in the conversation.
