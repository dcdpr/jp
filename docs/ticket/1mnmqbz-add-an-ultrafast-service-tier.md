# Add an Ultrafast service tier

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=llm
- **Label**: llm-provider=openai
- **Label**: package=jp_config
- **Label**: package=jp_llm
- **Label**: type=feature

OpenAI sells an Ultrafast tier, selected with `service_tier: "ultrafast"` in the
Responses API.
`ServiceTier` in `jp_config` has `off`, `flex`, `standard`, and `priority`, so
JP cannot ask for it.

What is published:

- Models: GPT-6 Astra, GPT-6.1 Sol, GPT-6.2 Astra, GPT-6.2 Sol.
- US data residency only.
- API price for 6.2: 3x Standard through 2027-02-28, 6x from 2027-03-01.
- ChatGPT plans: Pro $500 on both 6.2 models, Pro $200 on 6.2 Sol only. 6x
  included usage on 6.2.

Sources: <https://developers.openai.com/api/docs/guides/ultrafast-mode>,
<https://learn.chatgpt.com/docs/pricing>.

Fast mode needs nothing: OpenAI accepts `priority` and `fast` interchangeably,
and JP sends `priority`.

## To settle

- Whether the subscription host accepts `service_tier` at all.
- What other providers do with `ultrafast`.
  Anthropic's `flex` precedent is a `UnsupportedServiceTier` error; see
  T-0g1w1mw for why tiers must not be reused for different interaction models.
- Whether models outside the list above reject it, and whether the catalog
  should mark support with a feature flag.
- Whether the `openai_responses` fork's `types::ServiceTier` has the variant.
