# RFD D71: Provider-Hosted Tools

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Requires**: [RFD D31]
- **Summary**: Exposes provider-executed server tools such as Anthropic
  `web_search` through per-provider configuration without adding a core tool
  source.

## Summary

This RFD exposes provider-executed server tools (such as Anthropic's
`web_search` and `code_execution`, OpenAI's `web_search`, and Google Search
grounding) as **provider-hosted tools** configured inside each provider's
settings block.
Each provider adapter translates its hosted-tool settings into wire requests and
maps the resulting activity onto [RFD D31]'s response artifacts without adding
hosted-tool concepts to JP's core tool coordinator.
Anthropic web search is the first implementation slice.

## Motivation

Users want current web information in a conversation without setting up a search
MCP server.
Model providers already offer built-in search: Anthropic, for example, charges
$10 per 1,000 searches and returns cited sources.

Treating these tools as another tool source alongside `builtin`, `local`, and
`mcp` conflates tool definition with tool execution.

### Definition Ownership vs. Execution Ownership

| Example                | Schema owner | Executor      | Agent loop |
| ---------------------- | ------------ | ------------- | ---------- |
| Local JP tool          | JP / user    | JP subprocess | JP         |
| MCP tool               | MCP server   | MCP server    | JP         |
| Anthropic `bash`       | Anthropic    | JP            | JP         |
| Anthropic `web_search` | Anthropic    | Anthropic     | Anthropic  |
| OpenAI `web_search`    | OpenAI       | OpenAI        | OpenAI     |
| Google Search          | Google       | Google        | Google     |

Anthropic's documentation separates these two categories: `bash` and
`text_editor` are *client* tools whose schema Anthropic supplies but which the
caller executes, whereas `web_search` and `code_execution` are *server* tools
that Anthropic executes on its own infrastructure.

A single `ToolSource::Llm` variant would cover both categories, leaving it
ambiguous whether JP or the provider dispatches the call.
[RFD D10] rejects folding execution mechanics into `ToolSource` for the same
reason: `ToolSource` identifies where a tool's definition comes from, and mixing
in execution turns the enum into a cross-product of sources and runtimes.

Client tools like Anthropic `bash` fit a future `ToolSource` variant because the
provider supplies the schema while JP owns execution and applies every
`ToolConfig` field.
This RFD leaves that path open.
Server tools like `web_search`, by contrast, are not JP tool calls at all.

### Two Execution Planes

**Client tool** (the existing tool call):

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

## Design

### Per-Provider Configuration

Hosted tools are configured under each provider's settings block and default to
**disabled**:

```toml
[providers.llm.anthropic.hosted_tools.web_search]
enable = true
max_uses = 5
blocked_domains = ["reddit.com"]
```

These settings are typed in `jp_config::providers::llm::anthropic` and validated
by the Anthropic adapter; `jp_llm` core does not inspect them.

Each provider defines its own fields without a shared capability vocabulary
because the underlying parameters differ: `max_uses` has no OpenAI equivalent,
and `search_context_size` has no Anthropic equivalent.

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

### Configuration as the Consent Boundary

Because the provider executes a hosted tool before JP can prompt the user,
per-call approval is impossible.
Setting `enable = true` grants blanket consent for that facility's network
access, billing, and provider-side execution on every request to that provider.

That consent boundary matters even more for code execution and provider-side MCP
connectors than for web search, which is why hosted tools default to disabled.

### Anthropic Adapter Responsibilities

All provider-specific protocol handling lives in `anthropic.rs`; core sees only
the normalized hosted-activity event from [RFD D31].

**Request.** `convert_tools` emits
`types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(..))` with the
configured parameters.
The adapter selects the native wire version (`web_search_20250305` through
`web_search_20260318`) and records it in the persisted payload so that replaying
a conversation across a JP upgrade preserves the wire contract the activity
started with.
The native wire version is not user-configurable.

**Validation.** If a hosted tool is enabled for a model or endpoint that does
not support it (for example, web search on Bedrock, or dynamic filtering on
models earlier than Claude 4.6), the adapter returns a hard error before sending
the request.
Silently dropping the tool would invalidate A/B evaluations by running one model
with search and another without.

**Response.** `map_content_start` maps a `server_tool_use` block to a
hosted-activity part.
Because the result arrives as a single complete `web_search_tool_result` block
rather than streaming deltas, the adapter holds the activity open until the
result block arrives, keeps the activity line visible in the terminal while the
search runs, and then emits the completed event with the native payload
attached.

**Replay.** `convert_events` reconstructs both the `server_tool_use` and
`web_search_tool_result` blocks from the persisted payload, preserving
`encrypted_content` byte-for-byte because Anthropic rejects requests when that
field is modified or missing.
If an activity has no persisted payload, the adapter omits both blocks.

**Continuation.** When the API stops with `pause_turn`, the adapter reuses its
max-token chaining path and resends the paused assistant message unchanged until
the turn reaches a JP-visible boundary.

### Mixed Client and Hosted Calls

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

### Stateless Continuation and Explicit Abandonment

Replaying a deferred activity in step 4 reads from the **conversation stream**,
not from adapter memory.
Because the native payload is persisted in the stream, the adapter reconstructs
it on every request, just as it does for thinking signatures.
Retries, process restarts, and resumptions after an interrupt all rebuild the
request from the stream; holding continuation state in adapter memory would
break all three.

Under [RFD 078], a tool can mutate configuration between cycles, including the
active model, while a hosted activity is still pending:

- **Same provider, declaration changed.** As long as the conversation stream
  holds a pending activity, the adapter continues to declare the hosted tool by
  reading its own persisted payload.
  The newly resolved config does not silently drop a tool declaration that the
  provider transcript still requires.
- **Provider switched.** Per [RFD D31], the new provider's adapter preserves and
  ignores a payload written by another provider.
  The outgoing request remains valid, and the activity stays in `started` state
  with no terminal transition, recording that the search began and never
  finished.

In both cases, when a pending activity cannot continue, the adapter **abandons
it explicitly** and records the abandonment rather than failing mid-turn or
corrupting the provider transcript.
Config deltas are never rejected, and [RFD 078] config mutations are never
blocked to protect provider transcript state.

### Scoping `tool_choice` to Client Tools

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

**Citation rendering is a release gate.** Anthropic, OpenAI, and Google all
impose attribution display requirements on search-backed output, so shipping
search-backed responses without citations violates provider terms.
Both terminal output and the `serve-web` plugin must render the typed
attributions from [RFD D31], either inline or as a source list.

Hosted-activity usage is displayed as the number of searches in the turn,
counted from the recorded `HostedActivity` events.
Because [RFD D31] does not persist provider usage counters, the provider's
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

**Duplicated configuration across providers.** Enabling web search on both
Anthropic and OpenAI requires two config blocks.
Although the provider-specific parameters differ, the `enable` flag is
duplicated across providers.

## Alternatives

**`ToolSource::Llm` with a no-op executor.** Because `llm.bash` is executed by
JP and `llm.web_search` is executed by the provider, a single `ToolSource::Llm`
variant cannot describe both.
Routing server tools through `ToolSource` would also require special cases in
the turn loop, the execution plan, and orphan sanitization, which [RFD D10]
explicitly rejects.

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
enabling a hosted tool on an unsupported model or endpoint is a configuration
error (see Validation above).

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
  OpenAI or Google follows in Phase 5 to test that the boundary generalizes.

## Risks and Open Questions

- **The boundary is unproven until a second provider lands.** If adding OpenAI
  web search forces changes to [RFD D31]'s normalized activity shape, the
  normalization was derived from too few examples.
  Implementing a second provider in Phase 5 tests that boundary before more
  hosted tools are added.
- **Google may not fit `started | completed | failed`.** Search grounding may
  expose only completed metadata with attribution ranges and required Search
  Suggestion content, with no separate call and result pair.
- **Single-block assumption for search results.** Holding a hosted activity open
  until its result arrives relies on `web_search_tool_result` arriving as one
  complete block.
  If Anthropic later streams result content in deltas, the adapter will need a
  different accumulation strategy.
- **Interaction between `pause_turn` and max-token chaining.** Long search turns
  need testing to verify that the existing `should_chain` logic handles
  `pause_turn` alongside max-token continuation.
- **Prompt injection.** Web search pulls untrusted content into the model
  context.
  [RFD 096] covers terminal sanitization; whether hosted results need additional
  handling remains open.

## Implementation Plan

Requires [RFD D31] phases 1 through 4.

### Phase 1: Typed Configuration

Add `hosted_tools` to `AnthropicConfig` with the web-search fields, defaulting
to disabled, along with schema, delta, fill, and partial support.
This phase is independently mergeable before the adapter reads the new fields.

### Phase 2: Request Translation and Validation

Add the `convert_tools` branch, native wire version selection, model and
endpoint validation, and the two `tool_choice` adjustments in `anthropic.rs`.
At this point the tool is declared and the model can call it, while the response
path still drops returned server-tool blocks.

### Phase 3: Response Path

Implement `server_tool_use` mapping, deferred activity flush with native payload
attachment, `convert_events` replay of both native blocks, `pause_turn`
continuation, and explicit abandonment.
This phase completes end-to-end web search execution.

### Phase 4: Rendering and Usage

Implement terminal activity display, citation rendering in the terminal and
`serve-web`, and the per-turn search count.
This phase gates the release.

### Phase 5: Second Provider

Implement web search for OpenAI or Google to verify that provider-owned
translation generalizes without changes to `jp_llm` core.

## References

- `crates/jp_llm/src/provider/anthropic.rs`: `convert_tools`,
  `convert_tool_choice`, `convert_events`, chaining
- `crates/jp_config/src/providers/llm/anthropic.rs`: `AnthropicConfig`
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>

[#208]: https://github.com/dcdpr/jp/issues/208
[RFD 078]: ../078-tool-config-mutation.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[RFD D10]: D10-unified-tool-execution-model.md
[RFD D31]: D31-provider-response-artifacts.md
