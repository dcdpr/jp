# RFD 105: Mid-Conversation Operator Directives

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-26

## Summary

JP delivers system prompt sections and tool availability changes as content
anchored to a position in the conversation stream, rather than by rewriting the
request prefix.
On supported Anthropic models this keeps the prompt cache intact across
mid-conversation configuration changes.
Everywhere else the same content flattens into today's wire format, unchanged.

## Motivation

Anthropic hashes the request prefix in order: `tools`, then `system`, then
`messages`.
A cache hit requires that prefix to match byte for byte.
Any edit to the `tools` array or the top-level `system` field — appending one
sentence, enabling one tool — produces a different hash and re-processes the
entire conversation.

JP edits both routinely, mid-conversation:

- `jp config set --conversation assistant.system_prompt_sections...`
- `--cfg` overrides on a query
- `jp q --tool c` / `--no-tool c`
- The `has_tools`-gated "Tool Usage" section appearing or disappearing
- Access grant revocation ([RFD 076]) and tool config mutation ([RFD 078])

Every one of those currently costs a full cache miss on a conversation that may
be dozens of turns long.

Anthropic added two mechanisms for exactly this.
A `{"role": "system"}` message placed in `messages` carries operator-level
instructions without touching the prefix.
Inside that message, `tool_addition` and `tool_removal` blocks change which
tools are offered, again without touching the `tools` array.

Both mechanisms share one property that shapes this design: they are
**positional**.
The change applies from that point in the conversation onward.
JP already stores configuration positionally — `ConfigDelta` entries sit
between events in the conversation stream ([RFD 054]) — so the information
needed to place them is already there.

## Design

### What the user sees

Nothing changes in the CLI.
`jp config set --conversation`, `--cfg`, `--tool`, and `--no-tool` behave
exactly as they do today.
On a supported model they stop costing a full cache miss.

### Operator directive

An **operator directive** is a change to operator-level state — system prompt
content or tool availability — that takes effect at a specific position in the
conversation stream.

A directive is derived, not stored: the set of system sections and the set of
available tools at any stream position is a function of the merged config at
that position.
The diff between consecutive positions *is* the directive.
No new event type, no new durable state.

`ConversationStream::iter()` already yields `ConversationEventWithConfigRef`,
carrying the accumulated `PartialAppConfig` per event, and
`anthropic::convert_events` already reads it.

### Two payloads, one anchor

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

Text blocks come from `assistant.system_prompt_sections` and
`assistant.instructions`.
Tool blocks come from `conversation.tools` enablement state ([RFD 081]).

### The catalogue array is stable

This is the part that makes tool directives work, and it inverts today's model.

The `tools` array holds **every tool in the catalogue**, regardless of
enablement, with `defer_loading: true` on all but one anchor tool.
It is computed once per conversation and never changes.
Availability is expressed entirely through `tool_addition` and `tool_removal`
blocks, starting with an initial directive that offers whatever the user
enabled.

Two API constraints shape this:

- At least one tool must have `defer_loading: false`, or the request is
  rejected.
  One non-deferred anchor tool satisfies it.
- A tool with `defer_loading: true` cannot carry `cache_control`.
  The cache breakpoint moves to the anchor tool, which is non-deferred anyway.

Deferral is a mechanism here, not a feature.
There is no user-facing `defer` setting; the user expresses intent through
enablement as they always have.

### Placement

Anthropic constrains where a system message may sit:

- Not first in `messages`.
- Must immediately follow a `user` turn (including one carrying `tool_result`
  blocks) or an `assistant` turn ending in a server tool result.
- Must be last in `messages` or immediately followed by an `assistant` turn.
- Never between a `tool_use` block and its `tool_result`.

A `ConfigDelta` can sit anywhere in the stream, including inside a tool-call
pair.
The provider snaps each directive **forward** to the next legal boundary.
Consecutive directives that snap to the same boundary merge into one system
message; the API treats consecutive system messages as a single section anyway.

The initial directive snaps to just after the conversation's first user message.

### Provider boundary

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

Deciding *what* content exists and *where* happens once, in `jp_conversation`.
Deciding *how* to render it is each provider's choice.

### Fallback

Unsupported models and every non-Anthropic provider flatten.
Two flattening rules, lossy in different ways, both producing today's behavior:

| Payload                        | Flattens to                                                                                    | What is lost                                                 |
| ------------------------------ | ---------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `Text`                         | Concatenated into the top-level system field                                                   | Ordering and authority-timing; the instruction still applies |
| `ToolAddition` / `ToolRemoval` | The effective tool set computed per request, sent as the `tools` array with no `defer_loading` | Cache preservation; the tool set is identical                |

### Gates

Three independent checks:

- Text directives: a `MID_CONVERSATION_SYSTEM` entry in `ModelDetails::features`
  (Fable 5, Mythos 5, Opus 4.8, Opus 5).
- Tool directives: that feature **and**
  `mid-conversation-tool-changes-2026-07-01` in
  `providers.llm.anthropic.beta_headers`.
- Everything else flattens.

A user on Opus 5 without the beta header gets working text directives and a
flattened tool array.
That is a correct degradation, not a bug.

## Drawbacks

**The catalogue array grows the request.** Every tool definition is sent on
every request, including tools the user never enabled.
This costs bandwidth, not context — deferred definitions stay out of the
prefix.
For a large MCP setup the request body grows by tens of kilobytes.

**A beta API becomes the only cache-preserving path for tool changes.** If
`mid-conversation-tool-changes-2026-07-01` changes shape, tool directives break
and fall back to prefix rewriting.
The failure is a cache miss, not an error, which limits the damage.

**One more concept in the provider boundary.** `ThreadItem` is a real addition
to a boundary that seven providers cross.
Six of them only ever flatten it.

## Alternatives

**Model-initiated discovery via `tool_reference`.** JP could expose a
`search_tools` builtin returning `tool_reference` blocks in a `tool_result`,
which the API expands.
That mechanism works on ten models rather than four and needs no beta header,
but it cannot *withdraw* a tool, and it answers a different question: which
tools should the model pick from a large uncurated catalogue.
Deferred to its own RFD.

**Unify on `tool_reference` and drop directives.** Rejected: there is no
"unreference" block, so mid-conversation disabling would still rewrite the
prefix.

**System content as first-class stream events.** An `EventKind::SystemMessage`
would make ordering intrinsic.
Rejected: it duplicates config as a source of truth for the same content,
changes the durable on-disk format, and collides with `jp config set
--conversation`.

**Keep the `tools` array as "enabled right now".** Rejected: that array *is* the
prefix change this RFD exists to avoid.

**Remove `assistant.system_prompt` in favour of sections.** A merged string has
no addressable identity, so it cannot be diffed into directives.
This is the right change and it is deferred, not dismissed: it needs a config
migration and a rewrite of stored conversation data, which would double the size
of this RFD.

Deferring it has a real cost, worth naming rather than glossing.
Twelve persona files and `knowledge/software-laws.toml` compose their identity
through `[assistant.system_prompt]`, and four justfile recipes apply a persona
to an *already running* conversation with `--cfg=personas/<name>`, which lands
as a config delta.
So until the field is gone, the single most common operator-level change JP
makes is the one shape this RFD cannot help with, and a persona applied
mid-conversation still costs a full cache miss.

## Non-Goals

- **Tool search and model-initiated discovery.** No `search_tools`, no
  `tool_reference`, no `referenced_tools` on `ToolCallResponse`.
- **Removing `assistant.system_prompt`.** It stays for now, treated as prelude
  content that never becomes a directive.
  A follow-up RFD collapses it into the prompt list; see Alternatives for what
  that costs to defer.
- **A user-facing `defer` setting.** Deferral is internal.
- **Per-position derivation of the "Tool Usage" section.** It is computed once
  from the final resolved config and lives in the prelude, as today.

## Risks and Open Questions

**Catalogue reproducibility (first-order).** The array must be byte-identical
across requests or the prefix hash changes and the feature is pointless.
Tool definitions come from three tiers:

| Source    | Definition lives in                                    | Reproducible from stream?  |
| --------- | ------------------------------------------------------ | -------------------------- |
| `local`   | `ToolConfig`, durable in `base_config.json` and deltas | Yes                        |
| `builtin` | `ToolConfig`, defaults from the JP binary              | For a given binary version |
| `mcp`     | Fetched from the server each turn                      | No                         |

Only the MCP tier is a problem, and it is a real one: a server that is offline,
slow, or returns tools in a different order perturbs the array.
Worse, a `tool_removal` naming a tool absent from the array is a hard 400.

This RFD scopes the guarantee to the first two tiers.
MCP churn busts the cache; directives naming an unresolvable tool are stripped
with a warning.
Recording the catalogue durably in the stream would close the gap and is the
obvious follow-up, but it is a larger change than this RFD should carry.

**Anchoring stability under compaction.** `projection::apply` passes
`ConfigDelta` through verbatim and in position, and `assign_turn_indices` gives
it the current turn index without opening a turn, so a directive survives
compaction.
Collapsing a turn into a synthetic summary can move the *snap target*, but
adding a compaction already rewrites message history and busts the cache, so
nothing extra is lost.
The invariant to test: absent a new compaction, repeated requests anchor
byte-identically.

**Does `tool_removal` reclaim context?** No, and the feature's own mechanism
says so.
`tool_removal` preserves the cache precisely by *not* modifying the `tools`
array, and [the caching docs][anthropic-tool-caching] state that modifying tool
definitions invalidates the entire cache.
So the tools prefix is byte-identical across a removal, which means the
withdrawn definition is still in it.
The same page confirms the converse: "deferred tools are not included in the
system-prompt prefix".
`defer_loading` is the only mechanism that shrinks the window, which is why the
catalogue array defers everything.

The residual gap is narrow but real: "in the cached prefix" and "occupies the
context window" are adjacent claims, not identical ones.
Confirm with a two-minute check before phase 3, comparing `usage.input_tokens`
across three requests — both tools declared; both declared plus a
`tool_removal`; one declared with `defer_loading: true`.
Expect the first two to match and the third to drop.

**Does an initial directive apply to the first response?** The placement rules
force it after the first user message, and "applies from that point onward" is
doing work in that sentence.
Verify with a `tool_removal` of a tool the prompt asks for; a pass means the
model does not call it.

**Cache breakpoint budget.** `MAX_EXPLICIT_CACHE_CONTROL_COUNT` is 3, allocated
system, then documents, then tools.
With system content moving into messages the prelude covers less, and the tools
breakpoint must move to the non-deferred anchor tool.
The allocation comment in `anthropic.rs` needs revisiting.

## Implementation Plan

**Phase 1 — Directive derivation and the provider boundary.** `DirectiveBlock`,
`ThreadItem`, and `ThreadParts` in `jp_conversation`; derive text directives by
diffing section sets between consecutive config positions.
All seven providers flatten.
Behaviour identical to today; existing tests stay green.
Independently mergeable.

**Phase 2 — Anthropic text directives.** `MessageRole::System` and a system
content-block list in the `async-anthropic` fork.
Emit text directives as `role: system` messages with placement snapping, gated
on `MID_CONVERSATION_SYSTEM`.
Depends on phase 1.

**Phase 3 — Stable catalogue array.** Build the array from the full catalogue
with `defer_loading: true` on all but the anchor tool; move the cache breakpoint
to the anchor.
No directives emitted yet, so enablement still flattens.
Measurable: `cache_read_input_tokens` should be unchanged and `input_tokens`
should drop by roughly the definitions of all disabled tools.
Depends on the `tool_removal` verification above.

**Phase 4 — Tool directives.** `tool_addition` and `tool_removal` blocks in the
fork; derive tool directives from enablement diffs; emit the initial directive
after the first user message.
Gated on the feature and the beta header.
Depends on phases 2 and 3.

**Phase 5 — Anchoring stability tests.** Assert byte-identical anchoring across
repeated requests, including after compaction and turn pruning.
Assert unresolvable directives are stripped rather than sent.

## References

- [Mid-conversation system messages and tool changes][anthropic-midconv]
- [Tool search tool][anthropic-tool-search] — the `defer_loading` semantics and
  the `tool_reference` alternative
- [Tool use with prompt caching][anthropic-tool-caching] — what invalidates the
  tools prefix, and why deferral is the only way to shrink it
- [RFD 054] — config deltas positioned in the conversation stream
- [RFD 064] — compaction projection
- [RFD 076] — access grant revocation as a mid-conversation tool change
- [RFD 078] — tool config mutation as a mid-conversation tool change
- [RFD 081] — tool enablement state

[RFD 054]: 054-split-conversation-config-and-events.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[RFD 076]: 076-tool-access-grants.md
[RFD 078]: 078-tool-config-mutation.md
[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[anthropic-midconv]: https://platform.claude.com/docs/en/build-with-claude/mid-conversation-system-messages
[anthropic-tool-caching]: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-use-with-prompt-caching
[anthropic-tool-search]: https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool
