# RFD D17: Per-Tool Compaction Hints and Automatic Compaction

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-12
- **Extends**: [RFD 064](064-non-destructive-conversation-compaction.md)

## Summary

This RFD extends the compaction system from [RFD 064] with per-tool compaction
hints that let individual tools control how their calls are stripped, and
automatic compaction that triggers when conversations approach the context window
limit.

## Motivation

[RFD 064] delivered compaction as an explicit, user-initiated operation with
uniform stripping. Two gaps remain:

1. **Uniform stripping is too coarse.** `ToolCallPolicy::Strip` applies
   identically to all tools. But `fs_read_file` arguments (a file path) are
   cheap to keep, while `fs_create_file` arguments (full file content) dominate
   the token count. Without per-tool hints, users choose between stripping too
   aggressively (losing useful context like file paths) or too conservatively
   (keeping bulk they don't need).

2. **Manual compaction requires vigilance.** Users must notice degradation and
   run `jp conversation compact` at the right time. In long-running coding
   sessions, context windows fill gradually and quality degrades before the
   user intervenes.

## Design

### Per-Tool Compaction Hints

Tools declare how their calls should be compacted via a new `compaction` section
in their tool config:

```toml
[conversation.tools.fs_read_file.compaction]
request = "keep"
response = "strip"

[conversation.tools.fs_create_file.compaction]
request = "strip"
response = "keep"

[conversation.tools.fs_modify_file.compaction]
request = "strip"
response = "strip"
```

Each field accepts `"keep"` or `"strip"`. When absent, the field inherits from
the profile's `ToolCallPolicy`. A tool with `response = "keep"` is exempted from
response stripping even when the active profile sets `response: true`.

#### Config Type

A new `ToolCompactionConfig` struct is added as an optional field on
`ToolConfig`:

```rust
pub struct ToolCompactionConfig {
    /// How to handle this tool's request arguments during compaction.
    /// `None` inherits from the active compaction profile.
    pub request: Option<ToolFieldMode>,

    /// How to handle this tool's response content during compaction.
    /// `None` inherits from the active compaction profile.
    pub response: Option<ToolFieldMode>,
}

pub enum ToolFieldMode {
    Keep,
    Strip,
}
```

#### Projection Integration

The projection layer (`stream/projection.rs`) currently applies
`ToolCallPolicy::Strip` uniformly via `strip_tool_request` and
`strip_tool_response`. With per-tool hints:

1. Before projection, build a map of tool name → `ToolCompactionConfig` from the
   stream's resolved config.
2. When stripping a tool call request, check if the tool has
   `compaction.request = "keep"`. If so, skip stripping for that request.
3. When stripping a tool call response, check if the tool has
   `compaction.response = "keep"`. If so, skip stripping for that response.

The tool name is already available on `ToolCallRequest`. For responses, the
existing `tool_names` map (built during projection) provides the lookup.

#### Default Hints

JP should ship sensible defaults for its built-in tools in the workspace tool
config files:

| Tool             | `request` | `response` | Rationale                          |
|------------------|-----------|------------|------------------------------------|
| `fs_read_file`   | `keep`    | `strip`    | Path is cheap; file content isn't  |
| `fs_grep_files`  | `keep`    | `strip`    | Pattern is cheap; matches aren't   |
| `cargo_check`    | `keep`    | `strip`    | Args are cheap; output isn't       |
| `cargo_test`     | `keep`    | `strip`    | Args are cheap; output isn't       |
| `fs_create_file` | `strip`   | `keep`     | Content is bulk; "created" is cheap|
| `fs_modify_file` | `strip`   | `strip`    | Both carry large diffs             |
| `git_commit`     | `strip`   | `keep`     | Message is bulk; hash is cheap     |

### Automatic Compaction

Automatic compaction fires when the projected conversation size approaches the
model's context window. It evaluates after each turn completes (before
persisting).

#### Trigger

A character-based token estimate determines when to compact:

```
estimated_tokens = character_count / 4
threshold = model.context_window * auto.trigger_ratio
```

When `estimated_tokens > threshold` and the conversation has more turns than
`auto.min_turns`, compaction is applied using the configured profile.

#### Configuration

```toml
[conversation.compaction.auto]
enabled = false
trigger_ratio = 0.75
profile = "default"
min_turns = 5
```

Automatic compaction is disabled by default. It must be explicitly opted into
because compaction is lossy and the token estimation is approximate.

#### Behavior

When triggered:

1. Resolve the profile from `auto.profile`.
2. Resolve the range: `from = AfterLastCompaction`, `to = FromEnd(keep_last)`.
3. If the profile has `summary`, generate the summary (LLM call).
4. Append the compaction event.
5. Log the compaction (turn range, profile, estimated token reduction).

The compaction runs synchronously within the turn loop, between turns. This is
acceptable because mechanical strategies are fast, and summary generation (which
adds latency) is opt-in via the profile.

#### Context Window Discovery

Automatic compaction needs the model's context window size. This is available
from `ModelDetails` (returned by `provider.model_details()`). The turn loop
already has access to the provider and model details. If the context window is
unknown (e.g. a local model without metadata), automatic compaction is silently
skipped.

#### Prompt Cache Interaction

Adding a compaction event changes the projected prefix, invalidating cached
conversation history. For automatic compaction, this could cause unexpected
latency spikes. Mitigation: automatic compaction fires between turns, when the
cache is already partially invalidated by the new assistant response. The system
prompt cache (a separate prefix) is unaffected.

## Drawbacks

- **Per-tool hints add config surface.** Every tool gains an optional
  `compaction` section. Mitigation: hints are optional and inherit from the
  profile by default. Most users never set them.

- **Automatic compaction is lossy and invisible.** Users may not realize their
  conversation has been compacted. Mitigation: disabled by default, logged
  when it fires, original events always preserved.

- **Token estimation is approximate.** Character-based estimation can be off by
  2–3x depending on content. The `trigger_ratio` provides a safety margin,
  and a more accurate tokenizer-based approach can replace it later without
  changing the compaction model.

## Alternatives

### Tokenizer-based estimation

Use `tiktoken` or a model-specific tokenizer for accurate counts. Rejected for
now: adds a dependency, requires per-model tokenizer selection, and a
conservative `trigger_ratio` with character-based estimation is sufficient for
the trigger decision.

### Automatic compaction as a background task

Run compaction asynchronously (like title generation) so it doesn't block the
turn loop. Rejected: compaction modifies the event stream, and concurrent
modification during a turn would require synchronization that doesn't exist
today. Between-turn compaction is simpler and safe.

## Non-Goals

- **Token-accurate estimation.** This RFD uses character-based approximation.
  Precise tokenization is a future refinement that doesn't change the
  compaction model.

- **Per-tool compaction strategies beyond keep/strip.** Custom per-tool
  compaction functions (e.g. "summarize this tool's output") are interesting
  but add significant complexity. The keep/strip binary is sufficient for the
  common cases.

## Risks and Open Questions

- **What `trigger_ratio` works in practice?** 0.75 is a guess. Conversations
  with lots of tool calls may need a lower ratio since tool responses dominate
  token count. Needs experimentation.

- **Should automatic compaction notify the user?** A subtle indicator ("context
  compacted") during the next response could help, but adds UI complexity. The
  log is sufficient initially.

## Implementation Plan

### Phase 1: Per-Tool Compaction Hints

1. Add `ToolCompactionConfig` and `ToolFieldMode` to `jp_config`.
2. Add optional `compaction` field to `ToolConfig` with trait impls.
3. Update the projection layer to accept a tool config map and check per-tool
   overrides before stripping.
4. Add default hints to the JP tool config files.
5. Tests.

Can be merged independently. No behavioral change for users who don't configure
hints.

### Phase 2: Automatic Compaction

1. Add `AutoCompactionConfig` to `jp_config`.
2. Add token estimation function.
3. Wire the trigger check into the turn loop (after turn completion, before
   persist).
4. Reuse existing compaction logic to build and append the compaction event.
5. Add logging.
6. Tests (with mock provider for context window size).

Depends on Phase 1 for per-tool aware stripping. Can be merged independently
from Phase 1 if per-tool hints are not required for the trigger logic.

## References

- [RFD 064] — Non-Destructive Conversation Compaction

[RFD 064]: 064-non-destructive-conversation-compaction.md
