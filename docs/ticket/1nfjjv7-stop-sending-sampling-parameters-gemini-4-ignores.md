# Stop sending sampling parameters Gemini 4 ignores

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=google
- **Label**: package=jp_llm
- **Label**: type=enhancement

Gemini 4 models ignore `temperature`, `top_p`, and `top_k` when they are in
range, and reject out-of-range values with a 400.
Google's migration notes say to remove them from requests:
<https://ai.google.dev/gemini-api/docs/whats-new-gemini-4-neon#migrate-from-gemini-3>

`create_request` in `crates/jp_llm/src/provider/google.rs` copies
`assistant.model.parameters.temperature`, `top_p`, and `top_k` into
`GenerationConfig` for every model.
On a Gemini 4 model, a user who sets `temperature = 0.2` gets the server's
default sampling and no sign that the setting had no effect.

The fix is to leave the three fields out for models that ignore them, and warn
once when a configured value is dropped, naming the model.
That needs a way to know which models those are: either a fact in the Google
`MODEL_OVERRIDES` catalog, or a rule on the Gemini 4 family.
The catalog keeps unknown, newer models sending what the user configured, which
matches how unknown models are treated elsewhere.

This is the opposite gap from T-0px2158, where configured parameters never reach
providers that would use them.

- [ ] Mark the Gemini 4 catalog entries as not taking sampling parameters.
- [ ] Leave `temperature`, `top_p`, and `top_k` out of their requests, with a
      warning when a configured value is dropped.
- [ ] Test that a Gemini 4 request carries none of the three, and that a model
      outside the catalog still does.
