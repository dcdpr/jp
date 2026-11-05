# llama.cpp switches thinking off when reasoning is unset

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=llm
- **Label**: llm-provider=llamacpp
- **Label**: package=jp_llm
- **Label**: type=bug

With `assistant.model.parameters.reasoning` unset, the llama.cpp provider sends
`chat_template_kwargs: {"enable_thinking": false, "thinking": false}`
(`create_request` in `crates/jp_llm/src/provider/llamacpp.rs`, around line 232).
Request kwargs take precedence over the template's own default and over the
server's `--chat-template-kwargs`, so a model whose template thinks by default
(Qwen3, DeepSeek V3.1 and later) stops thinking without anyone having asked it
to, and the operator's server-side setting has no effect.

The vLLM provider had the same bug, fixed in `c3360b39a` ("Leave thinking to the
template when unset").

## Fix

Send `chat_template_kwargs` only when reasoning is configured: both kwargs
`false` for `off`, both `true` for any effort.
With reasoning unset, send neither, so the template and the server decide.
`reasoning_format: "deepseek"` stays, since it only says how the server
separates reasoning from the answer.

## Test

Request-body tests in `llamacpp_tests.rs`: no `chat_template_kwargs` with
reasoning unset; both `false` for `off`; both `true` for an effort.
The recorded llama.cpp cassettes and compaction snapshots need the kwargs
removed from requests that leave reasoning unset.
