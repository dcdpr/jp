# Evaluate moving the Google provider to the Interactions API

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_llm
- **Label**: type=pre-rfd

Google calls `generateContent` legacy and recommends the Interactions API (`POST
/v1beta/interactions`, GA June 2026) for new work.
It has the same shape as the Responses API JP already uses for OpenAI: typed
steps, SSE events, and `store: false` for client-held history.

## What it would and would not fix

Measured on 2026-11-07/08 against the Gemini API with `curl`:

- **No early sign of life.** On `streamGenerateContent`, a 330k-token request
  was silent for 111s, then the HTTP headers and first data arrived 90µs apart.
  On Interactions, `interaction.created` arrives in the same burst as the first
  output (12ms before the first thought summary), not when the request is
  accepted.
  With `store: false` it carries no `id` or `status`.
- **Function calls are not streamed.** On `gemini-4-argon`, a 9.4k-token
  `write_note` argument arrived as a single `arguments_delta` after 139s of
  silence, despite the docs describing `arguments_delta` as incremental.
  `streamGenerateContent` also delivers a call whole, and the Gemini API
  silently ignores `streamFunctionCallArguments` (Vertex-only).
- **Text and thought summaries do stream**, on both APIs, a delta every 0.3–3s.

So a migration would not have prevented the idle timeouts on large Gemini
requests.
Those are handled by `providers.llm.google.silence_allowance_secs`, with
`stream_idle_timeout_secs` doubling on each timeout retry as a fallback.

## Reasons to move anyway

- Typed error codes (`rate_limit_exceeded` vs `quota_exceeded`,
  `deadline_exceeded`, `safety`, `malformed_function_call`,
  `missing_thought_signature`) in place of matching message text.
- Thought signatures travel as `thought` steps resent verbatim, and the backend
  handles a model switch, which could retire some of the signature repair in
  `google.rs`.
- One `thinking_level` across model families instead of budget-or-level
  branching.
- `incomplete` and `requires_continuation` statuses, the latter for Gemini 4
  output past 64K tokens.
- New models and capabilities launch there first.

## Costs

- The schema still moves: May 2026 replaced `outputs` with `steps`, renamed
  every streaming event, and sunset the old schema a month later, under an
  `Api-Revision` header on `v1beta`.
- No explicit caching or Batch API on Interactions.
- No served `model_version` in the response.
- A safety block is an HTTP 400 or a mid-stream `error` event rather than a
  finish reason; what happens to text already streamed is undocumented.
- `temperature` and `top_p` are absent from the reference; Gemini 3.6+ ignores
  them on both APIs.
- `store` defaults to `true`, so every request must set `store: false`.
