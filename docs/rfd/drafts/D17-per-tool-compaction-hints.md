# RFD D17: Per-Tool Compaction Hints

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-12
- **Extends**: [RFD 064]

## Summary

This RFD extends [RFD 064] with per-tool compaction hints: a per-tool override
that decides how an individual tool's calls are compacted, instead of applying a
rule's `tool_calls` policy uniformly to every tool.
A tool can keep its request, keep its response, be stripped, or be omitted
entirely, independent of the rule's default.

## Motivation

[RFD 064] applies a compaction rule's `tool_calls` policy uniformly:
`ToolCallPolicy::Strip` strips every tool's request and response the same way,
and `Omit` removes every tool's calls.
But tools are not uniform in where their tokens sit:

- A search-style tool takes a short query and returns a large result.
  Its request is cheap to keep and useful for context; its response is bulk.
- A write-style tool takes a large payload and returns a short acknowledgement.
  Its request is bulk; its response is cheap.
- A noisy diagnostic tool may be pure clutter, worth removing entirely.

With one uniform policy the user picks a single trade-off for all of them: strip
aggressively and lose the cheap, useful parts, or strip conservatively and keep
the bulk.
Per-tool hints let each tool declare its own treatment.

## Design

### What the user configures

A tool's compaction treatment is a single value on its config, drawn from the
same vocabulary as a rule's `tool_calls` policy, plus `keep`:

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
- `strip-requests`: strip the request, keep the response.
- `strip-responses`: keep the request, strip the response.
- `omit`: remove this tool's call pairs entirely.
- *unset* (the default): inherit the rule's `tool_calls` policy.

This reuses the rule-level `tool_calls` vocabulary, so there is one set of
values to learn.
The `conversation.tools.*` defaults section can set a hint for every tool at
once.

JP ships no tools of its own, so it ships no default hints.
Hints are opt-in, set by the user (or by a tool's own bundled config) for the
tools they actually use.

### Resolved at creation, stored in the event

Per-tool hints are resolved from config when a compaction is created, and the
result is stored in the compaction event.
They are not looked up at projection time.

This follows the same principle as the rest of [RFD 064]: the projection layer
is a pure function of the stored stream, so the compacted view is deterministic.
Resolving hints live would thread tool config into projection and let a later
config edit silently rewrite the projection of every past compaction.
Resolving at creation freezes each compaction's per-tool decisions the way its
range and any summary are already frozen.

The `Compaction` event gains a per-tool override map beside the existing
default:

```rust
pub struct Compaction {
    // ...
    pub tool_calls: Option<ToolCallPolicy>, // default for tools without a hint
    pub tool_overrides: BTreeMap<String, ToolCallPolicy>,
}
```

At creation, every tool with a `compaction` hint is resolved into
`tool_overrides`:

| Hint              | Stored `ToolCallPolicy`                     |
| ----------------- | ------------------------------------------- |
| `keep`            | `Strip { request: false, response: false }` |
| `strip`           | `Strip { request: true, response: true }`   |
| `strip-requests`  | `Strip { request: true, response: false }`  |
| `strip-responses` | `Strip { request: false, response: true }`  |
| `omit`            | `Omit`                                      |

`ToolCallPolicy` already carries `Omit`, so omit and full-`keep` both fall out
of the existing type; no new policy variant is needed.

### Projection

The projection layer already resolves a per-turn `tool_calls` policy and already
has the tool name in hand (on the request directly, on the response via the
`tool_names` map).
The only change: before applying the policy to a tool event, look up the tool's
name in `tool_overrides` and use that policy when present, otherwise the turn's
default `tool_calls`.

```text
effective = tool_overrides.get(name).unwrap_or(default tool_calls)
```

A per-tool `omit` drops both the request and the response for that tool, exactly
as rule-level `Omit` does for all tools.
Projection takes no new inputs and stays pure.

## Drawbacks

- **Config surface per tool.** Every tool gains an optional `compaction` value.
  It is a single optional scalar, inherits the rule policy when unset, and most
  tools never need it.
- **Hints are frozen at creation.** Re-tuning a tool's hint does not change
  compaction events that already exist.
  This is the same deterministic property as compaction ranges and summaries;
  `jp conversation compact --reset` clears the old events so a re-run picks up
  the new hints.

## Alternatives

### Resolve hints at projection time

Look up each tool's hint from the resolved config *during* projection, storing
nothing in the event.
Rejected: it makes the projection layer depend on live config, so editing a hint
silently changes how past compactions project, and it breaks the pure-stream
projection boundary [RFD 064] relies on.

### Two booleans (`request` / `response`) instead of one value

Give each tool a `request` and a `response` field, each `keep` or `strip`.
Rejected: two independent booleans cannot express `omit` (whole-pair removal is
not a per-field choice), the shape does not match the rule-level `tool_calls`
vocabulary, and its four combinations are exactly `keep` / `strip` /
`strip-requests` / `strip-responses` in the single-value form.
The single value is more expressive and reuses one vocabulary.

## Non-Goals

- **Per-tool summarization or custom transforms.** A hint selects among the
  existing `tool_calls` behaviors (keep, strip, omit).
  Tool-specific compaction functions, such as summarizing one tool's output, are
  a larger feature and out of scope.
- **Argument-aware hints.** A hint applies to a tool by name, not to particular
  argument values.
  Deciding per call (for example, keep small reads and strip large ones) is not
  covered here.

## Risks and Open Questions

- **Is frozen-at-creation what users expect?** It is consistent with the rest of
  compaction, but a user who tunes hints and expects existing conversations to
  follow may be surprised.
  `--reset` is the answer; whether it needs to be more discoverable is open.
- **Tool identity.** Hints key on the tool name as it appears in the stream.
  Renamed, grouped, or plugin-namespaced tools must use the name the stream
  records, which needs checking against how tool names are stored.

## Implementation Plan

### Phase 1: Config

1. Add the per-tool `compaction` hint to `ToolConfig` (an optional value over
   the `tool_calls` vocabulary plus `keep`), and to the `*` defaults.
2. Tests for parsing and inheritance.

No behavioral change until projection reads it.

### Phase 2: Event and projection

1. Add `tool_overrides: BTreeMap<String, ToolCallPolicy>` to `Compaction` (serde
   default empty, backward compatible).
2. In projection, select the per-tool policy from `tool_overrides` before
   falling back to the turn default.
3. Tests for per-tool strip, omit, keep, and inheritance.

Depends on Phase 1.

### Phase 3: Bake hints at creation

1. When building a compaction event, resolve each tool's hint from config into
   `tool_overrides`.
2. Any path that creates a compaction event gets this automatically, since it
   builds the same event.
3. Tests that created events carry the resolved overrides.

Depends on Phases 1 and 2.

## References

- [RFD 064], Non-Destructive Conversation Compaction

[RFD 064]: ../064-non-destructive-conversation-compaction.md
