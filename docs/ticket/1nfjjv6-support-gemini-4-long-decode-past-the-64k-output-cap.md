# Support Gemini 4 long decode past the 64K output cap

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_llm
- **Label**: type=enhancement

Gemini 4 models (`gemini-4-carbon`, `-argon`, `-neon`, `-hydrogen`) can generate
up to 1,048,576 output tokens minus the input, but a single
`streamGenerateContent` request stops at 65,536.
Past that, Google expects the client to run a continuation loop:
<https://ai.google.dev/gemini-api/docs/long-decode?generate-content=true>

- Set `maxOutputTokens` above 65,536.
  It caps thinking and output across all hops.
- A hop that hits the per-request cap ends with `finishReason: CONTINUATION` and
  a `continuationToken`.
- The next hop re-sends the *same* request (same `contents`, same
  `maxOutputTokens`) plus the latest token.
  Partial output is not appended to `contents`.
- The loop stops on any other finish reason.
  The parts from all hops are joined into one model turn.
- Streaming hops past the first 64K send a checkpoint token every 5,000 tokens.
  A dropped stream resumes from the last checkpoint, discarding uncommitted
  chunks.
- A response that ends with `MAX_TOKENS` also carries a token and can be resumed
  with a higher `maxOutputTokens`.

## What JP does today

`create_request` sends `parameters.max_tokens`, falling back to the model's
reported output limit (65,536 for the Gemini 4 models).
With the default, output stops at 64K and the turn ends with the max-tokens
notice.

With `max_tokens` set above 65,536 it is worse: the forked `gemini_client_rs`
(`c072e4b`) has no `Continuation` variant in `types::FinishReason` and no
catch-all, so a `CONTINUATION` finish reason fails to deserialize and the stream
errors instead of ending.
`Candidate` also has no `continuation_token` field, and `GenerateContentRequest`
has no `continuationToken`.

The existing max-token chaining does not fit.
Anthropic's `chain_on_max_tokens` appends the partial output and asks the model
to continue, which is the opposite of Google's rule.
`jp_llm::stream::chain::EventChain` merges overlapping text across chained
streams; Google's hops continue exactly where the previous one stopped, so there
is no overlap to find.

## Usage reporting

Each hop reports its own `usageMetadata`, and every hop's `promptTokenCount`
includes all earlier hops' output.
Summing them overstates the context size: Google's example is a 1,000-token
prompt reporting about 397,000 input tokens over four hops.
Anything that reads usage for compaction or window fitting (T-0n0pex2,
T-0n0pgw6) has to take the first hop's `promptTokenCount` as the prompt size.

## Relation to the Interactions API

On the Interactions API the server runs the loop when `background` is `true`.
If T-1fev134 moves the provider there, this becomes a matter of setting
`background` and polling, and the `generateContent` loop is not needed.
Decide that first.

- [ ] Add `Continuation` (and a catch-all) to `FinishReason`,
      `continuation_token` to `Candidate`, and `continuation_token` to the
      request in the fork.
- [ ] Map `CONTINUATION` to a finish reason that is not an error.
- [ ] Run the hop loop in the Google provider, joining hops into one response.
- [ ] Resume a dropped stream from the last checkpoint token.
- [ ] Record a cassette for a response past 64K.
