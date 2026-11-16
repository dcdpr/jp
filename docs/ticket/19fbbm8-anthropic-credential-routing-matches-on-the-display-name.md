# Anthropic credential routing matches on the display name

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-27
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

The Anthropic provider resolves credentials with `model.name()`, which returns
`display_name` when one is set rather than the served model id.
The call sites are `start_query`, `chat_completion_stream`, and both `advance`
and `advance_for_rebuild` in the stream
(`crates/jp_llm/src/provider/anthropic.rs`, around lines 338, 435, 533, 611,
614).

That string reaches `scope_covers` in `crates/jp_credentials/src/store.rs`,
which checks `model.contains(family)` case-sensitively.
`map_model` sets `display_name` from the API listing for every listed model, so
the string compared is something like "Claude Opus 4.5", and `"Claude Opus
4.5".contains("opus")` is false.
A family-scoped cooldown (`seven_day_opus`) is therefore missed: a profile whose
Opus window is spent is tried again instead of the chain falling through to the
next credential.
Account-wide cooldowns are unaffected, since they cover every model.

I have not confirmed the exact display-name string the API returns; the code
path is unambiguous either way.

## Fix

Pass `model.id.name` to credential resolution and fallback at those call sites,
and keep `name()` for presentation and logging.

## Test

Record a `seven_day_opus` cooldown on a profile, resolve a model whose details
carry a display name such as "Claude Opus 4.5", and assert the profile is
skipped.

Found while reviewing RFD 125.
