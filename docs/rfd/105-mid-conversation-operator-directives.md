# RFD 105: Mid-Conversation Operator Directives

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-26
- **Summary**: Delivers mid-conversation system prompt and tool changes as
  positional stream messages to preserve Anthropic's prompt cache.

## Summary

JP delivers mid-conversation system prompt sections and tool availability
changes as positional messages anchored in the conversation stream rather than
rewriting the request prefix.
On supported Anthropic models, this preserves the prompt cache across
configuration changes; other providers and unsupported models flatten the
directives into the top-level system prompt and tool list.

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

## Design

### CLI Behavior

The CLI interface does not change.
`jp config set --conversation`, `--cfg`, `--tool`, and `--no-tool` keep their
existing behavior, without invalidating the prompt cache on supported models.

### Operator Directives

An **operator directive** is a change to system prompt content or tool
availability that takes effect at a specific position in the conversation
stream.

Directives are derived at request time rather than stored as stream events or
durable state.
At any stream position, the active system sections and enabled tools are a
function of the merged configuration at that position; a directive is the diff
between two consecutive positions.
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
}
```

`Text` blocks are derived from `assistant.system_prompt_sections` and
`assistant.instructions`.
`ToolAddition` and `ToolRemoval` blocks are derived from tool enablement state
in `conversation.tools` ([RFD 081]).

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

### Provider Fallback

Non-Anthropic providers and unsupported Anthropic models flatten directives into
the top-level request fields:

| Payload                        | Flattens to                                                                                    | Trade-off                                                          |
| ------------------------------ | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| `Text`                         | Concatenated into the top-level `system` field                                                 | Loses ordering and authority-timing; the instruction still applies |
| `ToolAddition` / `ToolRemoval` | The effective tool set computed per request, sent as the `tools` array with no `defer_loading` | Loses cache preservation on change; the tool set is identical      |

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
  provider boundary that six of the seven providers immediately flatten.

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

## References

- [Mid-conversation system messages and tool changes][anthropic-midconv]
- [Tool search tool][anthropic-tool-search]: `defer_loading` semantics and the
  `tool_reference` alternative
- [Tool use with prompt caching][anthropic-tool-caching]: what invalidates the
  `tools` prefix, and why deferral is required to shrink it
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
