# RFD 105: Mid-Conversation Operator Directives

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-26
- **Summary**: Delivers mid-conversation prompt, tool, reasoning, and schema
  changes positionally to preserve prompt caches and replayed reasoning.

## Summary

JP delivers mid-conversation system prompt sections, tool availability,
reasoning effort, and response format changes as positional items anchored in
the conversation stream rather than rewriting the request prefix.
On supported Anthropic and OpenAI models, this preserves the prompt cache and
the replayed reasoning across configuration changes; other providers and
unsupported models flatten the directives into the top-level request fields.

## Motivation

Anthropic hashes the request prefix in order: `tools`, then `system`, then
`messages`.
A cache hit requires that prefix to match byte for byte.
Any edit to the `tools` array or the top-level `system` field, such as appending
a sentence or enabling a tool, changes the hash and invalidates the cached
prefix for the entire conversation.

JP modifies both fields mid-conversation:

- `jp config set --conversation assistant.system_prompt_sections...`
- `--cfg` overrides on a query
- `jp q --tool c` / `--no-tool c`
- The `has_tools`-gated "Tool Usage" section appearing or disappearing
- Access grant revocation ([RFD 076]) and tool config mutation ([RFD 078])

Each of these changes invalidates the entire cached prefix, even dozens of turns
into a conversation.

Anthropic provides two mechanisms in `messages` that leave the request prefix
untouched:

- A `{"role": "system"}` message in `messages` delivers operator-level
  instructions without modifying the top-level `system` field.
- Inside that message, `tool_addition` and `tool_removal` blocks change which
  tools are offered without modifying the top-level `tools` array.

Both mechanisms are **positional**: the change applies from that point in the
conversation onward.
Because JP already records configuration changes positionally as `ConfigDelta`
entries between events in the conversation stream ([RFD 054]), the stream
already captures where each change belongs.

A rewritten prefix costs more than a cache miss.
Anthropic rejects replayed `thinking` blocks whose `system` or `tools` changed
since they were generated, and JP repairs the conversation after the 400 by
discarding that reasoning (T-0q1k0j4).
From GPT-6.2 on, OpenAI silently removes every replayed reasoning item that
follows an edit to the prefix, reported as `prefix_mismatch` in the response's
`input_transformations` ([keep the prefix unchanged][openai-prefix]).
The model must then repeat that reasoning, increasing token usage and
potentially degrading response quality.

OpenAI treats more top-level fields as part of the prefix than Anthropic does.
Changing the request-level `reasoning.effort` or `text.format` rewrites hidden
system instructions, so JP edits the prefix when
`assistant.model.parameters.reasoning` or `--schema` changes between turns.
OpenAI provides positional mechanisms for all four kinds of change:

- A `developer` message in `input` delivers new instructions.
- An `additional_tools` input item offers tools from its position onward.
- `tool_choice: { "type": "allowed_tools" }` restricts which offered tools are
  callable without changing the `tools` array.
- A `configuration_update` input item changes `reasoning.effort` and
  `text.format` from its position onward.

## Design

### CLI Behavior

The CLI interface does not change.
`jp config set --conversation`, `--cfg`, `--tool`, and `--no-tool` keep their
existing behavior, without invalidating the prompt cache on supported models.

### Operator Directives

An **operator directive** is a mid-conversation change to system prompt content,
tool availability, reasoning effort, or response format that takes effect at a
specific position in the conversation stream.

Directives are derived at request time rather than stored as stream events or
durable state.
At any stream position, the active system sections, enabled tools, and reasoning
effort are a function of the merged configuration at that position, and the
response format comes from the request's schema; a directive is the diff between
two consecutive positions.
`ConversationStream::iter()` already yields `ConversationEventWithConfigRef`
with the accumulated `PartialAppConfig` per event, which
`anthropic::convert_events` already reads.

A directive carries one or more blocks:

```rust
pub enum DirectiveBlock {
    /// A rendered system prompt section.
    Text(String),
    /// Offer a tool from this point onward.
    ToolAddition(String),
    /// Withdraw a tool from this point onward.
    ToolRemoval(String),
    /// Reason at this effort from this point onward.
    ReasoningEffort(ReasoningEffort),
    /// Constrain responses to this schema from this point onward, or lift the
    /// constraint with `None`.
    ResponseFormat(Option<Map<String, Value>>),
}
```

`Text` blocks are derived from `assistant.system_prompt_sections` and
`assistant.instructions`.
`ToolAddition` and `ToolRemoval` blocks are derived from tool enablement state
in `conversation.tools` ([RFD 081]).
`ReasoningEffort` blocks are derived from
`assistant.model.parameters.reasoning`, resolved against the model's effort
levels.
`ResponseFormat` blocks are derived from `ChatRequest::schema` rather than from
a `ConfigDelta`: a schema belongs to one request, so a directive is emitted
before every `ChatRequest` whose schema differs from the previous one's.

### Stable Tool Catalogue

For tool directives to preserve the prefix hash, the top-level `tools` array
must remain constant across turns.
Instead of listing only the currently enabled tools, the `tools` array holds
**every tool in the catalogue** regardless of enablement, with `defer_loading:
true` on all but one anchor tool.
The array is computed once per conversation and never changes.
Tool availability is expressed entirely through `tool_addition` and
`tool_removal` blocks, starting with an initial directive that enables the
configured tool set.

Two Anthropic API constraints shape this layout:

- The API rejects requests where every tool has `defer_loading: true`, so one
  anchor tool remains non-deferred (`defer_loading: false`).
- A tool with `defer_loading: true` cannot carry `cache_control`, so the tool
  cache breakpoint moves to the non-deferred anchor tool.

Deferral is an internal transport mechanism; there is no user-facing `defer`
setting, and users continue to configure tool availability through enablement.

### Directive Placement

The Anthropic API constrains where a system message may appear in `messages`:

- It cannot be the first message in `messages`.
- It must immediately follow a `user` turn (including one carrying `tool_result`
  blocks) or an `assistant` turn ending in a server tool result.
- It must be either the last message in `messages` or immediately followed by an
  `assistant` turn.
- It cannot sit between a `tool_use` block and its corresponding `tool_result`.

Because a `ConfigDelta` can sit anywhere in the stream, including inside a
tool-call pair, the provider snaps each directive **forward** to the next valid
boundary.
The initial directive snaps to immediately after the conversation's first user
message.
Consecutive directives that snap to the same boundary merge into a single system
message, matching how the API treats consecutive system messages.

### Provider Boundary

`ThreadParts::system_parts: Vec<String>` is replaced by a prelude plus an
ordered item stream:

```rust
pub struct ThreadParts {
    /// System content applying from the first turn.
    pub prelude: Vec<String>,
    /// Events interleaved with the directives anchored between them.
    pub items: Vec<ThreadItem>,
    pub attachments: Vec<Attachment>,
}

pub enum ThreadItem {
    Directive(Vec<DirectiveBlock>),
    Event(ConversationEvent),
}
```

`jp_conversation` computes the directives and their stream positions once; each
provider decides how to serialize `prelude` and `items` into its request format.

### OpenAI Serialization

The OpenAI provider serializes `prelude` as the leading `system` message, and
serializes each directive as follows:

| Block                                | Serialized as                                                    | Position                      |
| ------------------------------------ | ---------------------------------------------------------------- | ----------------------------- |
| `Text`                               | A `developer` message, with a cache breakpoint on its last block | Snapped position              |
| `ToolAddition`                       | An `additional_tools` item holding the tool's definition         | Snapped position              |
| `ToolRemoval`                        | The tool's absence from `tool_choice` (`allowed_tools`)          | Request-level                 |
| `ReasoningEffort` / `ResponseFormat` | A `configuration_update` item                                    | Before the next `ChatRequest` |

`additional_tools` items take `role: developer`.
`allowed_tools` lists every offered tool that is still enabled.
`configuration_update` carries `reasoning.effort`, `text.format`, or both.

Three request-level fields are pinned for the life of the conversation, so the
prefix never changes:

- `tools` holds the tools enabled at the conversation's first position.
  A tool enabled later reaches the model through `additional_tools` and stays
  offered; disabling and re-enabling it changes only `tool_choice`.
- `reasoning.effort` is the effort at the first position.
- `text.format` is the format of the first `ChatRequest`.

`tool_choice` is the one request-level field that changes.
OpenAI documents `allowed_tools` as the cache-preserving way to restrict tools
([manage tools with append-only updates][openai-tools]).
A forced tool call replaces `allowed_tools` with `{ "type": "function" }` for
that request, which names an enabled (and therefore already offered) tool.

`Text` directives carry an explicit cache breakpoint because, without one, a
`developer` message after the leading system message is not a cache lookup
boundary.

`configuration_update` must appear before the next user message, so effort and
format directives snap forward to the next `ChatRequest`.
`Text` and tool directives snap to the same boundary, which prevents directives
from landing between a function call and its output; OpenAI documents no
stricter placement constraints.

OpenAI has no counterpart to Anthropic's `defer_loading`-based catalogue outside
tool search, so the stable catalogue array described above applies to Anthropic
only.

### Provider Fallback

Providers other than Anthropic and OpenAI, and models whose gate is not
satisfied, flatten directives into the top-level request fields:

| Payload                              | Flattens to                                                                                    | Trade-off                                                          |
| ------------------------------------ | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `Text`                               | Concatenated into the top-level `system` field                                                 | Loses ordering and authority-timing; the instruction still applies |
| `ToolAddition` / `ToolRemoval`       | The effective tool set computed per request, sent as the `tools` array with no `defer_loading` | Loses cache preservation on change; the tool set is identical      |
| `ReasoningEffort` / `ResponseFormat` | The request-level effort and format of the current request                                     | Loses cache and reasoning preservation on change                   |

Anthropic flattens `ReasoningEffort` and `ResponseFormat` on every model, since
it offers no positional form for either.

### Feature Gates

Text and tool directives are gated independently:

- **Text directives** require `MID_CONVERSATION_SYSTEM` in
  `ModelDetails::features` (Fable 5, Mythos 5, Opus 4.8, Opus 5).
- **Tool directives** require both `MID_CONVERSATION_SYSTEM` and
  `mid-conversation-tool-changes-2026-07-01` in
  `providers.llm.anthropic.beta_headers`.
- Any directive whose gate is not satisfied falls back to flattening.

For example, a user on Opus 5 without the beta header gets positional text
directives and a flattened `tools` array.

On OpenAI, the gates are catalog features in `ModelDetails::features`:

- **Text and tool directives** require `POSITIONAL_DIRECTIVES`.
- **Effort and format directives** require `CONFIGURATION_UPDATE`, and are
  flattened on any request with `reasoning.mode = "pro"`, since OpenAI supports
  configuration updates only in standard, single-agent mode.

OpenAI documents `configuration_update` for "supported GPT-6 and later models"
without listing them, and does not document which models accept
`additional_tools`.
Both flags start on the GPT-6.2 models, where a prefix edit drops replayed
reasoning, and will be enabled on other GPT-6 models after live verification.

## Drawbacks

- **Larger request payloads.** Every tool definition in the catalogue is sent on
  every request, including tools the user never enabled.
  Because deferred definitions are excluded from the system-prompt prefix, this
  increases request bandwidth (by tens of kilobytes for large MCP setups)
  without consuming context window tokens.
- **Reliance on a beta API for tool changes.** Cache-preserving tool changes
  depend on the `mid-conversation-tool-changes-2026-07-01` beta.
  If that API changes shape, tool directives fall back to rewriting the `tools`
  prefix, degrading to a cache miss rather than failing the request.
- **Broader provider boundary.** `ThreadItem` adds a concept to the shared
  provider boundary that five of the seven providers immediately flatten.
- **Withdrawn tools still cost context on OpenAI.** A disabled tool stays in
  `tools` or in its `additional_tools` item, and its definition is billed on
  every request until the conversation is compacted.
  Flattening removes the definition, but invalidates the prompt cache and drops
  subsequent reasoning items.
- **Two fork extensions.** The `openai_responses` fork needs the
  `additional_tools` and `configuration_update` input items and the
  `allowed_tools` tool choice, alongside the `async-anthropic` changes.

## Alternatives

### Model-Initiated Discovery via `tool_reference`

JP could expose a `search_tools` builtin that returns `tool_reference` blocks in
a `tool_result` for the API to expand.
Although `tool_reference` works on ten models rather than four and needs no beta
header, it cannot withdraw a tool and solves a different problem: selecting
tools on demand from a large uncurated catalogue.
This is deferred to a separate RFD.

Under this RFD's model, the initial directive offers every enabled tool and
withdraws every disabled one, so referencing an enabled tool is a no-op and
referencing a disabled one would bypass the user's configuration.
Supporting `tool_reference` will require a third enablement state: present in
the catalogue and permitted, but not offered until the model requests it.

### Unify on `tool_reference` Without Directives

Relying solely on `tool_reference` was rejected because the API has no block to
unload a referenced tool, so disabling a tool mid-conversation would still
rewrite the `tools` prefix.

### System Content as Stream Events

Storing system messages as first-class `EventKind::SystemMessage` events would
record their ordering directly in the stream.
This was rejected because it duplicates configuration as a second source of
truth, changes the durable on-disk format, and conflicts with `jp config set
--conversation`.

### Keep Only Enabled Tools in the `tools` Array

Sending only currently enabled tools in the top-level `tools` array was rejected
because any tool enablement change mutates that array and invalidates the prompt
cache.

### Replace `assistant.system_prompt` with Sections

Because `assistant.system_prompt` is a single merged string without addressable
section IDs, changes to it cannot be diffed into directives.
Replacing it with `assistant.system_prompt_sections` requires a configuration
migration and a rewrite of stored conversation data, so it is deferred to a
follow-up RFD.

Deferring this migration has a significant cost: twelve persona files and
`knowledge/software-laws.toml` configure `[assistant.system_prompt]`, and four
justfile recipes apply a persona to an active conversation with
`--cfg=personas/<name>`, which records a `ConfigDelta`.
Until `assistant.system_prompt` is migrated to sections, applying a persona
mid-conversation still mutates the top-level `system` prompt and causes a full
cache miss.

## Non-Goals

- **Tool search and model-initiated discovery.** This RFD does not add
  `search_tools`, `tool_reference`, or `referenced_tools` on `ToolCallResponse`.
- **Removing `assistant.system_prompt`.** `assistant.system_prompt` remains in
  the prelude and is never diffed into a directive; a follow-up RFD will
  collapse it into the prompt section list.
- **A user-facing `defer` setting.** Deferral is an internal mechanism.
- **Per-position derivation of the "Tool Usage" section.** The "Tool Usage"
  section is computed once from the final resolved configuration and placed in
  the prelude.
- **Model switches.** Moving a conversation to another model family, or to a
  less capable model, drops its reasoning on OpenAI (`model_mismatch`) and is
  out of reach of any directive.
- **Attachments and compaction.** Both edit the prefix and are covered under
  Risks and Open Questions; neither is changed here.

## Risks and Open Questions

### Catalogue Reproducibility

The `tools` array must be byte-identical across requests to preserve the prefix
hash.
Tool definitions come from three tiers:

| Source    | Definition lives in                                    | Reproducible from stream?  |
| --------- | ------------------------------------------------------ | -------------------------- |
| `local`   | `ToolConfig`, durable in `base_config.json` and deltas | Yes                        |
| `builtin` | `ToolConfig`, defaults from the JP binary              | For a given binary version |
| `mcp`     | Fetched from the server each turn                      | No                         |

The MCP tier is not reproducible from the stream: a server that is offline,
slow, or returns tools in a different order changes the `tools` array.
Worse, a `tool_removal` naming a tool absent from the `tools` array causes an
HTTP 400 error.

This RFD scopes cache stability to the `local` and `builtin` tiers.
MCP tool changes invalidate the cache, and directives naming an unresolvable
tool are stripped with a warning.
Persisting the tool catalogue in the conversation stream would close this gap in
a follow-up RFD.

### Anchoring Stability Under Compaction

`projection::apply` passes `ConfigDelta` entries through verbatim and in
position, and `assign_turn_indices` assigns them the current turn index without
opening a turn, so directives survive compaction.
Collapsing a turn into a synthetic summary can shift a directive's snap
boundary, but adding a compaction already rewrites message history and
invalidates the cache.
Absent a new compaction, repeated requests must anchor directives at
byte-identical positions.

### Context Window Impact of `tool_removal`

`tool_removal` preserves the cache by leaving the `tools` array untouched; per
[the caching docs][anthropic-tool-caching], modifying tool definitions
invalidates the entire cache, whereas "deferred tools are not included in the
system-prompt prefix".
Because a withdrawn tool's definition remains in the `tools` prefix,
`defer_loading: true` is the only mechanism that keeps unused definitions out of
the prefix, which is why the catalogue array defers every tool except the
anchor.

To confirm that presence in the cached prefix matches billed context tokens,
Phase 3 is gated on a quick check comparing `usage.input_tokens` across three
requests: both tools declared non-deferred, both declared non-deferred plus a
`tool_removal`, and one declared with `defer_loading: true`.
The first two should match and the third should drop.

### Initial Directive Scope on the First Response

Because placement rules force the initial directive after the first user
message, we must verify that it governs the assistant's first response.
We can test this by sending a prompt that requests a tool disabled by a
`tool_removal` in the initial directive and confirming that the model does not
call it.

### Truncation and Configuration Updates on OpenAI

OpenAI rejects histories containing `configuration_update` items when
`truncation` is `auto`, and the turn loop asks for `Truncation::Allowed`, which
the OpenAI provider sends as `auto`.
A request carrying an effort or format directive must therefore send
`truncation: disabled`.
Automatic truncation drops items from the middle of the conversation, which
itself mutates the prefix and drops reasoning.
However, disabling truncation means a conversation that exceeds the context
window fails with `ContextWindowExceeded` until JP manages context window
fitting directly (T-0n0pex2).

It is unconfirmed whether `additional_tools` items and later `developer`
messages trip the same rejection.
Phase 6 checks this before any directive reaches a request that allows
truncation.

### Compaction and Attachments Edit the Prefix

A compaction summary ([RFD 064]) replaces the turns it covers in place, so the
kept turns follow an edited prefix and their reasoning is dropped on GPT-6.2.
OpenAI advises placing a home-grown compaction item at the end of the context
instead ([keep the prefix unchanged][openai-prefix]); deleting whole items
without editing the remaining ones does not by itself trigger a removal.
Attachments are sent in one user message ahead of the conversation, so an
attachment whose content changes between requests has the same effect.
Both deserve their own design; T-1mnmqbx's reporting of `input_transformations`
shows how often each one happens.

### Cache Breakpoint Budget

`MAX_EXPLICIT_CACHE_CONTROL_COUNT` is 3, allocated to system, then documents,
then tools.
Moving system content into `messages` shrinks the prelude, and the tools
breakpoint must move to the non-deferred anchor tool.
The breakpoint allocation comment in `anthropic.rs` needs updating to match.

## Implementation Plan

1. **Phase 1: Directive derivation and provider boundary.** Add
   `DirectiveBlock`, `ThreadItem`, and the updated `ThreadParts` in
   `jp_conversation`, and derive text directives by diffing section sets between
   consecutive config positions.
   Flatten directives in all seven providers so behavior and existing tests
   remain unchanged.
   Independently mergeable.
2. **Phase 2: Anthropic text directives.** Add `MessageRole::System` and a
   system content-block list in the `async-anthropic` fork.
   Emit text directives as `role: system` messages with forward placement
   snapping, gated on `MID_CONVERSATION_SYSTEM`.
   Depends on Phase 1.
3. **Phase 3: Stable catalogue array.** Build the `tools` array from the full
   catalogue with `defer_loading: true` on all but the anchor tool, and move the
   cache breakpoint to the anchor.
   Continue flattening tool enablement until Phase 4.
   `cache_read_input_tokens` should remain unchanged and `input_tokens` should
   drop by roughly the definitions of all disabled tools.
   Depends on the `tool_removal` verification above.
4. **Phase 4: Tool directives.** Add `tool_addition` and `tool_removal` blocks
   in the `async-anthropic` fork, derive tool directives from enablement diffs,
   and emit the initial directive after the first user message, gated on
   `MID_CONVERSATION_SYSTEM` and the beta header.
   Depends on Phases 2 and 3.
5. **Phase 5: Anchoring stability tests.** Assert byte-identical directive
   anchoring across repeated requests, including after compaction and turn
   pruning, and assert that unresolvable directives are stripped rather than
   sent.
6. **Phase 6: OpenAI text and tool directives.** Add the `additional_tools`
   input item and the `allowed_tools` tool choice to the `openai_responses`
   fork.
   Pin `tools` to the first position's tool set, emit `Text` directives as
   `developer` messages and `ToolAddition` directives as `additional_tools`
   items, and derive `allowed_tools` from current enablement, gated on
   `POSITIONAL_DIRECTIVES`.
   Before enabling the flag on a model, confirm against it that a request with
   `truncation: auto` and an `additional_tools` item is accepted, and that a
   tool change produces no `prefix_mismatch` in `input_transformations`
   (T-1mnmqbx).
   Depends on Phase 1.
7. **Phase 7: OpenAI configuration updates.** Add the `configuration_update`
   input item to the fork.
   Derive `ReasoningEffort` and `ResponseFormat` directives, pin the
   request-level effort and format to their first values, emit
   `configuration_update` before the next `ChatRequest`, and send `truncation:
   disabled` on any request whose history holds one, gated on
   `CONFIGURATION_UPDATE` and standard reasoning mode.
   Depends on Phase 6.

## References

- [Mid-conversation system messages and tool changes][anthropic-midconv]
- [Tool search tool][anthropic-tool-search]: `defer_loading` semantics and the
  `tool_reference` alternative
- [Tool use with prompt caching][anthropic-tool-caching]: what invalidates the
  `tools` prefix, and why deferral is required to shrink it
- [Keep the prefix unchanged][openai-prefix]: what OpenAI counts as a prefix
  edit, and the append-only alternative for each
- [Prompt caching: manage tools with append-only updates][openai-tools]
- [Change reasoning mid-conversation][openai-config-update]:
  `configuration_update` placement and constraints
- [Reasoning item removal across requests][openai-removal]:
  `input_transformations` reasons
- [RFD 054]: config deltas positioned in the conversation stream
- [RFD 064]: compaction projection
- [RFD 076]: access grant revocation as a mid-conversation tool change
- [RFD 078]: tool config mutation as a mid-conversation tool change
- [RFD 081]: tool enablement state

[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 076]: 076-tool-access-grants.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[anthropic-midconv]: https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages
[anthropic-tool-caching]: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-use-with-prompt-caching
[anthropic-tool-search]: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool
[openai-config-update]: https://developers.openai.com/api/docs/guides/reasoning#change-reasoning-mid-conversation
[openai-prefix]: https://developers.openai.com/api/docs/guides/keep-prefix-unchanged
[openai-removal]: https://developers.openai.com/api/docs/guides/reasoning#reasoning-item-removal-across-requests
[openai-tools]: https://developers.openai.com/api/docs/guides/prompt-caching#manage-tools-with-append-only-updates
