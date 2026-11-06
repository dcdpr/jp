# Unrecognised 404 bodies hide an unknown model

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=llm
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: type=bug

JP recognises a refused model only from a 404 whose body carries a known marker
(`looks_like_unknown_model` in `crates/jp_llm/src/error.rs`): `"code":
"model_not_found"` or `"model_archived"`, `"param": "model"`, or a vLLM
`NotFoundError` saying the model does not exist, plus Anthropic's
`not_found_error`.
A server with its own body gets a generic error instead of the unknown-model
error with the served models, and an interactive turn gets no picker.

Synthetic answers a retired or misspelled model on both of its routes with a
flat body:

```json
{"error": "hf:nobody/no-such-model is no longer supported. Try using a different model, like hf:moonshotai/Kimi-K3"}
```

## Fix

Preserve the 404 status on an unrecognised 404 response, and promote it to
`UnknownModel` when the provider's model listing is reachable and does not
contain the requested model.
The CLI already fetches the listing for the error and the picker
(`with_served_models` in `crates/jp_cli/src/cmd/query/turn_loop.rs`).
A wrong `base_url` fails the listing too, so it keeps the generic error.

## Test

A provider whose stream yields a 404 with an unrecognised body: with a listing
that omits the model, the turn ends with `Error::UnknownModel` carrying the
listing; with a failing listing, it ends with the original error.
