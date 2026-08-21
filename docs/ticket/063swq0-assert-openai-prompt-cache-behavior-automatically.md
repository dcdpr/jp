# Assert OpenAI prompt-cache behavior automatically

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-21

`test_gpt_5_6_prompt_cache_read_after_write` asks the person re-recording fixtures to inspect usage fields manually. The cassette currently contains nonzero cache writes followed by nonzero cached tokens, but the test cannot fail if either disappears because provider usage is discarded.

Acceptance criteria:

- Capture the relevant cache usage from recorded Responses API events in the test path.
- Assert nonzero cache writes on the first request and nonzero cached tokens on the second.
- Assert the stable prompt-cache key and expected breakpoints on both requests.
- Keep usage out of the persisted conversation unless product behavior requires it; a test-only observer is sufficient.
- Add a negative harness test proving zero cache activity fails the cache test.
