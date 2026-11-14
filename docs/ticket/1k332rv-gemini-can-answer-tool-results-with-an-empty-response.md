# Gemini can answer tool results with an empty response

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-14
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

After receiving tool results, Gemini sometimes ends its response without
content: a candidate with `"content": {"role": "model"}` and no `parts`,
`finishReason: "STOP"`, and no `candidatesTokenCount`.
The turn then ends with no answer to the user.

The recorded provider suite shows it on `gemini-2.5-flash-lite` in two committed
cassettes, as the second response:

- `crates/jp_llm/tests/fixtures/google/test_tool_call_stream.yml`
- `crates/jp_llm/tests/fixtures/google/test_tool_call_without_parameters.yml`

It also happened in the first recordings of `test_tool_call_error` and
`test_tool_call_nested_arguments`.
Both answered once their prompt asked for a report on the result ("then tell me
how it went"), so the prompt makes it less likely but does not prevent it.
Every other provider answered the same exchanges.

## Current behavior

- The Google provider maps the empty candidate to `Finished(Completed)` with no
  events.
- `jp query` prints "The model finished without responding."
  (`TurnCoordinator`, via `current_cycle_has_answer`) and ends the turn.
  Nothing is retried.
- `jp serve chat` sends the text written before the last tool call if there was
  any, and "(No answer.)" otherwise (`last_answer`).
  The planner persona forbids text before tool calls, so a Gemini-backed bridge
  would reply "(No answer.)".

## Options

1. **Retry in the Google provider.** Treat an empty `STOP` response to tool
   results as `FinishReason::Retry`, bounded.
   The provider already retries `UNEXPECTED_TOOL_CALL` up to three times
   (`google.rs`), so a Gemini quirk stays in the Google provider.
2. **Retry in the turn loop.** Re-send once when any provider completes with no
   answer to tool results.
   Covers every provider, but no other provider has shown the problem.
3. **Nudge.** Append a synthetic request asking the model to respond to the
   results.
   This changes the stored conversation, so it is the least attractive.

Option 1 is the narrowest.
Unverified: whether re-sending the identical request gets an answer, or whether
Gemini repeats the empty response.
Re-recording the two cassettes above with the retry in place answers that.

- [ ] Map an empty `STOP` candidate to tool results to a bounded retry in the
      Google provider, with a unit test on the mapped events
- [ ] Keep the "The model finished without responding." notice for the case
      where every retry comes back empty
- [ ] Re-record `google::test_tool_call_stream` and
      `google::test_tool_call_without_parameters` and check that they end in a
      message
