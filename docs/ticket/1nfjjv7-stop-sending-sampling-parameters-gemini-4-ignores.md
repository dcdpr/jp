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

The fix is to omit the three fields for models that ignore them, and warn once
when a configured value is dropped, naming the model.
To identify those models, either record a capability flag in the Google
`MODEL_OVERRIDES` catalog or match on the Gemini 4 model family.
Using the catalog preserves configured sampling parameters for unknown or newer
models, matching how unknown models are handled elsewhere.

This is the inverse of T-0px2158, where configured parameters are not forwarded
to providers that support them.

- [ ] Mark the Gemini 4 catalog entries as not accepting sampling parameters.
- [ ] Omit `temperature`, `top_p`, and `top_k` from their requests, logging a
      warning when a configured value is dropped.
- [ ] Test that a Gemini 4 request omits all three parameters, and that a model
      outside the catalog still sends them.
