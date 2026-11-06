# Token-count reasoning efforts are ignored on SGLang servers

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-28
- **Label**: domain=llm
- **Label**: package=jp_llm
- **Label**: type=bug

The `vllm` provider sends a token-count reasoning effort (for example
`reasoning.effort = 2048`) as `thinking_token_budget`, which is vLLM's field.
SGLang does not read it, so on an SGLang server, or a router that forwards to
one, the budget has no effect.

SGLang documents [two mechanisms][sglang-budget]:

- A hint, `chat_template_kwargs.thinking_budget`, which only works when the
  model's chat template declares that variable (Qwen3's does).
- A hard cap, `custom_params: {"thinking_budget": n}`, which needs the server
  started with `--enable-strict-thinking` (or a custom logit processor with
  `--enable-custom-logit-processor`).

## Observed

Against `deepseek-v4.1-flash` behind the DCD router (SGLang), a top-level
`thinking_budget: 32` produced 219 characters of reasoning that ended naturally,
about the same as with no budget.
SGLang does not document that top-level field, so the run only shows that form
is ignored.
It says nothing about whether the documented forms work on that deployment.

## Next

1. Run SGLang's verification on the deployment: the same prompt at temperature
   0, with and without a budget, for `chat_template_kwargs.thinking_budget` and
   for `custom_params.thinking_budget`.
2. Check that SGLang without strict thinking does not reject `custom_params`,
   and that vLLM ignores it (vLLM logs and drops request fields it does not
   know).
3. If both hold, send the budget under every name, as commit `b19f47ddd` does
   for the thinking switch.

[sglang-budget]: https://docs.sglang.io/docs/advanced_features/thinking_budget
