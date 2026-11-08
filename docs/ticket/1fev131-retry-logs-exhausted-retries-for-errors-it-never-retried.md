# Retry logs "exhausted retries" for errors it never retried

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=bug

`collect_with_retry` in `crates/jp_llm/src/retry.rs` logs `Stream error
(exhausted retries).` for every error that ends its loop, including a
non-retryable error on the first attempt.

Observed on a title request over the Claude ACP route: the error was
`StreamErrorKind::Other` (not retryable), and the log read `attempt=1 max=3
"Stream error (exhausted retries)."`.
That sends a reader looking for three failed attempts that never happened.

`handle_stream_error` in `jp_cli::cmd::query::stream::retry` has the same
problem in a milder form: it logs one message, `Stream error (not retryable or
max retries exceeded)`, for both cases.

- [ ] Log a non-retryable error and a spent retry budget as two distinct
      messages in `collect_with_retry`
- [ ] Do the same in `handle_stream_error`
