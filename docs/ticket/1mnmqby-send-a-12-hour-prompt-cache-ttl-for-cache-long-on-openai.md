# Send a 12-hour prompt cache TTL for `cache = long` on OpenAI

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=llm
- **Label**: llm-provider=openai
- **Label**: package=jp_llm
- **Label**: type=bug

`assistant.request.cache = "long"` has no effect on OpenAI models with explicit
prompt caching (GPT-5.6 and later, `EXPLICIT_PROMPT_CACHING` in the catalog).
`create_request` only sends `prompt_cache_options` to turn caching off, and
always with `ttl: None`, so every request gets the 30-minute default.

On these models `prompt_cache_options.ttl` accepts `"30m"` (default, writes at
1.25x input) and `"12h"` (writes at 2.5x input).
Unsupported values are a 400.
Source:
<https://developers.openai.com/api/docs/guides/prompt-caching#gpt-56-and-later-1>.

## Fix

- `CachePolicy::Long` on an `EXPLICIT_PROMPT_CACHING` model sends
  `prompt_cache_options: { ttl: "12h" }`.
- `CachePolicy::Short` keeps omitting the field.
- Check whether the `openai_responses` fork's `PromptCacheOptions::ttl` can
  express `"12h"`.

## Related, possibly out of scope

Models before GPT-5.6 use `prompt_cache_retention` instead (`"in_memory"` or
`"24h"`, with `"24h"` the only value on GPT-5.5).
JP sends neither, so `long` is also ignored there.
Decide whether to cover them here or leave them, since those models are on their
way out.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-17T21:55:19Z

Fixed.
On models with `EXPLICIT_PROMPT_CACHING`, `prompt_cache_options(CachePolicy)` in
`openai.rs` sends:

- `off`: `{ "mode": "explicit" }`, as before.
- `short`: nothing, leaving the 30-minute default.
- `long`: `{ "ttl": "12h" }`.
- a custom duration: rounded to the nearer TTL, so 6h15m or longer sends `12h`.
  This matches how the Anthropic provider rounds between its two TTLs.

The fork's `PromptCacheOptions::ttl` is a plain `Option<String>`, so the fork
needed no change.

Out of scope: `prompt_cache_retention` on models before GPT-5.6.
The fork's `Request` has no field for it, and those models are being retired.
`long` stays without effect there.
