# Add a vLLM provider

- **Status**: Done
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-16
- **Label**: domain=llm
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=feature

vLLM serves `GET /v1/models` and `POST /v1/chat/completions` behind a Bearer
token, speaking the same Chat Completions dialect that
`jp_llm::provider::openai_compat` already parses for llama.cpp.
A vLLM provider is therefore mostly config plumbing plus a thin provider module
over the shared dialect code.

## Config

- `ProviderId::Vllm` in `jp_config::model::id`, with `as_str() == "vllm"`.
- `providers/llm/vllm.rs` holding `VllmConfig`: `api_key_env` defaulting to
  `VLLM_API_KEY`, and `base_url` defaulting to `http://127.0.0.1:8000`.
  The four trait impls (`AssignKeyValue`, `PartialConfigDelta`, `FillDefaults`,
  `ToPartial`) follow `deepseek.rs`.
- A `vllm` field on `LlmProviderConfig`, wired into each of the four impls in
  `providers/llm.rs`.
- The `jp_config` snapshots for config fields, schema shape, and partial
  defaults all move; review them with `cargo insta`.

## Provider

- Hoist `to_system_messages`, `convert_events`, `convert_tools`, and
  `convert_tool_choice` out of `llamacpp.rs` into `openai_compat.rs` as
  `pub(crate)`, and have llamacpp call them there.
  Behavior-preserving.
- `provider/vllm.rs` with `Vllm { client, base_url }`.
  `TryFrom<&VllmConfig>` reads the key from the environment and sets the Bearer
  header, as `cerebras.rs` does.
- `models()` maps `GET /v1/models` entries to `ModelDetails`, taking
  `context_window` from the reported `max_model_len` and keeping the full id
  (e.g. `Qwen/Qwen3-8B`) as the name.
  `model_details()` returns `ModelDetails::empty()` for an unknown name.
- `create_request()` builds the chat body: model, messages, stream, temperature,
  top_p, max_tokens, tools, tool_choice, `response_format` for a structured
  schema, and `chat_template_kwargs.enable_thinking` from the reasoning setting.
  No `reasoning_format` field — that one is llama.cpp-specific.
  `chat_completion_stream()` posts it and parses the SSE stream with
  `parse_chunk()`.
- `mod vllm`, the `get_provider()` and `build_request_value()` arms in
  `provider.rs`, and the `Vllm` arms in `test.rs` for `base_url` and
  `api_key_env`.
- `vllm_tests.rs` covering `create_request()` for a plain message, a tool call
  round trip, a structured schema, and reasoning off, each against a static
  expected JSON body.

## Docs

Name vLLM in the provider sentence in `docs/features/tools.md`.

## Comments

-----

- **From**: jp
- **Date**: 2026-09-16T02:01:11Z

Filed after the fact: the config, provider, and docs work described above is
implemented and staged.
Deviations from the original plan worth recording:

- The plan also called for adding vLLM to a provider list in
  `docs/configuration.md`.
  That file has no provider list, so there is nothing to add there.
- The plan's dotfiles step (the `nebius` alias and the `[providers.llm.vllm]`
  table in `agentic-shepherd/dotfiles/jp-user-config/user-config.toml`) lives in
  another repository and is out of scope here.
