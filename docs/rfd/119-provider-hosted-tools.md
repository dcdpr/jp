# RFD 119: Provider-Hosted Tools

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Extends**: [RFD 110]
- **Requires**: [RFD 118]
- **Summary**: Exposes tools a provider runs itself, configured per provider,
  starting with Anthropic web search on API keys and subscriptions.

## Summary

This RFD exposes provider-executed server tools (such as Anthropic's
`web_search` and `code_execution`, OpenAI's `web_search`, and Google Search
grounding) as **provider-hosted tools** configured inside each provider's
settings block.
Each provider adapter translates its hosted-tool settings into wire requests and
maps the resulting activity onto [RFD 118]'s response artifacts without adding
hosted-tool vocabulary to `jp_llm` core.
Anthropic web search is the first vertical slice and ships across all three
`anthropic` flows: API key, direct subscription, and the ACP subscription flow
([RFD 110]).

## Motivation

Users want current web information in a conversation without setting up a search
MCP server.
Model providers already offer built-in search: Anthropic, for example, charges
$10 per 1,000 searches and returns cited sources.

Treating these tools as another tool source alongside `builtin`, `local`, and
`mcp` conflates tool definition with tool execution.

### Definition ownership and execution ownership are separate axes

| Example                       | Schema owner | Executor      | Agent loop  |
| ----------------------------- | ------------ | ------------- | ----------- |
| Local JP tool                 | JP / user    | JP subprocess | JP          |
| MCP tool                      | MCP server   | MCP server    | JP          |
| Anthropic `bash`              | Anthropic    | JP            | JP          |
| Anthropic `web_search`        | Anthropic    | Anthropic     | Anthropic   |
| Claude Code `WebSearch` (ACP) | Claude Code  | Claude Code   | Claude Code |
| OpenAI `web_search`           | OpenAI       | OpenAI        | OpenAI      |
| Google Search                 | Google       | Google        | Google      |

Anthropic's documentation separates these two categories: `bash` and
`text_editor` are *client* tools whose schema Anthropic supplies but which the
caller executes, whereas `web_search` and `code_execution` are *server* tools
that Anthropic executes on its own infrastructure.

A single `ToolSource::Llm` variant would cover both categories, leaving it
ambiguous whether JP or the provider dispatches the call.
`ToolSource` identifies where a tool's definition comes from, and folding
execution into it turns the enum into a cross-product of sources and runtimes.

Client tools like Anthropic `bash` fit a future `ToolSource` variant because the
provider supplies the schema while JP owns execution and applies every
`ToolConfig` field; this RFD leaves that path open.
Server tools like `web_search`, by contrast, are not JP Tool Calls at all.
Neither is Claude Code's `WebSearch` on the ACP flow: although Anthropic's API
treats `WebSearch` as a client tool, the client executing it is Claude Code
inside the `anthropic` provider's ACP runtime, so JP never dispatches it.

### Two execution planes

**Client tool** (the existing Tool Call):

```text
assistant emits ToolCallRequest
    -> JP resolves policy and permissions
    -> JP dispatches a runtime
    -> JP records ToolCallResponse
    -> JP returns the result to the provider
```

**Provider-hosted tool**:

```text
JP enables the facility on a request
    -> provider emits activity
    -> provider executes it
    -> provider continues generating
    -> JP records activity and assistant output
```

JP only observes provider-hosted tools.
It never dispatches them or fabricates a `ToolCallResponse` for them.

### Billing must not decide capability

The `anthropic` provider serves each request through one of three flows: API
key, direct subscription, or the ACP subscription flow ([RFD 110]).
The `auth` chain ([RFD 090]) selects the flow per request; with `auth =
["subscription", "api_key"]`, it falls back from the subscription to the API key
when the subscription allowance is exhausted, potentially between two requests
in the same turn.

If web search worked on only a subset of flows, whether a query could search
would depend on which credential the `auth` chain selected.
Because enabling web search on a flow that cannot serve it is a hard validation
error, an unsupported flow in the fallback chain would cause queries to fail
rather than silently omit search.
Web search therefore ships on all three flows with consistent consent and budget
semantics.

## Design

### Configuration lives in the provider block

Hosted tools are configured under each provider's settings block and default to
**disabled**:

```toml
[providers.llm.anthropic.hosted_tools.web_search]
enable = true
max_searches = 5
blocked_domains = ["reddit.com"]
```

These settings are typed in `jp_config::providers::llm::anthropic` and validated
by the Anthropic adapter; `jp_llm` core does not inspect them.

Each provider defines its own fields without a shared capability vocabulary
because the underlying parameters differ: `max_searches` has no OpenAI
equivalent, and `search_context_size` has no Anthropic equivalent.

Placing configuration under the provider block has four consequences:

- **No `source`, aliases, or fallback ordering.** `assistant.model.id` already
  selects the active provider, and the config key is scoped to that provider.
- **No `run`, `result`, `access`, `questions`, `command`, or `parameters`.**
  These `ToolConfig` fields do not apply because the provider executes the tool
  before JP receives the event, leaving no local execution or interactive
  approval step.
- **`ChatQuery` is unchanged.** `Anthropic` is constructed via
  `TryFrom<&AnthropicConfig>` and `convert_events` already reads per-event
  provider config, so hosted-tool settings arrive through the existing config
  path.
- **Plugin providers require no core changes.** A plugin provider's config
  subtree is already opaque to JP, so its hosted-tool settings pass through
  untouched.

### Settings

| Setting                              | Effect                                                     |
| ------------------------------------ | ---------------------------------------------------------- |
| `enable`                             | Offers web search to the model. Defaults to `false`.       |
| `max_searches`                       | Caps searches per turn. Unset leaves the provider's limit. |
| `allowed_domains`, `blocked_domains` | Restricts results by domain. At most one may be set.       |

`max_searches` is a per-turn budget rather than a per-request limit.
Anthropic's `max_uses` parameter applies to a single HTTP request, while a JP
turn sends one request per tool cycle.
On each request, the adapter sets `max_uses` to `max_searches` minus the number
of searches already completed in the turn (read from the conversation stream).
When a client tool call defers a search to the next request, that search counts
against the follow-up request's `max_uses`; any deferred search beyond the
remaining budget fails with `max_uses_exceeded`, which Anthropic does not bill.
Because Anthropic rejects `max_uses = 0`, once the turn's budget is exhausted
the adapter omits the web search tool declaration and abandons any pending
search (see Continuation).
Completed searches remain in the native history, as Anthropic accepts historical
search blocks even when the current request does not declare the tool.

`user_location` and the dynamic-filtering versions of the tool are out of scope
for the initial slice.

### Configuration is the consent boundary

Because the provider executes a hosted tool before JP can prompt the user,
per-call approval is impossible on the HTTP flows.
Setting `enable = true` grants blanket consent for that facility's network
access, billing, and provider-side execution on every matching request.
That consent boundary matters even more for code execution and provider-side MCP
connectors than for web search, which is why hosted tools default to disabled.

Although Claude Code prompts for permission before each `WebSearch` call on the
ACP flow, JP answers that prompt automatically from configuration so `enable =
true` has identical consent semantics across all flows.

Search results also reach the model without passing through JP: Anthropic
returns them encrypted on the HTTP flows, and Claude Code feeds them to the
model inside its own loop on the ACP flow.
JP cannot filter what the model reads, so `enable = true` also accepts the
prompt-injection surface of raw search results.
JP sanitizes all fields it renders to the terminal (queries, titles, URLs, and
cited text) under [RFD 096].

### HTTP flows: the native server tool

The API-key and direct subscription flows share `create_request` and `call` in
`anthropic.rs` and differ only in their credentials.
Both declare Anthropic's server tool.
All HTTP protocol handling lives in `anthropic.rs`; core sees only the
normalized hosted-activity event from [RFD 118].

**Request.** `convert_tools` emits
`types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(..))`, pinned to
`web_search_20250305` for the initial slice.
Later versions filter results by executing code first, which returns
code-execution blocks, nested search blocks with a `caller` field, and a
container ID to replay, and makes requests ineligible for Zero Data Retention.
The adapter records the wire version in `ProviderPayload` so a JP upgrade does
not continue an activity under a different wire contract.
The wire version is not user-configurable.

**Validation.** Enabling web search on a model or flow that cannot serve it, or
with a setting the flow cannot honor, is a hard error returned by the adapter
before sending the request.
Silently dropping the tool would invalidate A/B evaluations by running one model
with search and another without.
When an account cannot use search, the adapter reports that cause directly
rather than returning a generic rejection: Anthropic responds with a specific
400 error when an organization disables web search, and on the ACP flow a
startup tool inventory missing only `WebSearch` indicates that the active login
does not offer search.

**Response.** Stream translation tracks state per content block.
A `server_tool_use` block opens a hosted-activity part; its query arrives as
`input_json_delta` chunks, which `map_content_delta` currently routes into
client tool-call arguments (marking the turn as having requested client tools
and disabling max-token chaining).
Because the result arrives as a single complete `web_search_tool_result` block,
the adapter holds the activity open across the `server_tool_use` block's
`ContentBlockStop` (which `map_event` currently flushes unconditionally) and
emits the completed activity with its native payload once the result block
arrives.
This keeps the activity line visible in the terminal while the search runs.
Anthropic documents `web_search_tool_result` as a single complete block for
`web_search_20250305`; adopting a future version that streams result deltas
requires dedicated accumulation.

**Replay.** `convert_events` reconstructs both the `server_tool_use` and
`web_search_tool_result` blocks from the persisted payload, preserving
`encrypted_content` byte-for-byte because Anthropic rejects modified or missing
values.
If an activity has no payload or its payload was written by another flow, the
adapter omits both blocks.
A `started` activity is replayed only when it satisfies the continuation rule
below.

**`pause_turn`.** When the API stops with `pause_turn`, the adapter continues
the turn inside its request loop by resending the request with the paused
assistant message appended unchanged and the same tools, repeating until another
stop reason arrives (up to a bounded continuation limit).
This continuation cannot reuse max-token chaining: `chain` appends a user
message asking the model to continue, and `create_request` rewrites trailing
thinking blocks as text and appends a synthetic continuation prompt for models
without prefill, whereas Anthropic rejects any user text block immediately after
an unfinished `server_tool_use`.
Max-token chaining still applies once searches complete, because Anthropic
accepts trailing user text after completed search blocks, but `should_chain`
returns `false` while a search is pending.
If a turn hits `max_tokens` with a search still pending, the turn stops and the
next request abandons the pending search.

### Mixed client and hosted calls

When Claude calls web search and a client tool in the same parallel batch, the
API returns `stop_reason: "tool_use"` and defers the search:

1. JP persists the hosted activity in `started` state with its native payload.
2. JP emits `ToolCallRequest` events for the client tools only.
3. JP executes the client tools as usual.
4. On the next request, the adapter replays the pending activity and the client
   tool results in the native block order that Anthropic requires.
5. Anthropic executes the deferred search and resumes generation.

The core invariant holds throughout: every `ToolCallRequest` represents work
that JP dispatches.

### Continuation is stateless; abandonment is explicit

Replaying a deferred activity in step 4 reads from the **conversation stream**,
not from adapter memory.
Because the payload is persisted in the stream, the adapter reconstructs it on
every request, just as it does for thinking signatures.
Retries, process restarts, and resumptions after an interrupt all rebuild the
request from the stream; holding continuation state in adapter memory would
break all three.

The adapter replays a `started` activity natively only when the request
continues the tool cycle that left it pending and the turn has remaining search
budget.
A request continues that cycle when the activity belongs to the last assistant
message and every subsequent event is a `ToolCallResponse` for that message's
client tool calls.
When continuing a pending search, the adapter keeps declaring the web search
tool even if a tool mutated the configuration between cycles ([RFD 078]),
because the provider transcript requires the declaration.

Every other request abandons the pending activity by omitting the unfinished
`server_tool_use` block from the native request:

- **Budget exhausted.** No positive `max_uses` value can allow the pending
  search without exceeding `max_searches`.
- **Interrupted turn.** If the user interrupts the turn after client tools
  execute and sends a new user message, `convert_events` merges consecutive
  user-role events; replaying the pending `server_tool_use` would place user
  text after `tool_result`, which Anthropic rejects.
- **Interrupted `pause_turn`.** An interrupted `pause_turn` continuation is not
  resumed when rebuilding a later request.
- **Flow or provider switched.** [RFD 078] can change the active model between
  cycles, and the `auth` chain can switch flows.
  Per [RFD 118], the new adapter or flow preserves and ignores a payload format
  it did not write.

Abandonment writes no new event: the activity remains in `started` state with no
terminal transition (which [RFD 118] defines as an unfinished activity), and
because the rule inspects only the stream, every subsequent request reaches the
same decision.
Config deltas are never rejected, and [RFD 078] config mutations are never
blocked to protect provider transcript state.

### `tool_choice` must not broaden silently

Anthropic's wire format places hosted tools and client tools in the same `tools`
array.
Adding hosted tools to that array affects two places in `anthropic.rs`:

- `convert_tool_choice` maps `ToolChoice::Required` to Anthropic's `any` ("use
  one of the provided tools").
  Once `web_search` is in the `tools` array, a `Required` constraint intended
  for the user's client tools could be satisfied by a web search instead.
- The documented single-tool quirk workaround (`tools.len() == 1 &&
  matches!(tool_choice, ToolChoice::Function(_))` downgrading to `Required`)
  stops firing when a hosted tool brings the array length to 2.

`assistant.tool_choice` remains scoped to JP-dispatched client tools.
The adapter excludes hosted tools when applying `tool_choice`, and forcing a
hosted tool is unsupported.

### ACP flow: Claude Code's `WebSearch`

The ACP flow cannot declare Anthropic's HTTP server tool because Claude Code
constructs the API request and accepts built-in tool names rather than raw tool
definitions in its options.
Instead, enabling web search on the ACP flow enables Claude Code's built-in
`WebSearch` tool, which executes each search in a separate sub-request on a
smaller model (Claude Haiku 4.5 in Claude Code 2.1.280) and returns a summary
with result titles and URLs.

Although Anthropic's API sees `WebSearch` as a client tool executed by Claude
Code, from JP's perspective it is work executed by the provider adapter's
runtime and observed by JP.
JP therefore records it as a `HostedActivity` and never as a `ToolCallRequest`:

- **Request.** `options.tools` includes `WebSearch`, and JP's startup inventory
  check against Claude Code's `init` message verifies its presence.
  `WebSearch` is the only native Claude Code tool enabled on the ACP flow and is
  controlled by typed configuration; [RFD 110]'s rule disabling native
  side-effectful tools is unchanged.
- **Permission.** In its default permission mode (used by the ACP flow), Claude
  Code requests permission before each `WebSearch` call, which
  `claude-agent-acp` forwards as `session/request_permission`.
  While the turn's `max_searches` budget remains, JP responds with the same
  allow-once option it uses for JP tools; once the budget is exhausted, JP
  denies the request.
  Unlike JP tool permissions, handling `WebSearch` permission must not emit a
  `ToolCallRequest`.
- **Response.** The `WebSearch` `tool_use` block arrives on the SDK stream that
  JP already reads (which currently ignores `tool_use` blocks for non-JP tools),
  and its result arrives in the SDK user message forwarded by the adapter.
  Alongside the model-visible formatted text in `tool_result`, the SDK user
  message carries a structured `tool_use_result` field containing `query`,
  `searchCount`, and `results` (each backend search's `{title, url}` links
  followed by the summary), which JP's typed SDK message currently drops.
  The adapter records the query from the `tool_use` block, the sources from
  `tool_use_result`, and the `tool_result` text inside `ProviderPayload`,
  versioned by the qualified Claude Code runtime.
- **Replay.** When encoding a native transcript, the adapter reconstructs the
  `WebSearch` `tool_use` and `tool_result` blocks (with the model-visible text)
  from the payload.
  The `mcp__jp__` tool-name prefix applied in `PreparedRequest::new` remains
  restricted to JP tools.
  Only `completed` activities are replayed: because Claude Code completes each
  search before returning control to the model, any `started` activity on the
  ACP flow was interrupted.
- **Decoding.** Because Claude Code does not forward the stream of its internal
  search sub-request, the SDK stream contains only the standard `text` and
  `tool_use` blocks that JP already decodes, and does not require the
  `async-anthropic` block types added in [RFD 118] Phase 1.

Domain filtering (`allowed_domains` and `blocked_domains`) cannot be enforced on
the ACP flow.
Claude Code's `WebSearch` tool takes `allowed_domains` and `blocked_domains` as
model-supplied call arguments rather than runtime configuration, and its
permission rules do not support domain constraints.
Because ignoring a configured `blocked_domains` list would violate user policy
rather than merely degrade search quality, setting either domain list on an ACP
request fails validation before the request starts.

| Setting                              | API key and direct                        | ACP                          |
| ------------------------------------ | ----------------------------------------- | ---------------------------- |
| `enable`                             | Server tool declared                      | `WebSearch` enabled          |
| `max_searches`                       | Each request's `max_uses` from the budget | Permission denied once spent |
| `allowed_domains`, `blocked_domains` | Native tool fields                        | Hard error                   |

### Activation

Because hosted tools live outside `conversation.tools`, `-T`, `--no-tool`, and
tool groups do not apply to them.

To enable a hosted tool for a single query, pass `--cfg` with either the full
config path or a config fragment resolved against `config_load_paths`:

```sh
jp query --cfg providers.llm.anthropic.hosted_tools.web_search.enable=true "..."
jp query --cfg websearch "..."   # a websearch.toml fragment
```

This RFD introduces no dedicated CLI flag; a shorthand selector can be added
later if one-off activation proves common.

### Rendering

Terminal output displays the activity while it runs and its outcome when it
completes:

```text
⚒ web search: "rust async trait object safety"
  3 results
```

**Source rendering is a release gate.** Anthropic, OpenAI, and Google all impose
attribution display requirements on search-backed output, so shipping
search-backed responses without sources violates provider terms.
On the HTTP flows, the API anchors citations in the assistant text, and JP
renders the typed attributions from [RFD 118] inline or as a source list.
On the ACP flow, where citations are not anchored in text, JP renders the
activity's `sources` under the turn.
The `serve-web` plugin follows the same rules.

Hosted-activity usage is displayed as the number of searches in the turn,
counted from the recorded `HostedActivity` events.
Because [RFD 118] does not persist provider usage counters, the provider's
reported count (`usage.server_tool_use.web_search_requests`) is used only as a
diagnostic.

## Drawbacks

**Two configuration locations for tool-like features.** Users who enable web
search may look for it under `conversation.tools` and not find it.
Separating client tools from provider-hosted tools reflects their different
execution planes, but it adds a learning cost that ubiquitous-language
definitions only partially mitigate.

**Switching providers silently changes capability.** If a user configures
Anthropic's web search and runs a query on OpenAI, that query runs without
search.
Under provider-scoped placement, OpenAI treats this as an unconfigured
capability rather than a degraded cross-provider setting.
This matches how reasoning support works: `adaptive_effort` warns when it clamps
an explicitly requested level, whereas a model that does not support reasoning
simply runs without it.

**Verbose `--cfg` overrides.** Passing `--cfg
providers.llm.anthropic.hosted_tools.web_search.enable=true` on the command line
is long, though config fragments (`--cfg websearch`) mitigate the typing cost.

**Domain lists are unsupported on the ACP flow.** While web search itself works
across all three flows, domain filtering does not.
Users who configure `allowed_domains` or `blocked_domains` alongside
`subscription` in `auth` receive a validation error on the ACP flow and must
choose between domain filtering and subscription billing.

**`max_searches` measures different units across flows.** On the ACP flow, a
single `WebSearch` call can execute up to eight backend searches (reported as
`searchCount` by Claude Code), and the primary model receives only the smaller
model's summary and result links rather than the full page content replayed on
the HTTP flows.
Because JP can only allow or deny each `WebSearch` invocation, `max_searches`
counts `WebSearch` calls rather than backend queries (though observed probes
executed one backend search per call).

**Switching flows mid-conversation drops native search context.** When the
`auth` chain falls back between flows mid-conversation, searches recorded by the
earlier flow retain their portable projection (assistant answer and sources),
but their native search results are not replayed.
If search remains enabled, the model may repeat searches it already ran.

**Subscription search cost is opaque.** Each ACP `WebSearch` call runs a
separate sub-request on the smaller model (roughly 15,000 input tokens in
verification probes), and Claude Code reports that sub-request's token usage and
search count in per-model usage metadata.
Because Anthropic does not report subscription allowance deduction per request,
JP can display search counts and token usage but not subscription quota cost.

**Duplicated configuration across providers.** Enabling web search on both
Anthropic and OpenAI requires two config blocks.
Although the provider-specific parameters differ, the `enable` flag is
duplicated across providers.

## Alternatives

**`ToolSource::Llm` with a no-op executor.** Because `llm.bash` is executed by
JP and `llm.web_search` is executed by the provider, a single `ToolSource::Llm`
variant cannot describe both.
Routing server tools through `ToolSource` would also require special cases in
the turn loop, the execution plan, and orphan sanitization.

**`assistant.request.hosted_tools.<capability>.<provider>`.** A
provider-agnostic capability key with per-provider sub-blocks would provide one
`enable` flag per capability, symmetric A/B test configurations, and persistent
activation across model switches.
However, it requires core to maintain a capability namespace and a
`ChatQuery.hosted_tools` field, and it forces plugin providers to use capability
names that core recognizes.
Provider-scoped placement aligns with moving providers into plugins.
Earlier objections to provider-scoped placement do not hold:
`PartialAppConfig::delta` covers `providers`, the config tree stores
`api_key_env` rather than raw secrets, and [RFD 078] grants are scoped per path
rather than per section.
This trade-off would change only if credential fallback ever crossed provider
blocks and required duplicating `enable` to preserve a capability.

**A `provider_alias` map on the tool entry, or a provider-side `tools.<native> =
"<jp name>"` map.** Both rely on the rejected tool-entry model.
The provider-side form also inverts lookup direction: understanding a tool would
require searching every provider block for a reverse mapping, and provider
config would contain JP-namespace tool names.

**Ordered fallback variants.** Resolving a single tool name to a provider-hosted
search, then an MCP search, and then a local tool hides major operational
differences behind one name.
Provider-hosted and client-executed tools differ in schema, invocation protocol,
pricing, security posture, and citation obligations, and one runs inside the
provider's loop while the other pauses generation for JP execution.
Capability routing and multi-exposure (tracked in [#208]) belong in a separate
feature.

**Automatic discovery via `Provider::executed_tools()`.** Nothing in this design
queries a runtime capability list.
Typed fields inside each provider's config block give built-in providers
compile-time and schema diagnostics, and a plugin provider's own schema covers
external providers without core arbitrating.

**Silently dropping an unsupported hosted tool with a warning.** Skipping an MCP
tool when its server is unreachable is a runtime availability fallback, whereas
enabling a hosted tool on an unsupported model or flow is a configuration error
(see Validation above).

**Shipping on the HTTP flows before the ACP flow.** Because billing must not
determine capability, the feature cannot be announced until all three flows
support web search.
The HTTP implementation phases can merge first, provided that enabling web
search on an ACP request returns a hard error until Phase 4 lands.

**A JP client tool that calls the Anthropic Messages API with web search.**
While a JP-dispatched wrapper tool would work uniformly across providers and
flows, it requires an API key (which subscription-only setups lack) and
reintroduces the cross-provider capability routing excluded from this RFD.

**Routing subscription search requests through the `direct` flow.** Per [RFD
110], JP never falls back to the `direct` subscription flow automatically
because direct OAuth replay carries account-policy risks that require explicit
opt-in.

**Prompting per call on the ACP flow.** Although Claude Code requests permission
before each `WebSearch` call, the HTTP flows cannot prompt per call.
Prompting only on the ACP flow would make consent semantics depend on the active
billing flow.

**Enforcing domain lists through ACP permission responses.** Although JP
inspects each `WebSearch` call's arguments during permission handling, the model
chooses `allowed_domains` and `blocked_domains` without knowing JP's policy, and
ACP permission responses cannot rewrite tool arguments.
Enforcing domain lists through permission denials would require injecting prompt
instructions and repeatedly denying non-compliant calls.

## Non-Goals

- **Provider-supplied client tools** (Anthropic `bash`, `text_editor`,
  `computer_use`).
  These tools use a provider-owned schema with JP execution, which is a separate
  `ToolSource` design.
- **Forcing a hosted tool** via `assistant.tool_choice`.
- **Fallback or capability routing** across hosted, MCP, and local
  implementations of one capability.
- **A shared cross-provider capability vocabulary.** Each provider defines its
  own hosted-tool names and parameters.
- **Provider-hosted tools beyond web search** in the first slice.
  OpenAI follows in Phase 6 to verify that the boundary generalizes.
- **Google Search grounding.** Google requires displaying its HTML and CSS
  Search Suggestions alongside every grounded answer, and [RFD 118] defers that
  display content to a future Google Search RFD.
  Whether a terminal client can satisfy Google's display terms determines
  whether Google Search fits JP.
- **Web search versions with dynamic filtering** (`web_search_20260209` and
  later) and `user_location`.
- **Claude Code native tools other than `WebSearch`** on the ACP flow, including
  `WebFetch`.

## Risks and Open Questions

- **Claude Code's `WebSearch` behavior is observed rather than documented.** The
  nested sub-request model, the structure of `tool_use_result`, and
  `searchCount` reflect observed behavior in Claude Code 2.1.280 rather than a
  published specification.
  [RFD 110]'s runtime qualification must re-verify these details against the
  Phase 4 fixture on each qualified release.
- **`pause_turn` is documented rather than observed.** Because no test prompt
  reliably triggers `pause_turn`, the continuation implementation follows
  Anthropic's documentation and is tested in Phase 3 against a synthetic
  response fixture.

## Implementation Plan

Requires [RFD 118] phases 1 through 4.

### Phase 1: Typed configuration

Add `hosted_tools` to `AnthropicConfig` with the web-search fields (defaulting
to disabled), along with schema, delta, fill, and partial support.
This phase is independently mergeable before the adapter reads the new fields.

### Phase 2: HTTP request translation and validation

Add the `convert_tools` branch pinned to `web_search_20250305`, model and
setting validation, per-request `max_uses` calculation from the turn budget, and
the two `tool_choice` adjustments in `anthropic.rs`.
Enabling web search on an ACP request returns a hard error until Phase 4.
At this point the tool is declared and the model can call it, while the response
path still drops returned server-tool blocks.

### Phase 3: HTTP response path

Implement per-block stream state, deferred activity flush with native payload
attachment, `convert_events` replay of both native blocks, `pause_turn`
continuation, and the stateless continuation and abandonment rules.
This completes end-to-end web search on the API-key and direct subscription
flows, each backed by a recorded cassette fixture (verifying server-tool
requests with a subscription credential on the direct flow).

### Phase 4: ACP flow

Record a `WebSearch` session against the qualified adapter and runtime as the
test fixture for the phase.
Then add `WebSearch` to the ACP options and expected startup inventory, answer
permission requests from the per-turn budget, parse `tool_use_result` on the
typed SDK user message, record `HostedActivity` events from the SDK stream and
user message, replay searches in native transcripts, and reject domain lists on
ACP requests.

### Phase 5: Rendering and usage

Implement terminal activity display, source rendering in the terminal and
`serve-web` across both HTTP and ACP flows, and the per-turn search count.
Releasing the feature requires completing Phases 3 through 5.

### Phase 6: OpenAI

Implement OpenAI web search to verify that provider-owned translation
generalizes without changes to `jp_llm` core.
OpenAI's search call items and text annotations already fit [RFD 118]'s model.

## References

- `crates/jp_llm/src/provider/anthropic.rs`: `convert_tools`,
  `convert_tool_choice`, `convert_events`, `map_event`, chaining
- `crates/jp_llm/src/provider/anthropic/acp/options.rs`: Claude Code tool
  options
- `crates/jp_llm/src/provider/anthropic/acp/protocol.rs`: inventory check,
  permission answers, SDK message translation
- `crates/jp_llm/src/provider/anthropic/acp/transcript.rs`: native transcript
  encoding
- `crates/jp_config/src/providers/llm/anthropic.rs`: `AnthropicConfig`
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>
- <https://code.claude.com/docs/en/tools-reference#websearch-tool-behavior>

[#208]: https://github.com/dcdpr/jp/issues/208
[RFD 078]: 078-tool-config-mutation.md
[RFD 090]: 090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[RFD 118]: 118-provider-response-artifacts.md
