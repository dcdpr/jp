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

Some LLM providers execute tools on their own infrastructure: Anthropic's
`web_search` and `code_execution`, OpenAI's `web_search`, Google's Search
grounding.
This RFD exposes them as **provider-hosted tools**, configured inside each
provider's own config block and translated entirely by that provider's adapter.
Anthropic web search is the first vertical slice.
It ships on every flow the `anthropic` provider serves: API key, direct
subscription, and the ACP subscription flow.

JP core gains no vocabulary for hosted tools.

## Motivation

Users want current information in a conversation without wiring up a search MCP
server, and the provider already offers it — Anthropic charges $10 per 1,000
searches and returns cited sources.

The obvious approach is to make these tools appear as another tool source
alongside `builtin`, `local`, and `mcp`.
That is wrong, and the reason matters enough to state up front.

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

Anthropic's own documentation splits these: `bash` and `text_editor` are
*client* tools whose schema Anthropic supplies but which the caller executes;
`web_search` and `code_execution` are *server* tools Anthropic runs itself.

A `ToolSource::Llm` variant would therefore mean opposite things about who
dispatches, depending on which tool it named.
`ToolSource` answers "where does the definition come from?", and folding
execution into it makes the enum a product of sources times runtimes.

The `bash` row *is* a legitimate future `ToolSource` case: provider-supplied
schema, JP execution, every `ToolConfig` field meaningful.
This RFD does not cover it, and does not foreclose it.

The `web_search` row is not a Tool Call at all.
Neither is the Claude Code row, seen from JP: Anthropic's API treats `WebSearch`
as a client tool, but the client running it is Claude Code, which the
`anthropic` provider's ACP flow drives.
JP never dispatches it.

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

### Billing must not decide capability

The `anthropic` provider serves each request through one of three flows: API
key, direct subscription, or the ACP subscription flow ([RFD 110]).
The `auth` chain ([RFD 090]) picks the flow per request.
With `auth = ["subscription", "api_key"]` it moves from the subscription to the
API key when the allowance runs out, possibly between two requests of one turn.

If web search worked on one flow and not another, whether a query could search
would depend on which credential the chain picked.
Under the Validation rule below, an enabled search on a flow that cannot serve
it is a hard error, so the gap would show up as failed queries, not a quiet
absence.
Web search therefore ships on all three flows, with the same consent and budget
semantics on each.

## Design

### Configuration lives in the provider block

```toml
[providers.llm.anthropic.hosted_tools.web_search]
enable = true
max_searches = 5
blocked_domains = ["reddit.com"]
```

Typed, defined in `jp_config::providers::llm::anthropic`, validated by the
Anthropic adapter, invisible to `jp_llm` core.

OpenAI's block has its own fields under its own names.
No shared capability vocabulary is negotiated, because the knobs are not shared:
`max_searches` has no OpenAI equivalent and `search_context_size` has no
Anthropic one.

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

### Settings

| Setting                              | Effect                                                     |
| ------------------------------------ | ---------------------------------------------------------- |
| `enable`                             | Offers web search to the model. Defaults to `false`.       |
| `max_searches`                       | Caps searches per turn. Unset leaves the provider's limit. |
| `allowed_domains`, `blocked_domains` | Restricts results by domain. At most one may be set.       |

`max_searches` counts per turn because that is the unit a user budgets.
Anthropic's `max_uses` applies to one request, and a JP turn sends one request
per tool cycle.
The adapter sets each request's `max_uses` to the budget minus the searches
completed in the turn, read from the stream.
A search deferred by a client call runs in the next request and counts against
that request's `max_uses`, even when it was called under a different one, so
deferred searches beyond the remaining budget fail with `max_uses_exceeded`,
which Anthropic does not bill.
Anthropic rejects `max_uses = 0`, so once the budget is spent the adapter stops
declaring the tool and abandons any search still pending (see Continuation).
Completed searches still replay natively: Anthropic accepts them in history when
the request does not declare the tool.

`user_location` and the dynamic-filtering versions of the tool are outside the
first slice.

### Configuration is the consent boundary

The provider executes the work before JP can prompt, so per-call approval is
impossible.
`enable = true` is therefore blanket consent for that facility: its network
access, its spend, and its provider-side execution behavior for every matching
request.

This matters more for code execution and provider-side MCP connectors than for
web search, and it is why the default is off.

The ACP flow could prompt, since Claude Code asks before each `WebSearch` call.
JP answers that request from configuration instead, so `enable = true` means the
same thing on every flow.

Search results reach the model without passing through JP.
On the HTTP flows Anthropic returns them encrypted, and on the ACP flow Claude
Code hands them to the model inside its own loop, so JP has no point at which to
filter what the model reads.
Prompt injection through search results is part of what `enable = true` consents
to.
JP's own exposure is what it renders, the queries, titles, URLs, and cited text,
which [RFD 096] sanitizes.

### HTTP flows: the native server tool

The API-key and direct subscription flows share `create_request` and `call` in
`anthropic.rs`, and differ only in the credential.
Both declare Anthropic's server tool.

**Request.** `convert_tools` gains a branch emitting
`types::Tool::WebSearch(ToolWebSearch::WebSearch20250305(..))`.
The first slice pins `web_search_20250305`.
Later versions filter results by running code first: responses then carry
code-execution blocks, nested search blocks with a `caller`, and a container id
to pass back, and that filtering is not eligible for Zero Data Retention.
The adapter records the version in the payload, so a JP upgrade does not
continue an activity under a contract it did not begin under.
The version is not user-configurable.

**Validation.** Enabling web search on a model or flow that cannot serve it, or
with a setting the flow cannot honor, is a hard error raised by the adapter
before the request goes out.
Silently dropping it would let an A/B run compare a model with search against
one without while claiming to compare models.
An account that cannot search is reported as such, not as a generic rejection:
Anthropic answers a request from an organization with web search disabled with a
400 that says so, and on the ACP flow an inventory missing only `WebSearch`
means the login does not offer it.

**Response.** Stream translation becomes stateful per content block.
A `server_tool_use` block opens a hosted-activity part.
Its query arrives as `input_json_delta`, which `map_content_delta` today turns
into client tool-call arguments; that path also marks the turn as having
requested client tools, which disables max-token chaining.
The result arrives as one complete `web_search_tool_result` block, so the
adapter keeps the activity open across its `ContentBlockStop`, which `map_event`
flushes unconditionally today, and emits it with the native payload once the
result lands.
The user sees the activity line stay up while the search runs.
Anthropic documents the result block as complete for the pinned version; a
version that streamed results would be adopted deliberately, with its own
accumulation.

**Replay.** `convert_events` reconstitutes both native blocks from the persisted
payload, `server_tool_use` and `web_search_tool_result`, with
`encrypted_content` byte-identical, since Anthropic rejects a modified or
missing value.
An activity whose payload is absent, or written by another flow, has neither
block emitted.
A `started` activity is replayed only under the continuation rule below.

**`pause_turn`.** A paused turn is continued by resending the request with the
paused assistant message appended unchanged and the same tools, until another
stop reason arrives, within a bounded number of continuations.
It cannot reuse max-token chaining: `chain` appends a user message asking the
model to continue, and `create_request` rewrites trailing thinking as text and
appends a synthetic continue message for models without prefill.
Anthropic rejects a user text block after an unfinished `server_tool_use`.
The continuation therefore runs inside the adapter's request loop and bypasses
both.
Max-token chaining still applies within a turn that contains searches, since
Anthropic accepts the user text it appends after completed search blocks, but
`should_chain` refuses while a search is pending.
A turn that stops at `max_tokens` with a search pending ends there, and the next
request abandons the search.

Every one of these lives in `anthropic.rs`.
Core sees only the normalized hosted-activity event from [RFD 118].

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

A `started` activity is replayed natively only when the request continues the
tool cycle that left it pending, and the turn has search budget left.
The request continues the cycle when the activity is in the last assistant
message and every event after it answers that message's client tool calls.
The adapter then keeps declaring the tool, even when [RFD 078] changed the
declaration between cycles, because the transcript depends on it.

Every other request abandons the activity:

- **Budget spent.** No `max_uses` admits the pending search without admitting it
  past the budget.
- **Interrupted turn.** The user stops the turn after the client tools ran, then
  sends a new message.
  `convert_events` merges consecutive user-role events, so replaying the pending
  block would put text after the `tool_result`, which Anthropic rejects.
- **Interrupted pause.** A `pause_turn` continuation that did not finish is not
  resumed from a rebuilt request.
- **Flow or provider switched.** [RFD 078] can change the active model between
  cycles, and the `auth` chain can change the flow.
  The new adapter or flow preserves and ignores a payload format it did not
  write ([RFD 118]'s contract).

Abandoning omits the pending `server_tool_use` from the native request.
Nothing is written: the activity stays `started` with no terminal transition,
which [RFD 118] defines as begun and never finished, and since the rule reads
only the stream, every later request reaches the same decision.
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

### ACP flow: Claude Code's `WebSearch`

The ACP flow cannot declare a server tool.
Claude Code builds the API request, and its options take built-in tool names,
not tool definitions.
Its own `WebSearch` tool runs each search in a separate request on a smaller
model (Claude Haiku 4.5 on Claude Code 2.1.280), and returns that model's
summary of the results with their titles and URLs.
Enabling web search on this flow enables that tool.

The API sees `WebSearch` as a client tool, executed by Claude Code.
JP sees what it sees on the HTTP flows: work the provider adapter's runtime runs
and JP observes.
It is recorded as a `HostedActivity`, never as a `ToolCallRequest`.

- **Request.** `options.tools` names `WebSearch`, and the inventory JP checks
  against Claude Code's `init` message includes it.
  It is the only native tool this flow enables, and it comes from typed
  configuration; [RFD 110]'s rule that native side-effectful tools stay disabled
  is unchanged.
- **Permission.** Claude Code asks before each `WebSearch` call in its default
  permission mode, which the ACP flow runs it in, and the qualified adapter
  sends that as `session/request_permission`, with the same allow-once option JP
  selects for its own tools.
  JP allows it while the turn's `max_searches` budget remains and denies it once
  the budget is spent.
  The permission path rejects any tool that is not a JP tool today, and for this
  one it must not emit a Tool Call.
- **Response.** The `WebSearch` tool-use block arrives on the SDK stream JP
  already reads, which today ignores tool-use blocks for tools JP did not
  configure.
  Its result arrives in the SDK user message, which the qualified adapter
  forwards whole.
  Beside the model-visible `tool_result`, which is formatted text, the message
  carries a structured `tool_use_result`: the `query`, a `searchCount`, and
  `results` holding each backend search's links as `{title, url}`, followed by
  the summary.
  JP's typed SDK message drops that field today.
  The adapter records the query from the tool-use block, the sources from
  `tool_use_result`, and the `tool_result` text in the payload.
  The payload format is Claude Code's, versioned by the qualified runtime.
- **Replay.** The transcript encoder emits a `WebSearch` `tool_use` and its
  `tool_result`, with the text the model saw, from the payload.
  The `mcp__jp__` renaming in `PreparedRequest::new` applies to JP tools only.
  Only completed activities are replayed: Claude Code finishes a search before
  the model continues, so a `started` one was interrupted.
- **Decoding.** Claude Code does not forward the stream of its nested search
  request.
  The SDK stream carries only the `text` and `tool_use` blocks JP already
  decodes, so this flow needs none of the block types [RFD 118] phase 1 adds.

Domain lists cannot be honored here.
Claude Code's `WebSearch` takes `allowed_domains` and `blocked_domains` from the
model on each call, which sets them for its own purposes, and its permission
rule accepts no domain specifier, so JP has nowhere to impose a list.
Setting either list makes an ACP request a hard error before it starts, by the
Validation rule: ignoring a blocked domain is a policy breach, not a degraded
search.

| Setting                              | API key and direct                        | ACP                          |
| ------------------------------------ | ----------------------------------------- | ---------------------------- |
| `enable`                             | Server tool declared                      | `WebSearch` enabled          |
| `max_searches`                       | Each request's `max_uses` from the budget | Permission denied once spent |
| `allowed_domains`, `blocked_domains` | Native tool fields                        | Hard error                   |

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

**Sources are a release gate, not a follow-up.** Anthropic, OpenAI, and Google
all impose attribution or display obligations on search-backed output.
Rendering search-backed assistant text without its sources is not an incomplete
feature, it is a non-compliant one.
On the HTTP flows the API anchors citations in the assistant text, and the typed
attributions from [RFD 118] render inline or as a source list.
On the ACP flow there are no anchored citations, so JP renders the activity's
`sources` under the turn.
Web rendering in the `serve-web` plugin follows the same rule.

Hosted-activity spend is shown as the number of searches in the turn, counted
from the recorded `HostedActivity` events.
[RFD 118] persists no usage counters, so the provider's own count
(`usage.server_tool_use.web_search_requests`) is a diagnostic, not a display
source.

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

**Domain lists are not available on the ACP flow.** Search itself has parity;
this refinement does not.
A user who needs a blocked domain and lists a subscription in `auth` gets an
error on that flow, and has to choose between the list and the subscription.

**A search is a different unit on each flow.** On the ACP flow one `WebSearch`
call can run up to eight backend searches, which Claude Code reports as
`searchCount`.
`max_searches` counts calls, because a call is what JP can allow or deny; the
calls checked while writing this ran one search each.
The model there reads a smaller model's summary and the result titles and URLs,
not the page content the HTTP flows replay.

**Switching flows loses search context.** When the `auth` chain falls back
mid-conversation, searches written by the other flow keep their portable
projection, the answer and its sources, but the model no longer sees their
results.
With search enabled, it may search again for what it already found.

**What a subscription search costs is opaque.** Each `WebSearch` call runs its
own request on the smaller model, about 15,000 input tokens when this was
checked, and Claude Code reports that request's tokens and search count in the
result's per-model usage.
Anthropic reports no allowance consumption per request, so JP can show how many
searches a turn ran and the tokens they used, but not what they cost a
subscription.
How the allowance prices searches is Anthropic's policy and can change.

**Per-provider configuration duplicates intent.** Wanting web search on both
Anthropic and OpenAI means two config blocks.
The knobs genuinely differ, so some duplication is inherent, but the `enable`
flag is duplicated for no reason other than placement.

## Alternatives

**`ToolSource::Llm` plus a no-op executor.** Rejected.
`llm.bash` and `llm.web_search` would differ in who executes, so one source
variant cannot describe both; and it requires exceptions in the turn loop, the
execution plan, and orphan sanitization.

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
This flips if keeping one capability enabled across a credential fallback ever
requires the same `enable` in two provider blocks.

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

**Ship on the HTTP flows first and the ACP flow later.** Rejected: see "Billing
must not decide capability".
The HTTP phases can merge first, but until the ACP phase lands, enabling search
on an ACP request is a hard error, and the feature is not announced.

**A JP tool that calls the Messages API with web search.** Would give search to
every provider and flow alike.
Rejected: it needs an API credential, which a subscription-only setup does not
have, and it is the capability routing this RFD leaves out.

**Sending search requests through the direct flow.** Rejected: [RFD 110] never
selects the direct flow automatically, because it carries account-policy risk
the user did not choose.

**Prompting per call on the ACP flow.** Possible, since Claude Code asks before
each `WebSearch`.
Rejected: the HTTP flows cannot prompt, and consent that depends on the flow is
the inconsistency this RFD avoids.

**Enforcing domain lists through permission answers on the ACP flow.** JP sees
each `WebSearch` call's input when Claude Code asks, so it could deny calls
whose lists do not match the configuration.
Rejected: the model chooses those lists and does not know the policy, so
enforcement would mean instructing it through the prompt and denying until it
complies, and an ACP permission answer cannot rewrite the input.

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
  OpenAI follows as the test that the boundary generalizes.
- **Google Search grounding.** Google requires its Search Suggestions, HTML and
  CSS, to be displayed with every grounded answer, and [RFD 118] leaves that
  display content to the RFD that adds Google.
  Whether a terminal client can meet the requirement decides whether Google
  search fits JP at all.
- **Web search versions with dynamic filtering** (`web_search_20260209` and
  later), and `user_location`.
- **Claude Code native tools other than `WebSearch`** on the ACP flow, including
  `WebFetch`.

## Risks and Open Questions

- **Claude Code's `WebSearch` is observed, not documented.** The nested request
  and its model, the shape of `tool_use_result`, and `searchCount` are what
  Claude Code 2.1.280 does, not a published contract.
  [RFD 110]'s runtime qualification re-checks them for each qualified release,
  against the phase 4 fixture.
- **`pause_turn` is documented, not observed.** No prompt reliably produces it,
  so the continuation follows Anthropic's documentation, and phase 3 tests it
  against a constructed response.

## Implementation Plan

Requires [RFD 118] phases 1 through 4.

### Phase 1: Typed configuration

`hosted_tools` on `AnthropicConfig` with the web-search fields, defaulting to
disabled.
Schema, delta, fill, and partial support.
Independently mergeable; nothing reads it yet.

### Phase 2: HTTP request translation and validation

`convert_tools` branch pinned to `web_search_20250305`, model and setting
validation, each request's `max_uses` from the turn budget, and the two
`tool_choice` corrections.
Enabling search on an ACP request is a hard error until phase 4.
At this point the tool is declared and the model can call it, but the response
path drops the blocks.

### Phase 3: HTTP response path

Per-block stream state, deferred flush with payload attachment, `convert_events`
replay of both native blocks, the `pause_turn` continuation, and the
continuation and abandonment rules.
Web search works end to end on the API-key and direct flows, each with a
recorded fixture.
The direct flow's is the first JP request to send the server tool with a
subscription credential; Claude Code's own searches already run Anthropic's
server search tool on one.

### Phase 4: ACP flow

A recorded `WebSearch` run against the qualified adapter and runtime first, as
the fixture for the rest of the phase.
Then `WebSearch` in the options and the expected inventory, permission answers
from the budget, `tool_use_result` on the typed SDK user message, recording from
the SDK stream and user message, transcript replay, and the domain-list error.

### Phase 5: Rendering and usage

Activity display, sources in terminal and `serve-web` for both flows, and the
per-turn search count.
Release requires phases 3 through 5, per "Billing must not decide capability"
and Rendering above.

### Phase 6: OpenAI

OpenAI web search, as the test that provider-owned translation generalizes
without touching core.
Its search call and annotations already fit [RFD 118]'s model.

## References

- `crates/jp_llm/src/provider/anthropic.rs` — `convert_tools`,
  `convert_tool_choice`, `convert_events`, `map_event`, chaining
- `crates/jp_llm/src/provider/anthropic/acp/options.rs` — Claude Code tool
  options
- `crates/jp_llm/src/provider/anthropic/acp/protocol.rs` — inventory check,
  permission answers, SDK message translation
- `crates/jp_llm/src/provider/anthropic/acp/transcript.rs` — native transcript
  encoding
- `crates/jp_config/src/providers/llm/anthropic.rs` — `AnthropicConfig`
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/server-tools>
- <https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool>
- <https://code.claude.com/docs/en/tools-reference#websearch-tool-behavior>

[#208]: https://github.com/dcdpr/jp/issues/208
[RFD 078]: 078-tool-config-mutation.md
[RFD 090]: 090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[RFD 118]: 118-provider-response-artifacts.md
