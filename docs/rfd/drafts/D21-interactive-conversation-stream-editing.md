# RFD D21: Interactive Conversation Stream Editing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-13
- **Summary**: Adds `jp conversation edit --interactive` for git-rebase-style
  editing, reordering, and pruning of conversation events in `$EDITOR`.

## Summary

This RFD introduces `jp conversation edit --interactive`, an `$EDITOR`-based
workflow for destructively editing a conversation's event stream.
The editor opens a temporary directory containing a `CONVERSATION` plan file and
individual Markdown and TOML event files that the user can reorder, delete, or
edit before JP validates and rebuilds the `ConversationStream`.

## Motivation

JP users already edit conversations by opening `events.json` directly to tweak
context, remove noisy tool calls, edit responses, or trim history.
Editing `events.json` by hand is error-prone: some fields are base64-encoded,
conversation events are interleaved with config deltas, and stream invariants
(paired tool calls and results, alternating requests and responses) are easy to
break.

[RFD 064] introduced non-destructive compaction for routine context reduction.
Compaction is a *projection*: it changes what the LLM sees without modifying the
stored event stream.
It cannot fix a wrong tool result, reword a user request, delete a tangent, or
inject a clarifying message.

`jp conversation fork --last N` only discards everything before the last `N`
turns.
It cannot keep turn 1, drop turns 2 through 4, and keep turn 5 onward.

Users need a way to make precise, destructive edits to the conversation stream
in a format they can read and manipulate with their existing editor.

## Design

### User-Facing Behavior

#### The `--interactive` Flag

```sh
jp conversation edit --interactive [ID]
```

`--interactive` opens an interactive editing session for the active
conversation, or for `ID` when specified, and modifies the stored event stream
in place.

The short flag is `-i`:

```sh
jp conversation edit -i
```

#### Temporary Directory Layout

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

JP passes this directory to `$EDITOR`.
Most editors (VS Code, Vim, Neovim, Emacs) open a directory as a file browser or
project root and display a tree view of all files.

#### The Plan File

`CONVERSATION` is the edit manifest: it lists every event file, grouped into
turns by comment headers.
The all-caps name, like `Makefile` or `Dockerfile`, sorts to the top in file
explorers in most locales and gives editors a fixed filename for syntax
highlighting.

```txt
# Conversation Edit Plan
# - Delete  lines to remove events.
# - Reorder lines to reorder events.
# - Edit    event contents in the corresponding files.

# Turn 1 (k3m9x2a): Set up project structure with error handling
000-request.md
001-message.md
002-tool-call-fs_create_file.md
003-tool-result-fs_create_file.md

# Turn 2 (p8q2r4b): Add retry backoff to stream handler
004-config-delta.toml
005-request.md
006-reasoning.md
007-tool-call-fs_read_file.md
008-tool-result-fs_read_file.md
009-message.md

# Turn 3 (t5v7w1c): Why does the retry test still fail?
010-compaction.toml
011-request.md
012-message.md
```

`CONVERSATION` is authoritative for stream structure: it determines which events
are kept and in what order.
Reordering lines reorders events; deleting a line drops the event even if its
file remains on disk.
Comment lines (`#`) are ignored during parsing.

Each turn header shows the turn's `TurnStart` `event_id` ([RFD 097]) and its
turn summary ([RFD 126]).
If a turn has no stored summary, the header shows the fallback summary instead:
the first line of the turn's opening request, truncated to 72 characters.
Turn headers are ordinary comments, so editing them has no effect.

#### Event Files

Each event is written to its own file in a format determined by the event type.

**Conversation events** use Markdown with YAML frontmatter:

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

````markdown
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
````

````

```markdown
---
type: tool-result
id: call_abc123
is_error: false
---
File created successfully.
````

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
`ConversationEvent` (event type, tool name, call ID, and error status), and the
body holds the editable content.

#### File Naming

Files are named with a zero-padded numeric prefix followed by a descriptive
suffix:

| Event type     | Suffix                    | Extension |
| -------------- | ------------------------- | --------- |
| `ChatRequest`  | `request`, `reply`        | `.md`     |
| `ChatResponse` | `message`, `reasoning`,   | `.md`     |
|                | `structured`              |           |
| `ToolCallReq`  | `tool-call-{tool_name}`   | `.md`     |
| `ToolCallResp` | `tool-result-{tool_name}` | `.md`     |
| `InquiryReq`   | `inquiry-request`         | `.md`     |
| `InquiryResp`  | `inquiry-response`        | `.md`     |
| `ConfigDelta`  | `config-delta`            | `.toml`   |
| `Compaction`   | `compaction`              | `.toml`   |

The numeric prefix records the original position for orientation; it is an
identifier, not a sort key.
`CONVERSATION` determines the final event order.

#### Turn Markers

`TurnStart` events are not written as files, and the turn headers in
`CONVERSATION` are comments with no semantic effect.
JP infers turn boundaries during reconstruction: each `request` file starts a
new turn.

Not every `ChatRequest` starts a turn.
When the user replies to an interrupt (`InterruptAction::Reply`), JP appends a
`ChatRequest` to the active turn after the assistant's partial output.
That request is written as a `reply` file (`type: reply`) and continues the
current turn.
Changing a file's `type` from `reply` to `request` splits the turn in two;
changing `request` to `reply` merges the turn into the preceding one.

Turn identity and summaries follow the stream-rewriting contract in [RFD 126]:

- When a `request` file that originally started a turn still starts a turn after
  editing, the reconstructed turn reuses the original `TurnStart` and keeps its
  `event_id`.
- A turn's stored summary is preserved only when the turn is unchanged: the same
  event files in the same order, each with its original content hash
  (`ConfigDelta` and `Compaction` events belong to no turn and are ignored).
  If any event in the turn changed, JP clears the summary and the next `jp
  query` regenerates it.
- A turn started by a new `request` file, or by a `reply` file changed to `type:
  request`, gets a new `TurnStart` with a fresh `event_id` and no summary.

#### The Edit Cycle

1. JP creates the temporary directory, writes all files, and hashes each event
   file.
2. JP opens `$EDITOR` with the directory path.
3. The user edits `CONVERSATION` and any event files, then closes the editor.
4. JP reads `CONVERSATION` and all referenced event files.
5. JP validates the reconstructed stream (see [Validation](#validation)).
6. If valid, JP rebuilds the `ConversationStream` and persists it.
7. If invalid, JP prepends error annotations to `CONVERSATION` and re-opens the
   editor so the user can fix the errors or clear the file to abort.

**Abort:** If the editor exits with a non-zero status code, or if `CONVERSATION`
contains no event lines when the editor exits, JP aborts the edit without
modifying the conversation.

**Unchanged files:** If an event file's hash matches its original hash, JP
reuses the original event data and only re-parses modified files.
Unmodified events are not round-tripped through the file parser.

#### Injecting New Events

Users can inject new events, such as a clarifying user request, a corrected tool
result, or a config delta, by creating files in the temporary directory and
adding their filenames to `CONVERSATION`.
JP parses new files the same way as modified files: the frontmatter must contain
valid metadata for the event type.

A `tool-call` file may specify an `id` in its frontmatter, or omit it to let JP
generate one.
A `tool-result` file must reference a `tool-call` ID present in the plan.

### Integration with `fork`

`jp conversation fork` gains an `--edit` flag:

```sh
jp conversation fork --edit
jp conversation fork --last 5 --edit
```

`--edit` forks the conversation (applying any turn filters such as `--last`) and
opens the interactive editor on the new fork, leaving the original conversation
untouched.

`--edit` is incompatible with `--compact`, because compaction adds a
non-destructive overlay whereas interactive editing rewrites the event stream.

### Validation

When the editor exits, JP validates the rebuilt stream against the structural
invariants that model providers require:

1. **Tool result follows its tool call.** A `tool-result` must appear after the
   `tool-call` with the matching call ID.
2. **Request/response alternation.** A `ChatRequest` (user role) must not be
   immediately followed by a `request` without an intervening assistant
   response.
   A `reply` may directly follow a `ChatRequest`, because an interrupt can occur
   before the assistant emits a response.
3. **Orphaned references.** A `tool-result` whose call ID does not match any
   `tool-call` in the plan is rejected.
   An `inquiry-response` without a matching `inquiry-request` is also rejected.
4. **Unanswered tool calls.** A `tool-call` without a matching `tool-result` in
   the plan receives a synthetic error response, matching
   `sanitize_orphaned_tool_calls`.
5. **Non-empty stream.** The plan must contain at least one `ChatRequest`.
6. **Reply placement.** Because a `reply` continues an existing turn, it cannot
   appear before the plan's first `request`.

Validation reuses and extends the rules in `ConversationStream::sanitize()`.
Whereas `sanitize()` repairs broken streams silently for automatic recovery,
interactive validation reports errors and re-opens the editor so the user can
fix them.

When validation fails, JP prepends error messages to `CONVERSATION`:

```txt
# ERROR: tool-result call_abc123 appears before its tool-call (line 5)
# ERROR: orphaned tool-result call_xyz789 has no matching tool-call
#
# Fix the errors above and save, or clear this file to abort.

# Turn 1 (k3m9x2a): Set up project structure with error handling
000-request.md
...
```

### Round-Trip Fidelity

The editing format preserves all data needed to reconstruct events:

- **Tool call arguments.** Arguments are written as a fenced JSON code block in
  the Markdown body rather than inlined into YAML frontmatter, so nested JSON
  structures survive round-tripping.
- **Timestamps.** Each event's timestamp is serialized in the frontmatter as
  `timestamp`.
  Modifying the field updates the event's timestamp; omitting it on a new event
  defaults to the current time.
- **Metadata.** The `metadata` map on `ConversationEvent` (cache breakpoints,
  rendered arguments, and provider metadata) is serialized into the YAML
  frontmatter.
  For unmodified files, hash-based change detection preserves the original event
  directly.
- **Base64 encoding.** The storage layer base64-encodes fields such as tool
  arguments and response content.
  Edit files contain decoded plain text, and JP re-encodes them during
  reconstruction.
- **Base config.** The conversation's `base_config.json` is immutable and
  excluded from the edit session.
- **Event identity.** Each generated file maps back to its source stream entry,
  so an edited event keeps its `event_id` ([RFD 097]).
  A newly created file gets a fresh ID.
  If a filename appears more than once in `CONVERSATION`, each additional
  occurrence is treated as a copy and gets a fresh ID.

## Drawbacks

- **Destructive edits.** Unlike compaction, interactive editing overwrites the
  stored event stream with no undo.
  `jp conversation fork --edit` mitigates this by editing a copy while leaving
  the original conversation untouched.

- **Format maintenance.** The Markdown-with-frontmatter format is a second event
  serialization format to maintain alongside the JSON storage format.

- **Editor compatibility.** Passing a directory to `$EDITOR` works in editors
  with a file tree (VS Code, Vim, Neovim, Emacs), but minimal editors such as
  `ed` or `nano` cannot open directories.
  Users of those editors must open `CONVERSATION` and individual event files
  manually.

- **Large conversations.** A conversation with 500 events produces over 500
  files and a long `CONVERSATION` file.
  Turn header comments keep the plan navigable, and users typically edit recent
  turns.

## Alternatives

### Single-File Editing (`git rebase` Model)

Present all events in a single plan file with inline content, using `pick`,
`drop`, and `edit` verbs like `git rebase -i`.

We rejected this because conversation events are contextual: editing event 7
often requires reading events 6 and 8 alongside it.
A single-file model either inlines every event body into one large file or steps
through marked events one at a time.
The directory layout lets users open multiple event files side by side.

### Git Rebase `pick` and `drop` Verbs

Prefix each line in `CONVERSATION` with a `pick` or `drop` verb, matching `git
rebase -i`.

We rejected this because the plan file only supports two structural operations:
deleting and reordering events.
Keeping present lines, dropping deleted lines, and using line order as event
order expresses both operations without extra syntax.

### Editing `events.json` Directly

Have users open and edit the raw `events.json` file in storage.

This requires no dedicated tooling, but `events.json` has base64-encoded fields,
interleaved config deltas, and no validation on save.
The interactive editor provides human-readable files and validates stream
invariants before persisting.

### Non-Destructive Editing via Overlay

Extend the compaction overlay model ([RFD 064]) to store content replacements as
projection events without modifying stored events.

We rejected this because compaction and content editing solve different
problems.
Compaction reduces what the LLM sees while preserving history; stream editing
rewrites history itself.
Layering content edits onto the projection pipeline would complicate projection
and still would not support reordering or deleting arbitrary events.

## Non-Goals

- **Automatic conflict resolution.** Interactive editing acquires the
  conversation lock for the duration of the session rather than merging
  concurrent edits.

- **Undo/redo.** Destructive edits have no undo stack.
  Use `jp conversation fork --edit` to edit a copy while keeping the original
  conversation intact.

- **TUI-based editing.** This RFD uses `$EDITOR` exclusively.
  A built-in terminal UI with live validation and interactive reordering is a
  separate feature.

- **Partial stream editing.** An edit session always exports the entire
  conversation stream.
  To edit only recent history (for example, turns 5 through 10), fork with
  `--last` first and edit the fork.

- **Custom editor invocation.** The editor is resolved through `EditorConfig`
  (`JP_EDITOR`, `VISUAL`, `EDITOR`).
  A separate editor configuration for directory-based editing is deferred.

- **On-demand turn summaries.** The plan file shows stored summaries and falls
  back to the truncated request line when a turn has no summary; JP makes no LLM
  request before opening `$EDITOR`.
  Because [RFD 126] generates a turn's summary during the next query, the most
  recent turn typically uses the fallback.

## Risks and Open Questions

- **Frontmatter parsing.** YAML frontmatter in Markdown has delimiter edge cases
  (content that resembles YAML, or `---` inside code blocks).
  The `jp_md` crate does not parse frontmatter, so interactive editing needs a
  parser that handles these cases cleanly.

- **Inquiry event editing.** `InquiryRequest` and `InquiryResponse` have
  structured fields (answer types, select options, default values) that must be
  both human-readable and lossless across a round-trip.
  Inquiry events may need a more structured representation than plain
  frontmatter.

- **Structured response editing.** `ChatResponse::Structured` holds a
  `serde_json::Value`.
  The editing format must present this payload as editable JSON while preserving
  its structure on round-trip.

- **Editor directory support.** If the configured editor cannot open a
  directory, JP should detect the failure and fall back to opening
  `CONVERSATION` directly, with a comment explaining how to open individual
  event files.

- **Conversation lock duration.** JP holds the conversation lock for the entire
  editor session, matching `git rebase -i`.
  Leaving the editor open blocks other commands on that conversation, which may
  warrant a warning or timeout.

## Implementation Plan

### Phase 1: Event Serialization Format

1. Define the Markdown-with-frontmatter format for each `EventKind` variant,
   including the `request` and `reply` types of a `ChatRequest`.
2. Define the TOML format for `ConfigDelta` and `Compaction` events.
3. Implement `serialize_to_edit_file()` and `deserialize_from_edit_file()` for
   each event type.
4. Add round-trip unit tests that serialize an event, deserialize it, and assert
   equality.

This phase introduces no behavioral changes and can be merged independently.

### Phase 2: Plan File and Directory Builder

1. Implement the temporary directory builder: given a `ConversationStream`,
   produce the file tree (`CONVERSATION` and event files).
2. Implement the file naming scheme (numeric prefix and descriptive suffix),
   writing each `ChatRequest` after the first in a turn as a `reply`.
3. Write turn-grouped header comments in `CONVERSATION` with each turn's
   `TurnStart` `event_id` and its stored or fallback summary.
4. Add unit tests for directory generation from sample streams.

This phase depends on Phase 1.

### Phase 3: Plan Parser and Stream Reconstruction

1. Implement the plan file parser to read `CONVERSATION` and return an ordered
   list of filenames.
2. Implement hash-based change detection for event files.
3. Implement the reconstruction pipeline: parse the plan, read modified or new
   event files, reuse original events for unchanged files, and rebuild the
   `ConversationStream`.
4. Preserve event and turn identity during reconstruction: edited events keep
   their `event_id`, turns started by an existing `request` file reuse their
   original `TurnStart`, and modified turns have their stored summary cleared.
5. Add unit tests for plan parsing, change detection, and turn reconstruction:
   unchanged turns (`event_id` and summary preserved), modified turns
   (`event_id` preserved, summary cleared), reordered turns, new `request` files
   (fresh `TurnStart`), turns containing an interrupt `reply`, and `reply` files
   changed to `request` (splitting the turn and assigning a fresh `TurnStart` to
   the second turn).

This phase depends on Phase 2.

### Phase 4: Validation

1. Implement structural validation on the rebuilt stream (tool call pairing,
   request/response alternation, orphaned references).
2. Implement error annotation in `CONVERSATION` when re-opening the editor.
3. Add unit tests for each validation rule and the error-annotation format.

This phase depends on Phase 3 and can reuse validation rules from
`ConversationStream::sanitize()`.

### Phase 5: CLI Integration

1. Add the `--interactive` / `-i` flag to `jp conversation edit`.
2. Implement the edit cycle: create the directory, hash files, open the editor,
   read back the plan, validate, rebuild, and persist.
3. Implement abort detection (empty plan or non-zero editor exit status).
4. Add the `--edit` flag to `jp conversation fork`.
5. Add integration tests using `MockEditorBackend`.

This phase depends on Phase 4.

## References

- [RFD 047]: Editor and Path Access for Conversations
- [RFD 064]: Non-Destructive Conversation Compaction (defers interactive editing
  as a non-goal)
- [RFD 097]: Stable Event Identifiers (`event_id` kept across edits)
- [RFD 126]: Turn Summaries (summaries in turn headers, consumer contract)
- [Issue #57]: Make conversation management more powerful
- `crates/jp_conversation/src/stream.rs`: `ConversationStream` and
  `InternalEvent` definitions
- `crates/jp_conversation/src/event.rs`: `EventKind` variants
- `crates/jp_editor/src/lib.rs`: `EditorBackend` trait
- `crates/jp_config/src/editor.rs`: `EditorConfig` and editor resolution

[Issue #57]: https://github.com/dcdpr/jp/issues/57
[RFD 047]: ../047-editor-and-path-access-for-conversations.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD 126]: ../126-turn-summaries.md
