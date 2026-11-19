# Record the Anthropic API suite against Claude Haiku 5.5

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=task

The Anthropic API route in
`crates/jp_llm/src/provider/anthropic/test_support.rs` records against
`claude-haiku-4-5`, a budget-era model.
Every Anthropic model tier has a 5.x release, and the catalog describes
`claude-haiku-5-5` as adaptive and accepting `thinking: disabled`.

Move the API route to `claude-haiku-5-5`, taking its details from the catalog
rather than a hand-written `ModelDetails`, and re-record every Anthropic API
cassette.

`SubscriptionTestRoute::model()` delegates to the API route's model.
Pin `SubscriptionTestRoute::model()` to `claude-haiku-4-5` because its cassettes
cannot be re-recorded until T-1ngnv8p adds an ACP recorder for the subscription
route.

- [ ] Move the API route to `claude-haiku-5-5`.
- [ ] Pin the subscription route to `claude-haiku-4-5`, referencing T-1ngnv8p.
- [ ] Re-record `provider::tests::anthropic::*`.
