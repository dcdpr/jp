# RFD D71: Provider-Hosted Tools

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Requires**: [RFD D31]

## Summary

Some LLM providers execute tools on their own infrastructure: Anthropic's
`web_search` and `code_execution`, OpenAI's `web_search`, Google's Search
grounding.
This RFD exposes them as **provider-hosted tools**, configured inside each
provider's own config block and translated entirely by that provider's adapter.
Anthropic web search is the first vertical slice.

JP core gains no vocabulary for hosted tools.

## Motivation

Users want current information in a conversation without wiring up a search MCP
server, and the provider already offers it — Anthropic charges $10 per 1,000
searches and returns cited sources.

The obvious approach is to make these tools appear as another tool source
alongside `builtin`, `local`, and `mcp`.
That is wrong, and the reason matters enough to state up front.

### Definition ownership and execution ownership are separate axes

| Example                | Schema owner | Executor      | Agent loop |
| ---------------------- | ------------ | ------------- | ---------- |
| Local JP tool          | JP / user    | JP subprocess | JP         |
| MCP tool               | MCP server   | MCP server    | JP         |
| Anthropic `bash`       | Anthropic    | JP            | JP         |
| Anthropic `web_search` | Anthropic    | Anthropic     | Anthropic  |
| OpenAI `web_search`    | OpenAI       | OpenAI        | OpenAI     |
| Google Search          | Google       | Google        | Google     |

Anthropic's own documentation splits these: `bash` and `text_editor` are
*client* tools whose schema Anthropic supplies but which the caller executes;
`web_search` and `code_execution` are *server* tools Anthropic runs itself.

A `ToolSource::Llm` variant would therefore mean opposite things about who
dispatches, depending on which tool it named.
[RFD D10] already rejects this class of change — its Alternatives section
rejects folding execution mechanics into `ToolSource` on the grounds that
`ToolSource` answers "where does the definition come from?" and mixing in
execution makes the enum a product of sources times runtimes.

The `bash` row *is* a legitimate future `ToolSource` case: provider-supplied
schema, JP execution, every `ToolConfig` field meaningful.
This RFD does not cover it, and does not foreclose it.

The `web_search` row is not a Tool Call at all.

### Two execution planes

**Client tool** — the existing Tool Call:

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

JP observes the second.
It never dispatches it, and never fabricates a `ToolCallResponse` for it.

## Design

### Configuration lives in the provider block

```toml
[providers.llm.anthropic.hosted_tools.web_search]
enable = true
max_uses = 5
blocked_domains = ["reddit.com"]
```

Typed, defined in `jp_config::providers::llm::anthropic`, validated by the
Anthropic adapter, invisible to `jp_llm` core.

OpenAI's block has its own fields under its own names.
No shared capability vocabulary is negotiated, because the knobs are not shared:
`max_uses` has no OpenAI equivalent and `search_context_size` has no Anthropic
one.

Consequences of this placement:

- **No `source`, no aliases, no fallback ordering.** The provider serving the
  request is already selected by `assistant.model.id`, and the config key is
  scoped to that provider.
- **No `run`, `result`, `access`, `questions`, `command`, or `parameters`.**
  None of them apply: there is no local execution and no approval point, because
  the work has already happened by the time JP sees it.
- **`ChatQuery` is unchanged.** `Anthropic` is built via
  `TryFrom<&AnthropicConfig>` and `convert_events` already reads per-event
  provider config, so hosted-tool settings arrive through the existing path with
  no new plumbing.
- **A plugin provider needs nothing from core.** Its config subtree is already
  opaque to JP; its hosted tools ride along inside it.

Hosted tools default to **disabled**.

### Configuration is the consent boundary

The provider executes the work before JP can prompt, so per-call approval is
impossible.
`enable = true` is therefore blanket consent for that facility: its network
access, its spend, and its provider-side execution behavior for every matching
request.

This matters more for code execution and provider-side MCP connectors than for
web search, and it is why the default is off.

### The Anthropic adapter owns everything else

**Request.** `convert_tools` gains a branch emitting
`types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(..))` with the
configured knobs.
The adapter selects the native wire version (`web_search_20250305` through
`web_search_20260318`) and records that selection in the payload, so a JP
upgrade does not continue an activity under a contract it did not begin under.
The native version is not user-configurable.

**Validation.** Enabling a hosted tool the active model or endpoint cannot serve
is a hard error raised by the adapter before the request goes out — web search
is unavailable on Bedrock, and dynamic filtering requires Claude 4.6 or later.
Silently dropping it would let an A/B run compare a model with search against
one without while claiming to compare models.

**Response.** `map_content_start` maps a `server_tool_use` block to a
hosted-activity part.
The result arrives as one complete `web_search_tool_result` block rather than
deltas, so the adapter holds the activity open until the result lands and then
emits it with the native payload attached.
The user sees the activity line stay up while the search runs, which is the
correct display.

**Replay.** `convert_events` reconstitutes both native blocks from the persisted
payload — `server_tool_use` and `web_search_tool_result`, with
`encrypted_content` byte-identical, since Anthropic rejects a modified or
missing value.
An activity whose payload is absent has neither block emitted.

**Continuation.** `pause_turn` joins the existing max-token chaining inside the
adapter: resend the paused assistant message unchanged until the turn reaches a
JP-visible boundary.

Every one of these lives in `anthropic.rs`.
Core sees only the normalized hosted-activity event from [RFD D31].

### Mixed client and hosted calls

When Claude calls web search and a client tool in the same parallel batch, the
API returns `stop_reason: "tool_use"` and does *not* run the search yet:

1. JP persists the hosted activity in `started` state with its native payload.
2. JP emits `ToolCallRequest` events for the client tools only.
3. JP executes those normally.
4. On the next request the adapter replays the pending activity and the client
   results in the native order Anthropic requires.
5. Anthropic runs the deferred search and resumes.

The core invariant holds throughout: every `ToolCallRequest` is work JP must
dispatch.

### Continuation is stateless; abandonment is explicit

Step 4 reads from the **conversation stream**, not adapter memory.
The payload is persisted, so the adapter reconstitutes it on every request —
exactly as it does for thinking signatures today.
Retry, restart, and resumption after interrupt all rebuild the request from the
stream, and adapter-held state would break all three.

[RFD 078] lets a tool change config between cycles, including the active model,
so a pending activity can meet a config that cannot continue it.
Two cases:

- **Same provider, declaration changed.** The adapter keeps declaring the tool
  for as long as it holds a pending activity, reading its own persisted payload.
  The freshly resolved config does not silently drop a declaration the
  transcript depends on.
- **Provider switched.** The new adapter preserves and ignores a payload it did
  not write ([RFD D31]'s contract), so the request is valid and the activity
  stays `started` with no terminal transition — an honest record that a search
  began and never finished.

In both cases the adapter **abandons explicitly**, recording the abandonment,
rather than erroring mid-turn or silently corrupting the provider transcript.
No config delta is rejected, and no capability [RFD 078] promises is blocked to
protect a transcript detail.

### `tool_choice` must not broaden silently

Hosted tools go into the same native `tools` array as client tools, which is the
wire shape.
Two existing sites in `anthropic.rs` change behavior as a result, and both must
be handled:

- `convert_tool_choice` maps `ToolChoice::Required` to Anthropic's `any`,
  meaning "use one of the provided tools."
  Once `web_search` is in that array, a user who asked the model to call one of
  *their* tools can have that satisfied by a search instead.
- The documented single-tool quirk workaround (`tools.len() == 1 &&
  matches!(tool_choice, ToolChoice::Function(_))` downgrading to `Required`)
  stops firing when a hosted tool makes the array length 2.

Policy: `assistant.tool_choice` stays scoped to JP-dispatched tools.
The adapter ensures the native request does not extend it to hosted tools, and
forcing a hosted tool is unsupported.

### Activation

Hosted tools sit outside `conversation.tools`, so `-T`, `--no-tool`, and tool
groups do not apply to them — correctly, since they are not tools.

For a single query, `--cfg` carries the override.
The full path is long, so the documented form uses a config fragment resolved
against `config_load_paths`:

```sh
jp query --cfg providers.llm.anthropic.hosted_tools.web_search.enable=true "..."
jp query --cfg websearch "..."   # a websearch.toml fragment
```

No new flag.
If the pattern proves common enough, a dedicated selector can be added later.

### Rendering

Terminal output shows the activity while it runs and its outcome afterward:

```text
⚒ web search: "rust async trait object safety"
  3 results
```

**Citations are a release gate, not a follow-up.** Anthropic, OpenAI, and Google
all impose attribution or display obligations on search-backed output.
Rendering search-backed assistant text without its citations is not an
incomplete feature, it is a non-compliant one.
The typed attributions from [RFD D31] are rendered inline or as a source list,
and web rendering in the `serve-web` plugin follows the same rule.

Hosted-activity spend (`usage.server_tool_use.web_search_requests`) surfaces
alongside token usage.

## Drawbacks

**Two configuration idioms for things users think of as tools.** Someone who
enabled web search will look for it under `conversation.tools` and not find it.
The split is honest — they are different execution planes — but it is a real
learning cost, and the ubiquitous-language entries are the mitigation rather
than a fix.

**Switching provider silently changes capability.** Configure Anthropic's web
search, run a query on OpenAI, and that query has no search.
This is deliberate: under this placement it is an *absent capability on an
unconfigured provider*, not a degraded explicit setting, which matches how
reasoning support already behaves.
`adaptive_effort` warns when it clamps an explicitly requested level; nothing
warns when a model simply does not reason.

**`--cfg providers.llm.anthropic.hosted_tools.web_search.enable=true` is long.**
A direct cost of provider-scoped placement, mitigated by config fragments rather
than removed.

**Per-provider configuration duplicates intent.** Wanting web search on both
Anthropic and OpenAI means two config blocks.
The knobs genuinely differ, so some duplication is inherent, but the `enable`
flag is duplicated for no reason other than placement.

## Alternatives

**`ToolSource::Llm` plus a no-op executor.** Rejected.
`llm.bash` and `llm.web_search` would differ in who executes, so one source
variant cannot describe both; and it requires exceptions in the turn loop, the
execution plan, and orphan sanitization.
[RFD D10] rejects this class of change explicitly.

**`assistant.request.hosted_tools.<capability>.<provider>`,** with a
provider-agnostic capability key and per-provider sub-blocks.
Genuinely attractive: one `enable` per capability, symmetric A/B arms, and the
tool stays enabled across a model switch.
Rejected because core would have to hold a capability namespace and a
`ChatQuery.hosted_tools` field, and a plugin provider's hosted tools would need
names core recognizes.
Provider-scoped placement is a step toward providers-as-plugins; this is a step
away.
The earlier arguments for it — that provider config is not conversation-scoped,
and that grants would sit next to credentials — do not hold:
`PartialAppConfig::delta` covers `providers`, the tree holds only `api_key_env`
rather than a secret, and [RFD 078] grants are per-path rather than per-section.

**A `provider_alias` map on the tool entry**, or a provider-side `tools.<native>
= "<jp name>"` map.
Rejected with the tool-entry model itself.
The provider-side form also reads backwards: understanding a tool would mean
grepping every provider block for a reverse mapping, and it puts JP-namespace
names inside a block that should describe the provider.

**Ordered fallback variants** — one tool name resolving to a provider-hosted
search, else an MCP search, else a local one.
Rejected here.
Their schemas, invocation protocols, pricing, security posture, and citation
obligations all differ, and one runs inside the provider's loop while the other
stops it for JP execution.
Ordered fallback would hide those differences behind a single name.
Capability routing is a separate feature; issue [#208] covers adjacent
multi-exposure requirements and should not be mixed in.

**Automatic discovery** via a `Provider::executed_tools()` capability list.
Rejected as premature — it exists to serve a discovery caller this design does
not have.
Typed fields inside each provider's config block give in-tree providers better
diagnostics, and a plugin provider's own schema covers the open case without
core arbitrating.

**Silently dropping an unsupported hosted tool** with a warning, mirroring how
an MCP tool whose server is down is skipped.
Rejected: see Validation above.

## Non-Goals

- **Provider-supplied client tools** (Anthropic `bash`, `text_editor`,
  `computer_use`).
  Provider-owned schema, JP execution — a `ToolSource` question, deliberately
  separate.
- **Forcing a hosted tool** via `assistant.tool_choice`.
- **Fallback or capability routing** across hosted, MCP, and local
  implementations of one capability.
- **A shared cross-provider capability vocabulary.** Each provider names its
  own.
- **Provider-hosted tools beyond web search** in the first slice.
  OpenAI or Google follows as the test that the boundary generalizes.

## Risks and Open Questions

- **The boundary is unproven until a second provider lands.** If adding OpenAI
  web search forces changes to [RFD D31]'s normalized activity shape, the
  normalization was derived from too few examples.
  Finding that with one implementation in hand is the point of doing them in
  this order.
- **Google may not fit `started | completed | failed`.** Search grounding may
  expose only completed metadata with attribution ranges and required Search
  Suggestion content, with no call/result pair.
- **Deferring the flush of a hosted activity until its result arrives** relies
  on the result being a single complete block.
  If Anthropic ever streams result content in deltas, the adapter needs a
  different accumulation strategy.
- **Whether `pause_turn` interacts with max-token chaining** in ways the
  existing `should_chain` logic mishandles needs testing against a long search
  turn.
- **Prompt injection.** Web search pulls untrusted content into context.
  [RFD 096] covers terminal sanitization; whether hosted results need handling
  beyond that is open.

## Implementation Plan

Requires [RFD D31] phases 1 through 4.

### Phase 1: Typed configuration

`hosted_tools` on `AnthropicConfig` with the web-search fields, defaulting to
disabled.
Schema, delta, fill, and partial support.
Independently mergeable; nothing reads it yet.

### Phase 2: Request translation and validation

`convert_tools` branch, native version selection, model and endpoint validation,
and the two `tool_choice` corrections.
At this point the tool is declared and the model can call it, but the response
path drops the blocks.

### Phase 3: Response path

`server_tool_use` mapping, deferred flush with payload attachment,
`convert_events` replay of both native blocks, `pause_turn` continuation, and
the abandonment rules.
This is the slice that makes web search work end to end.

### Phase 4: Rendering and usage

Activity display, citation rendering in terminal and `serve-web`, hosted usage
reporting.
Gates the release, per Rendering above.

### Phase 5: Second provider

OpenAI or Google, as the test that provider-owned translation generalizes
without touching core.

## References

- `crates/jp_llm/src/provider/anthropic.rs` — `convert_tools`,
  `convert_tool_choice`, `convert_events`, chaining
- `crates/jp_config/src/providers/llm/anthropic.rs` — `AnthropicConfig`
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>

[#208]: https://github.com/dcdpr/jp/issues/208
[RFD 078]: ../078-tool-config-mutation.md
[RFD 096]: ../096-terminal-output-sanitization-for-untrusted-content.md
[RFD D10]: D10-unified-tool-execution-model.md
[RFD D31]: D31-provider-response-artifacts.md
