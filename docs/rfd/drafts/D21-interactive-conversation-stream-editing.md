# RFD D21: Interactive Conversation Stream Editing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-13

## Summary

This RFD introduces `jp conversation edit --interactive`, an `$EDITOR`-based
workflow for destructively editing the raw event stream of a conversation. The
editor opens a temporary directory containing a plan file (the manifest of
events) and individual event files (markdown or TOML). Users delete or reorder
lines in the plan to restructure the conversation, and edit individual event
files to modify content. On editor exit, JP validates the result and rebuilds
the `ConversationStream`.

## Motivation

JP users already edit conversations by opening `events.json` directly — tweaking
context, removing noisy tool calls, editing responses, trimming history. This
works but is painful: the JSON is base64-encoded in places, events are
interleaved with config deltas, and the structure is fragile (orphaned tool
responses, broken request/response alternation).

[RFD 064] introduced non-destructive compaction as an overlay — the right
approach for routine context reduction. But compaction is a *projection*: it
changes what the LLM sees without changing what's stored. Sometimes you need
actual surgery: fix a wrong tool result, reword the user's request, delete an
entire tangent, inject a clarifying message. Compaction can't do this.

`jp conversation fork --last N` is the blunt instrument for this today. It
discards everything before the last N turns. There's no way to selectively keep
turn 1, drop turns 2-4, and keep turn 5 onward.

Users need a way to make precise, destructive edits to the conversation stream
in a format they can read and manipulate with their existing editor.

## Design

### User-Facing Behavior

#### The `--interactive` Flag

```sh
jp conversation edit --interactive [ID]
```

Opens an interactive editing session for the active conversation (or the
specified one). This is a destructive operation — it modifies the stored event
stream.

The short flag is `-i`:

```sh
jp conversation edit -i
```

#### What the Editor Sees

JP creates a temporary directory with this structure:

```
/tmp/jp-edit-<id>/
├── CONVERSATION
├── 000-request.md
├── 001-message.md
├── 002-tool-call-fs_create_file.md
├── 003-tool-result-fs_create_file.md
├── 004-config-delta.toml
├── 005-request.md
├── 006-reasoning.md
├── 007-tool-call-fs_read_file.md
├── 008-tool-result-fs_read_file.md
├── 009-message.md
├── 010-compaction.toml
└── ...
```

The editor is invoked with this directory as its argument. Most editors (VS
Code, Vim, Neovim, Emacs) open a directory as a file browser or project root,
giving the user a tree view of all files.

#### The Plan File

`CONVERSATION` is the manifest. It lists every event file, grouped by turn with
comment headers. The all-caps name sorts to the top in file explorers (uppercase
before lowercase in most locales) and gives editors a recognizable filename for
syntax highlighting — the same pattern as `Makefile` or `Dockerfile`.

```txt
# Conversation Edit Plan
# - Delete  lines to remove events.
# - Reorder lines to reorder events.
# - Edit    event contents in the corresponding files.

# Turn 0
000-request.md
001-message.md
002-tool-call-fs_create_file.md
003-tool-result-fs_create_file.md

# Turn 1
004-config-delta.toml
005-request.md
006-reasoning.md
007-tool-call-fs_read_file.md
008-tool-result-fs_read_file.md
009-message.md

# Turn 2
010-compaction.toml
011-request.md
012-message.md
```

The `CONVERSATION` file controls **structure**: which events survive and in what order.
Users delete lines to drop events and reorder lines to reorder events. Comment
lines (`#`) are ignored during parsing.

The `CONVERSATION` file is authoritative for structure. If a line is removed, the event is
dropped — regardless of whether the file still exists on disk.

#### Event Files

Each event is written to an individual file. The file format depends on the
event type:

**Conversation events** use markdown with YAML frontmatter:

```markdown
---
type: request
---
Set up the project with error handling and logging.
```

```markdown
---
type: message
---
I'll create the project structure for you.
```

```markdown
---
type: reasoning
---
The user wants a Rust project with error handling. I should use
anyhow for the error type and set up a basic main.rs...
```

```markdown
---
type: tool-call
tool: fs_create_file
id: call_abc123
---
```json
{
  "path": "src/main.rs",
  "content": "fn main() {\n    println!(\"hello\");\n}"
}
```
```

```markdown
---
type: tool-result
id: call_abc123
is_error: false
---
File created successfully.
```

**Metadata events** use TOML:

```toml
# config-delta.toml
[assistant]
model = "anthropic/claude-sonnet"
```

```toml
# compaction.toml
from_turn = 0
to_turn = 5

[reasoning]
policy = "strip"
```

The frontmatter carries the metadata needed to reconstruct the
`ConversationEvent`: event type, tool name, call ID, error status. The body
carries the user-editable content.

#### File Naming

Files are named with a zero-padded numeric prefix followed by a descriptive
suffix:

| Event type      | Suffix                          | Extension |
|-----------------|---------------------------------|-----------|
| `ChatRequest`   | `request`                       | `.md`     |
| `ChatResponse`  | `message`, `reasoning`,         | `.md`     |
|                 | `structured`                    |           |
| `ToolCallReq`   | `tool-call-{tool_name}`         | `.md`     |
| `ToolCallResp`  | `tool-result-{tool_name}`       | `.md`     |
| `InquiryReq`    | `inquiry-request`               | `.md`     |
| `InquiryResp`   | `inquiry-response`              | `.md`     |
| `ConfigDelta`   | `config-delta`                  | `.toml`   |
| `Compaction`    | `compaction`                    | `.toml`   |

The numeric prefix establishes original ordering for orientation — it is an
address, not a sort key. The `CONVERSATION` file determines final ordering.

`TurnStart` events are not represented as files. They are internal markers
inferred from the event sequence during reconstruction (a `ChatRequest` starts a
new turn). The turn comments in the plan file (`# Turn N`) provide visual
grouping but have no semantic effect.

#### The Edit Cycle

1. JP creates the temporary directory and writes all files.
2. JP hashes every file.
3. JP opens the editor with the directory path.
4. The user edits the plan and/or event files.
5. The editor exits.
6. JP re-reads the plan file and all referenced event files.
7. JP validates the result (see [Validation](#validation)).
8. If valid: JP rebuilds the `ConversationStream` and persists it.
9. If invalid: JP writes error annotations to the top of `CONVERSATION` and
   re-opens the editor. The user can fix the issue or clear the file to abort.

**Abort:** If `CONVERSATION` is empty (all lines deleted or cleared) when the
editor exits, the edit is aborted with no changes. If the editor exits with a
non-zero status code, the edit is also aborted.

**Unchanged files:** If a file's hash matches its original, JP uses the original
event data. Only files with changed hashes are re-parsed. This avoids
round-trip fidelity issues for events the user didn't touch.

#### New Events

Users can create new event files in the temporary directory and add their
filenames to the plan. JP parses new files the same way as modified files — the
frontmatter must contain valid metadata for the event type.

For tool calls, the user must provide a call ID in the frontmatter. If omitted,
JP generates one. Tool results must reference an existing call ID.

This enables injecting events: adding a clarifying user message, inserting a
corrected tool result, or adding a config delta.

### Integration with `fork`

`jp conversation fork` gains an `--edit` flag:

```sh
jp conversation fork --edit
jp conversation fork --last 5 --edit
```

This forks the conversation first (with any applicable filtering), then opens
the interactive editor on the forked conversation. The original conversation is
untouched. This is the safe workflow for destructive edits — you always work on
a copy.

The `--edit` flag is incompatible with `--compact` (compaction is
non-destructive and additive; editing is destructive).

### Validation

When the editor exits, JP validates the rebuilt stream. Validation enforces
structural invariants that providers require:

1. **Tool result follows its tool call.** A `tool-result` must appear after the
   `tool-call` with the matching call ID.
2. **Request/response alternation.** A `ChatRequest` (user role) must not be
   immediately followed by another `ChatRequest` without an intervening
   assistant response.
3. **Orphaned references.** A `tool-result` whose call ID doesn't match any
   `tool-call` in the plan is rejected. Similarly for inquiry responses.
4. **Missing references.** A `tool-call` without a matching `tool-result` in the
   plan gets a synthetic error response injected (consistent with
   `sanitize_orphaned_tool_calls`).
5. **Non-empty stream.** The plan must contain at least one `ChatRequest`.

Validation reuses and extends the existing `ConversationStream::sanitize()`
logic. The difference: `sanitize()` silently fixes issues (it's designed for
automatic recovery), while the editor validation reports errors and re-opens the
editor so the user can fix them intentionally.

When validation fails, JP prepends error messages to `CONVERSATION`:

```txt
# ERROR: tool-result call_abc123 appears before its tool-call (line 5)
# ERROR: orphaned tool-result call_xyz789 has no matching tool-call
#
# Fix the errors above and save, or clear this file to abort.

# Turn 0
000-request.md
...
```

### Round-Trip Fidelity

The editing format must preserve all data needed to reconstruct events exactly.
Key concerns:

- **Tool call arguments.** Complex nested JSON must survive the round-trip
  through the markdown frontmatter format. Arguments are stored as a JSON code
  block in the file body, not inlined into YAML.
- **Timestamps.** Original event timestamps are preserved in the frontmatter but
  hidden from casual editing (they appear as a `timestamp` field). If the user
  modifies a timestamp, the new value is used. If omitted from a new event, the
  current time is used.
- **Metadata.** The `metadata` map on `ConversationEvent` (cache breakpoints,
  rendered arguments, etc.) is serialized into the YAML frontmatter. Fields the
  user doesn't touch are preserved via the hash-based change detection.
- **Base64 encoding.** The storage layer base64-encodes certain fields (tool
  arguments, response content). The editor files contain decoded (plain text)
  content. Re-encoding happens during reconstruction.
- **Base config.** The conversation's `base_config.json` is not part of the edit
  session. It is immutable and preserved as-is.

## Drawbacks

- **Destructive by nature.** Unlike compaction, this modifies the actual stored
  events. There is no undo. Mitigation: `fork --edit` is the recommended
  workflow, and we document it prominently. The original conversation is
  untouched.

- **Format complexity.** The markdown-with-frontmatter format for events is a
  new serialization format that must be maintained alongside the JSON storage
  format. It adds code surface for parsing and round-tripping.

- **Editor compatibility.** The design assumes the editor can open a directory.
  Most modern editors support this, but some minimal editors (e.g. `ed`, `nano`)
  do not. For these editors, the experience degrades — the user would need to
  open `CONVERSATION` directly and navigate to event files manually.

- **Large conversations.** A conversation with 500 events produces 500+ files in
  the temporary directory. This is fine for filesystems and editors, but the plan
  file becomes long. Mitigation: the turn-grouped comments help navigation, and
  users typically edit recent portions of the conversation.

## Alternatives

### Single-file editing (git rebase model)

Present all events in a single plan file with inline content. The user edits
everything in one file, using `pick`/`drop`/`edit` verbs like `git rebase -i`.

Rejected because conversation events are contextual — when editing event 7, you
want to see events 6 and 8 simultaneously. A single-file model either inlines
all content (making the file enormous and hard to navigate) or forces a
sequential `edit` workflow where you're walked through events one at a time.
The directory model lets you open multiple files side-by-side in your editor.

### Git rebase `pick`/`drop` verbs

Add `pick` and `drop` verbs to the plan file lines, matching git rebase's
interface.

Rejected as unnecessary complexity. Since the plan file only supports two
structural operations (delete and reorder), the simpler model works: lines
present = kept, lines absent = dropped, line order = event order. No verbs to
learn.

### Edit `events.json` directly

The status quo. Users open the raw JSON file and edit it.

This already works but is error-prone: base64-encoded fields, interleaved
config deltas, fragile structural invariants. The interactive editor provides a
human-readable format with validation on save.

### Non-destructive editing via overlay

Extend the compaction overlay model to support content replacement — store
"event X should have this content instead" as a projection event.

Rejected because it conflates two different concerns. Compaction reduces what
the LLM sees while preserving history. Content editing changes history itself.
Overlaying content edits would make the projection layer significantly more
complex and would still not support structural changes (reordering, deletion of
arbitrary events).

## Non-Goals

- **Automatic conflict resolution.** If two users edit the same conversation
  simultaneously, the last writer wins. Interactive editing acquires the
  conversation lock, so concurrent edits are prevented during the session.

- **Undo/redo.** There is no undo for destructive edits. Use `fork --edit` to
  work on a copy. The original conversation is the "undo."

- **TUI-based editing.** This RFD uses `$EDITOR` exclusively. A built-in
  terminal UI for event editing (with live validation, drag-and-drop reordering,
  etc.) is a separate feature.

- **Partial stream editing.** The editor session covers the entire conversation
  stream. Editing a subset (e.g. "only turns 5-10") can be achieved by forking
  with `--last` first, then editing the fork.

- **Custom editor invocation.** The editor is resolved using `EditorConfig`
  (the existing `JP_EDITOR` / `VISUAL` / `EDITOR` chain). Adding a
  conversation-edit-specific editor config (e.g. for opening a directory vs. a
  file) is deferred.

## Risks and Open Questions

- **Frontmatter parsing robustness.** YAML frontmatter in markdown is a
  de-facto standard but has edge cases (content that looks like YAML, `---`
  in code blocks). We need a robust parser that handles these correctly. The
  existing `jp_md` crate does not parse frontmatter today.

- **Inquiry event editing.** `InquiryRequest` and `InquiryResponse` have
  complex structures (answer types, select options, default values). The
  frontmatter representation needs to be both human-readable and round-trippable.
  This may require a more structured format for inquiry events specifically.

- **Structured response editing.** `ChatResponse::Structured` contains a
  `serde_json::Value`. The editing format needs to present this as editable
  JSON while preserving the value's structure on round-trip.

- **Editor directory support.** If the configured editor cannot open a
  directory, the experience breaks. We should detect this and fall back to
  opening `CONVERSATION` directly, with a note about how to edit individual
  event files.

- **Conversation lock duration.** The conversation lock is held for the entire
  editor session. If the user leaves the editor open for hours, other operations
  on that conversation are blocked. This matches the behavior of `git rebase`
  holding a lock, but may need a warning or timeout.

## Implementation Plan

### Phase 1: Event Serialization Format

1. Define the markdown-with-frontmatter format for each `EventKind` variant.
2. Define the TOML format for `ConfigDelta` and `Compaction` events.
3. Implement `serialize_to_edit_file()` and `deserialize_from_edit_file()` for
   each event type.
4. Add round-trip unit tests: serialize an event, deserialize it, assert
   equality.

Can be merged independently. No behavioral changes.

### Phase 2: Plan File and Directory Builder

1. Implement the temporary directory builder: given a `ConversationStream`,
   produce the file tree (`CONVERSATION` + event files).
2. Implement the file naming scheme (numeric prefix + descriptive suffix).
3. Implement turn-grouped comments in the plan file.
4. Add unit tests for directory generation from sample streams.

Depends on Phase 1.

### Phase 3: Plan Parser and Stream Reconstruction

1. Implement the plan file parser: read `CONVERSATION`, produce an ordered list
   of filenames.
2. Implement the reconstruction pipeline: parse the plan, read event files
   (using originals for unchanged files), rebuild the `ConversationStream`.
3. Implement hash-based change detection for event files.
4. Add unit tests for plan parsing, reconstruction, and change detection.

Depends on Phase 2.

### Phase 4: Validation

1. Implement structural validation on the rebuilt stream (tool call pairing,
   request/response alternation, orphaned references).
2. Implement error annotation in `CONVERSATION` for re-opening.
3. Add unit tests for each validation rule and the error-annotation format.

Depends on Phase 3. Can partially reuse `ConversationStream::sanitize()`.

### Phase 5: CLI Integration

1. Add `--interactive` / `-i` flag to `jp conversation edit`.
2. Implement the edit cycle: create directory, hash files, open editor, read
   back, validate, rebuild, persist.
3. Implement abort detection (empty plan, non-zero exit).
4. Add `--edit` flag to `jp conversation fork`.
5. Integration tests with `MockEditorBackend`.

Depends on Phase 4.

## References

- [RFD 047] — Editor and Path Access for Conversations
- [RFD 064] — Non-Destructive Conversation Compaction (defers interactive
  editing as a non-goal)
- [Issue #57] — Make conversation management more powerful
- `crates/jp_conversation/src/stream.rs` — `ConversationStream` and
  `InternalEvent` definitions
- `crates/jp_conversation/src/event.rs` — `EventKind` variants
- `crates/jp_editor/src/lib.rs` — `EditorBackend` trait
- `crates/jp_config/src/editor.rs` — `EditorConfig` and editor resolution

[RFD 047]: 047-editor-and-path-access-for-conversations.md
[RFD 064]: 064-non-destructive-conversation-compaction.md
[Issue #57]: https://github.com/dcdpr/jp/issues/57
