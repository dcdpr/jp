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

Tool cases need a tool host.
`chat_completion_stream` passes `mcp_endpoint: None`, and `acp::stream` returns
`ToolHostRequired` when tools are present, so the route must run through
`start_query` with a test MCP endpoint serving the case's tools.
The adapter executes tools itself (`ToolExecution::Agent`), so the test endpoint
answers each tool call rather than the runner appending a tool response.

Land before or with T-19mv6rh, which deletes the direct flow these cassettes
depend on.

- [ ] Add an ACP route for the shared suite that records through the ACP
      cassette tap.
- [ ] Add a test MCP tool host for the ACP route that answers each case's tool
      calls, and run the route through `start_query`.
- [ ] Remove the `records_new_cases` skip for the Anthropic subscription route.
- [ ] Record every `every_route` case against ACP and delete
      `tests/fixtures/anthropic_subscription/`.
- [ ] Record the subscription route against a 5.x model (keeping it on
      `claude-haiku-4-5` until then so existing cassettes continue to replay).
- [ ] Add multi-turn cases that run more than one turn: attachments on turn 1, a
      tool round, and a follow-up turn.
      Assert that the follow-up's first request reads the earlier turns from
      cache (T-1s7kt7k), not only the system prompt and tools.
- [ ] Record what Claude Code sends to Anthropic, beside the JP-to-adapter
      traffic: the model request decides caching and carries Claude Code's
      additions (the synthetic "No response requested." reply, the `userEmail`
      reminder from T-1njzkjd).
      Claude Code writes each request into its session JSONL as
      `api-request-blob` records, which JP deletes unless `JP_DEBUG` is set;
      `OTEL_LOG_RAW_API_BODIES=file:<dir>` is the documented alternative but
      needs telemetry, which JP turns off.
      Replace the account email with a placeholder before committing.
