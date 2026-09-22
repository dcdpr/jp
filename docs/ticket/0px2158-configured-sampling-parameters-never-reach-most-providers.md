# Configured sampling parameters never reach most providers

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-22
- **Label**: domain=llm
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=bug

`assistant.model.parameters` accepts settings that no request builder reads.
A user writes them in `config.toml`, sees them in the generated file and in the
JSON schema, and the request goes out with the server's defaults.
Nothing reports the gap: the query succeeds and the answer looks plausible.

Three separate gaps, in order of how clear the fix is.

## `stop_words` reaches no provider at all

`ParametersConfig::stop_words` is declared in `jp_config::model::parameters`,
carries an `append_vec` merge strategy, has delta and fill coverage, and appears
in four places in the config snapshots.
Grepping `crates/jp_llm/src` for `stop_words` returns nothing: every hit in the
workspace is in `jp_config` or its tests.

Every provider JP speaks to accepts a stop sequence list — `stop` for the
OpenAI-compatible dialects, `stop_sequences` for Anthropic, `stopSequences` for
Google, `options.stop` for Ollama.
Either the field is wired to each of them, or it should be removed from
`ParametersConfig` rather than left as configuration that does nothing.

## `top_k` reaches three providers of nine

Forwarded by `anthropic`, `google`, `ollama`, and `vllm`.
Not forwarded by `openai`, `openrouter`, `cerebras`, or `llamacpp`.

llama.cpp and vLLM serve the same dialect and the same locally-hosted models, so
the split between them is the odd one: a Qwen3 deployment documents a `top_k`
alongside its `top_p`, and which of the two servers is running decides whether
it applies.

## `parameters.other` is documented as a passthrough but is not one

The doc comment reads "Parameters JP does not model are collected into
[`Self::other`], so a provider-specific key can be written directly in the
parameter block."
No provider forwards the map.
Each reads named keys out of it: `verbosity` and `reasoning_mode` (`openai.rs`),
`keep_alive` (`ollama.rs`), `context_management` (`anthropic.rs`).
Anything else a user writes there is dropped in silence.

This one carries a decision rather than a fix.
A generic passthrough sends a user's typo to the server as an unknown field,
which some servers reject and others ignore; a curated allowlist means every
provider-specific key costs code and the documented promise has to be narrowed
to match.
Settle that before touching the other two, since it decides whether `min_p` and
friends are a provider's job or the map's.

## Notes

Found while reviewing #1174, which added the `vllm` provider.
`top_k` was wired there as part of that review; the rest is untouched and
predates it.
