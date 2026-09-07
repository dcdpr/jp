# `--quiet` hides the only notice that a turn ended abnormally

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-07
- **Label**: client=cli
- **Label**: package=jp\_cli
- **Label**: type=bug

`-q` closes the chrome channel, and the notice that a turn ended on a
non-standard finish reason travels on it.
So a quiet run that was truncated, refused, or stopped early says nothing about
it, and nothing else records the fact either.

## The path

`TurnCoordinator::handle_streaming_event`
(`crates/jp_cli/src/cmd/query/turn/coordinator.rs:351-352`) emits the notice
with `printer.eprintln`.
`Printer::send` drops the task before queueing it when the chrome channel is
silenced, so nothing is written.

Nothing recovers the fact afterwards: `transition_from_streaming` takes the
reason as `_reason` (`coordinator.rs:364-367`) and ignores it, `FinishReason`
appears nowhere in `jp_conversation`, and the names of tool calls discarded
mid-build are harvested from buffers that are then dropped
(`coordinator.rs:322-324`).

## The input that reaches it

An explicitly configured `assistant.model.parameters.max_tokens` disables
Anthropic's request chaining — `chain_on_max_tokens` is `!is_structured &&
max_tokens_config.is_none() && self.chain_on_max_tokens`
(`crates/jp_llm/src/provider/anthropic.rs:194-195`), and `chains_remaining` is
`0` when that is false, which `should_chain` requires to be non-zero.
The config docs recommend setting `max_tokens` for exactly this purpose
("tighter cost controls", `crates/jp_config/src/model/parameters.rs:38-42`).

Chaining is Anthropic-only, so `openai`, `google`, `cerebras`, `llamacpp`, and
`openrouter` reach the notice on a plain max-tokens finish regardless of
configuration.

## The two positions

**Leave it.** The notice is commentary by every mechanical definition JP uses:
emitted mid-run through the printer, the coordinator transitions to `Complete`,
`turn_loop` returns `Ok(())`, the outcome is `RunOutcome::AsExpected`, exit 0.
It never reaches `parse_error`.
An error message is what JP prints when it did not produce a result; this prints
when it did.
If the information is load-bearing enough to survive `-q`, then the run is not
succeeding, and the fix belongs to the outcome rather than the channel — at
which point it flows through the error report, which `-q` already keeps.

**Exempt it.** Returning `Ok(())` describes the implementation rather than
justifying it, so the current classification cannot be its own defence.
A caller gets a truncated answer, or one missing a tool call that was supposed
to run, and is told the run succeeded.
Suppressing the notice is new behaviour introduced by [PR \#1088]; before it,
the line always printed, and no replacement signal exists yet.

## Where the design lives

Not a fourth output channel.
[D15] already lists graduated `-q` levels as future work (lines 223-226), and
[D32] frames `-v`/`-vv`/`-vvv` as chrome *verbosity* (lines 124-129), so
"suppress progress, keep notices" is a severity floor on the existing chrome
channel rather than a new category.

If this turns into a disagreement about approach rather than a decision, it
belongs in D15 rather than in this ticket's comments.

## Cheapest resolution

[T-0eyhkvs] proposes persisting the finish reason and the discarded tool call
names.
That removes the reason this question exists: once the fact has a durable home,
the notice stops being its only witness and `-q` can keep a rule with no
exceptions.

## Severity

Hidden, and deliberately accepted for now.
A truncated answer under `-q` is indistinguishable from a complete one: exit 0,
no notice, and no record in the conversation.
A tool call dropped mid-build is worse, because the prose that survives reads as
if the assistant chose not to act.

[D15]: ../rfd/drafts/D15-structured-logging-infrastructure.md
[D32]: ../rfd/drafts/D32-jp-tracing-infrastructure.md
[PR \#1088]: https://github.com/dcdpr/jp/pull/1088
[T-0eyhkvs]: 0eyhkvs-a-tool-call-truncated-by-max_tokens-is-discarded-without-a-r.md
