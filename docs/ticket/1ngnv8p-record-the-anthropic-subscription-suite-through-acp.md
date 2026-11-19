# Record the Anthropic subscription suite through ACP

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=task

The shared recorded suite (`provider::tests::anthropic_subscription::*` in
`crates/jp_llm/src/provider_tests.rs`) runs Anthropic's subscription route
through `SubscriptionTestRoute` (`provider/anthropic/test_support.rs`).
That route records with `SubscriptionFlow::Direct`: it sends a stored Claude
Pro/Max OAuth token straight to the Messages API, which Anthropic does not
sanction and T-19mv6rh removes.

`records_new_cases` therefore skips every newer case on that route, and the 11
existing cassettes under `tests/fixtures/anthropic_subscription/` can only be
replayed.
Any change to the request JP builds for those cases breaks them, with no
sanctioned way to record them again.

The supported subscription path is ACP (the Claude Code adapter).
It has its own JSON-RPC recorder (`acp/cassette.rs`, `acp/recorded_tests.rs`,
fixtures under `tests/fixtures/acp/`), separate from the shared suite.

## Work

Give the shared suite an ACP subscription route, so the `every_route` cases
record and replay against the adapter instead of the Messages API.
The harness assumes an HTTP route (`ProviderTestRoute::base_url` /
`set_base_url` behind httpmock); ACP is a subprocess, so the route needs the ACP
cassette tap instead.
Claude Code picks the thinking mode when `thinking` is omitted, so request
assertions on this route compare what JP sends to the adapter, not the Messages
API body.

Land before or with T-19mv6rh, which deletes the direct flow these cassettes
depend on.

- [ ] ACP route for the shared suite, recording through the ACP cassette tap.
- [ ] Drop the `records_new_cases` exception for the Anthropic subscription
      route.
- [ ] Record every `every_route` case against ACP and delete
      `tests/fixtures/anthropic_subscription/`.
- [ ] Record the subscription route against a 5.x model; it stays on
      `claude-haiku-4-5` until then so the existing cassettes keep replaying.
