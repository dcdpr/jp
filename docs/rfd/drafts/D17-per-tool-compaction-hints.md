# RFD D17: Per-Tool Compaction Hints

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-12
- **Extends**: [RFD 064]
- **Summary**: Adds a per-tool `compaction` setting to override how individual
  tools' requests and responses are compacted.

## Summary

This RFD extends [RFD 064] with per-tool compaction hints that override a
compaction rule's `tool_calls` policy for individual tools.
Each tool can keep its calls intact, strip requests, strip responses, strip
both, or omit its calls entirely.

## Motivation

[RFD 064] applies a compaction rule's `tool_calls` policy uniformly across all
tools: `ToolCallPolicy::Strip` strips every tool's requests and responses the
same way, and `Omit` removes every tool's calls.
Tools, however, differ in which parts of a call are worth keeping:

- A search-style tool takes a short query and returns a large result.
  The request is cheap and useful for context, while the response holds most of
  the tokens.
- A write-style tool takes a large payload and returns a short acknowledgement.
  The request holds most of the tokens, while the response is cheap.
- A noisy diagnostic tool adds clutter without useful context and is better
  omitted altogether.

A single uniform policy forces one trade-off across all tools: strip
aggressively and lose small, useful context, or strip conservatively and retain
large payloads.
Per-tool hints let each tool specify its own compaction policy.

## Design

### Configuration

Each tool config accepts an optional `compaction` setting.
It uses the same values as a compaction rule's `tool_calls` policy, plus `keep`:

```toml
# A search-style tool: keep the query, drop the large result.
[conversation.tools.search]
compaction = "strip-responses"

# A write-style tool: drop the bulk payload, keep the acknowledgement.
[conversation.tools.upload]
compaction = "strip-requests"

# A noisy tool whose calls are not worth keeping at all.
[conversation.tools.heartbeat]
compaction = "omit"

# A tool whose calls should never be compacted, even when the rule strips.
[conversation.tools.plan]
compaction = "keep"
```

Accepted values:

- `keep`: never compact this tool's calls.
- `strip`: strip both request arguments and response content.
- `strip-requests`: strip request arguments and keep response content.
- `strip-responses`: keep request arguments and strip response content.
- `omit`: remove this tool's call pairs entirely.
- *unset* (default): inherit the rule's `tool_calls` policy.

Setting `compaction` under `conversation.tools.*` configures a default hint for
all tools.
Because JP ships no built-in tools, it defines no default hints; users or
bundled tool configs set them for the tools they use.

### Resolution and Storage

Per-tool hints are resolved from config when a `Compaction` event is created and
stored directly in the event, rather than looked up at projection time.
This preserves [RFD 064]'s guarantee that projection is a pure, deterministic
function of the stored stream: each compaction freezes its per-tool policies
alongside its turn range and summary, and later config edits do not alter how
past compactions project.

`Compaction` stores these resolved hints in `tool_overrides` alongside the
default `tool_calls` policy:

```rust
pub struct Compaction {
    // ...
    pub tool_calls: Option<ToolCallPolicy>, // default for tools without a hint
    pub tool_overrides: BTreeMap<String, ToolCallPolicy>,
}
```

When a compaction is created, every tool with a `compaction` hint is resolved
into `tool_overrides`:

| Hint              | Stored `ToolCallPolicy`                     |
| ----------------- | ------------------------------------------- |
| `keep`            | `Strip { request: false, response: false }` |
| `strip`           | `Strip { request: true, response: true }`   |
| `strip-requests`  | `Strip { request: true, response: false }`  |
| `strip-responses` | `Strip { request: false, response: true }`  |
| `omit`            | `Omit`                                      |

Because `Strip { request: false, response: false }` represents `keep` and
`ToolCallPolicy` already includes `Omit`, all five hints map to existing
`ToolCallPolicy` variants.

### Projection

The projection layer already resolves a per-turn `tool_calls` policy and knows
each event's tool name (directly on requests and via the `tool_names` map on
responses).
When projecting a tool event, it checks `tool_overrides` for the tool's name
before falling back to the turn's default `tool_calls` policy:

```text
effective = tool_overrides.get(name).unwrap_or(default tool_calls)
```

A per-tool `omit` drops both the request and the response for that tool,
matching how rule-level `Omit` drops calls for all tools.
Projection takes no new inputs and stays pure.

## Drawbacks

- **Additional per-tool config surface.** Every tool gains an optional
  `compaction` setting, though it inherits the rule policy when unset and can be
  omitted for most tools.
- **Hints are frozen at creation.** Changing a tool's `compaction` hint does not
  update existing compaction events, matching the deterministic behavior of
  compaction ranges and summaries.
  Running `jp conversation compact --reset` clears prior compaction events so a
  subsequent run applies the updated hints.

## Alternatives

### Resolve Hints at Projection Time

Projection could read each tool's hint from the resolved config instead of
storing overrides in the compaction event.
This was rejected because it couples projection to live config: editing a hint
would silently change how past compactions project and break [RFD 064]'s
pure-stream projection boundary.

### Two Booleans (`request` / `response`) Instead of One Value

Each tool could expose separate `request` and `response` fields, each set to
`keep` or `strip`.
This was rejected because two independent booleans cannot express `omit`
(removing a call pair is not a per-field choice), whereas a single setting
covers all four keep/strip combinations (`keep`, `strip`, `strip-requests`,
`strip-responses`) plus `omit` and reuses the rule-level `tool_calls`
vocabulary.

## Non-Goals

- **Per-tool summarization or custom transforms.** A hint selects among the
  existing `tool_calls` behaviors (keep, strip, omit) rather than running
  tool-specific compaction functions such as summarizing a tool's output.
- **Argument-aware hints.** A hint applies to a tool by name rather than
  inspecting call arguments or payload sizes (for example, keeping small reads
  while stripping large ones).

## Risks and Open Questions

- **Expectations around frozen hints.** Freezing hints at creation time is
  consistent with the rest of compaction, but users who edit a hint may expect
  existing conversations to update automatically.
  Re-running compaction with `--reset` applies the new hints; whether `--reset`
  needs to be more discoverable remains open.
- **Tool identity.** `tool_overrides` is keyed by the tool name recorded in the
  conversation stream.
  Implementation must verify that config keys for renamed, grouped, or
  plugin-namespaced tools match the names stored in stream events.

## Implementation Plan

### Phase 1: Config

1. Add the optional `compaction` hint (`tool_calls` values plus `keep`) to
   `ToolConfig` and the `*` defaults.
2. Add tests for parsing and inheritance.

This phase introduces no behavioral change.

### Phase 2: Event and Projection

1. Add `tool_overrides: BTreeMap<String, ToolCallPolicy>` to `Compaction`
   (defaulting to empty in serde for backward compatibility).
2. In projection, select the per-tool policy from `tool_overrides` before
   falling back to the turn default.
3. Add tests for per-tool strip, omit, keep, and inheritance.

Depends on Phase 1.

### Phase 3: Resolve Hints at Creation

1. Resolve each configured tool's `compaction` hint into `tool_overrides` when
   constructing a `Compaction` event, so every compaction creation path
   populates the overrides.
2. Add tests verifying that created events carry the resolved overrides.

Depends on Phases 1 and 2.

## References

- [RFD 064], Non-Destructive Conversation Compaction

[RFD 064]: ../064-non-destructive-conversation-compaction.md
