# RFD 052: Workspace Data Store Sanitization

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-18
- **Summary**: Validates the `.jp` conversation store on startup via
  `Workspace::sanitize()` and moves corrupt conversations to `.trash/` with
  diagnostics.

## Summary

This RFD introduces `Workspace::sanitize()`, which validates and repairs the
`.jp` data store on every CLI invocation.
Conversations that fail validation are moved to `.jp/conversations/.trash/` with
a `TRASHED.md` file explaining the error.
Before any command touches workspace state, `sanitize()` guarantees four
invariants: at least one conversation exists, the active conversation ID
resolves, every conversation directory is well-formed, and every conversation
has valid `metadata.json` and `events.json` files.

## Motivation

The `.jp/conversations/` directory is JP's primary data store, and every CLI
command loads it on startup.
When any part of the store is corrupt (a truncated `events.json`, a directory
with an unparseable name, or a `metadata.json` that references a deleted
conversation), the CLI either crashes or silently falls back to an empty default
state.

Because the data store consists of plain JSON files in a documented directory
layout, users can inspect, edit, and script against it directly: renaming
conversations, pruning events, copying conversation directories between
workspaces, or integrating external tools.
As a result, the store can enter states that JP itself would never produce, such
as partial edits, hand-written JSON with syntax errors, directories left behind
by interrupted scripts, or files modified by tools that do not follow the full
schema.
The sanitizer must move unloadable conversations to `.trash/`, explain what went
wrong, and continue operating on the valid data so a single bad file never
blocks access to the rest of the workspace.

Existing defenses against corrupt state are scattered and inconsistent:

- `load_conversations_from_disk` has a retry loop that falls back through
  conversation IDs when the active conversation's `metadata.json` is missing,
  but returns a hard error when `metadata.json` is corrupt.

- The active conversation's `events.json` is loaded eagerly, so a corrupt or
  missing file fails the entire load (`FIXME` at `workspace/lib.rs:222`).
  Non-active conversations are loaded lazily via `OnceCell` and silently skipped
  on failure.

- In `cli/lib.rs`, `load_conversations_from_disk` errors are caught and logged,
  and execution continues with an empty default state.
  The `persist()` call in `Workspace::Drop` does not destroy existing data (the
  empty `TombMap` has no dead keys, an empty active conversation writes nothing,
  and a subsequent query persists its new conversation alongside existing ones),
  but the all-or-nothing load hides the entire conversation list.
  A workspace with hundreds of valid conversations shows only the fresh default
  in `jp conversation ls`, and the `ERROR` log does not identify which
  conversation is broken or how to fix it without manually inspecting JSON files
  on disk.

- Conversation directories with unparseable names (for example, manually created
  directories or leftovers from a bug) log a warning on every run and remain
  ignored indefinitely.

Issue [#404] is one symptom: a clean install fails because
`load_conversations_from_disk` cannot find the conversation referenced by
`metadata.json`.
[PR #450][#450] fixes the fresh-workspace case, but corrupt data still causes
cascading failures.

JP needs a single pass that enforces data store invariants before any command
runs, repairing broken state or moving corrupt conversations aside with a clear
diagnostic instead of failing the load or silently ignoring errors.

## Design

### Invariants

The sanitizer enforces the following invariants on `.jp/conversations/` and its
user-storage counterpart:

1. **Well-formed directories.** Every entry in `conversations/` is either a
   known file (`metadata.json`) or a directory whose name starts with a valid
   `ConversationId` (parseable via `try_from_dirname`).

2. **Valid conversation data.** Every conversation directory contains a
   `metadata.json` that deserializes as `Conversation` and an `events.json` that
   is structurally valid JSON (see [Lightweight events.json
   validation](#lightweight-events-json-validation)).

3. **Valid conversations metadata.** The `conversations/metadata.json` file (the
   global metadata that holds `active_conversation_id`) is structurally valid
   JSON that deserializes as `ConversationsMetadata`.
   If the file is missing, the system uses a default.
   If the file is corrupt, the sanitizer deletes it so that subsequent loads use
   a default.

4. **Resolvable active conversation.** `active_conversation_id` in
   `conversations/metadata.json` resolves to a conversation that passes checks 1
   and 2.
   This check runs unconditionally, covering both trashed active conversations
   and metadata that points to a missing directory (such as stale metadata from
   a previous session or a corrupt metadata file that was just reset to the
   default).

5. **Graceful empty state.** If all conversations are trashed (or none existed),
   the stale metadata file is removed and `load_conversations_from_disk` handles
   this as a fresh workspace with default state.
   No conversation is written to disk during sanitization.

### User experience

When sanitization trashes a conversation, the CLI logs a warning:

```txt
WARN Trashed corrupt conversation 17457886043-my-chat
     (see .jp/conversations/.trash/17457886043-my-chat/TRASHED.md)
```

If the active conversation was trashed and a fallback was selected:

```txt
WARN Active conversation was corrupt, switched to 17636257528-other-chat
```

If all conversations were trashed and a new default was created:

```txt
WARN No valid conversations found, created a new empty conversation
```

Users can inspect `.trash/` to understand what went wrong and manually recover
data if the underlying files are salvageable.

### Trash directory

Conversations that fail validation are moved to
`.jp/conversations/.trash/<original-dirname>/`.
The leading dot hides the trash directory from the rest of the system:
`load_conversation_id_from_entry` skips entries whose names start with `.`, so
`.trash/` is never parsed as a `ConversationId` and generates no warnings.

Each trashed conversation gets a `TRASHED.md` file written into its directory:

```markdown
# Trashed Conversation

This conversation was moved here because it failed workspace sanitization.

**Error:** metadata.json: expected value at line 3 column 1
**Date:** 2026-03-18T14:22:07Z

The original conversation files are preserved alongside this file.
If the data is recoverable, you can fix the issue and move the
directory back to `.jp/conversations/`.
```

`TRASHED.md` uses human-readable Markdown rather than JSON because it is a
recovery aid for the user, not machine-readable state.

If `.trash/` already contains a directory with the same name from a previous
sanitization run, the new entry receives an integer suffix (for example,
`17457886043-my-chat-1`).

### API

`Workspace` exposes a new public method:

```rust
impl Workspace {
    /// Validate and repair the `.jp/conversations/` data store.
    ///
    /// Trashes conversations that fail validation and ensures the workspace
    /// invariants hold. Returns a report of actions taken.
    pub fn sanitize(&mut self) -> Result<SanitizeReport>;
}
```

`SanitizeReport` records the repairs performed:

```rust
pub struct SanitizeReport {
    /// Conversations moved to `.trash/`.
    pub trashed: Vec<TrashedConversation>,

    /// Whether the active conversation was reassigned.
    pub active_reassigned: bool,

    /// Whether a new default conversation was created because none remained.
    pub default_created: bool,
}

pub struct TrashedConversation {
    /// The original directory name.
    pub dirname: String,

    /// The error that caused the conversation to be trashed.
    pub error: String,
}

impl SanitizeReport {
    /// Returns `true` if any repairs were made.
    pub fn has_repairs(&self) -> bool {
        !self.trashed.is_empty() || self.active_reassigned || self.default_created
    }
}
```

### Sanitization logic

The method operates directly on the filesystem, before in-memory state is
constructed.
It runs against both the workspace storage root and the user storage root (if
configured).

```txt
for each storage root (workspace, user):
    for each entry in {root}/conversations/:
        skip if entry is a file (metadata.json, etc.)
        skip if entry name starts with "." (.trash/)

        1. try parse ConversationId from dirname
           → on failure: trash with "invalid directory name: {name}"

        2. try deserialize {entry}/metadata.json as Conversation
           → on missing: trash with "missing metadata.json"
           → on parse error: trash with "metadata.json: {error}"

        3. try lightweight validation of {entry}/events.json
           (valid JSON, array of objects with `timestamp` fields)
           → on missing: trash with "missing events.json"
           → on parse error: trash with "events.json: {error}"

load conversations/metadata.json:
    → on missing: use default ConversationsMetadata
    → on corrupt JSON: delete the file, use default ConversationsMetadata
    → on I/O error: propagate (not a data-corruption issue)

if no valid conversations remain and nothing was trashed:
    return (fresh workspace, nothing to repair)

resolve active_conversation_id:
    if the referenced conversation doesn't exist among the valid IDs:
        pick the most recent valid conversation (by ConversationId sort order,
            descending)
        update conversations/metadata.json
        set active_reassigned = true

if no valid conversations remain:
    remove stale conversations/metadata.json if present
    set active_reassigned = true
    set default_created = true
    (load_conversations_from_disk handles this as a fresh workspace)
```

After sanitization completes, `load_conversations_from_disk` runs as usual
against the cleaned directory.
Sanitization is a single best-effort repair pass; if loading still fails, the
error propagates.

### Call site in `cli/lib.rs`

Sanitization runs between workspace construction and conversation loading:

```rust
let mut workspace = load_workspace(cli.globals.workspace.as_ref())?;

let report = workspace.sanitize()?;
if report.has_repairs() {
    for trashed in &report.trashed {
        tracing::warn!(
            dirname = trashed.dirname,
            error = trashed.error,
            "Trashed corrupt conversation"
        );
    }
    if report.active_reassigned {
        tracing::warn!("Active conversation was corrupt, switched to fallback");
    }
    if report.default_created {
        tracing::warn!("No valid conversations found, created new default");
    }
}

if let Err(error) = workspace.load_conversations_from_disk() {
    tracing::error!(error = ?error, "Failed to load workspace.");
}
```

With sanitization in place, `load_conversations_from_disk` should fail only on
external errors such as permission failures or concurrent modifications by
another process.
When a load error does occur, the CLI continues with a fresh default
conversation.
`persist()` remains safe in that state: the empty `TombMap` has no dead keys to
delete existing conversations from disk, and any new conversation from the
session is written alongside the existing directories.

Active conversation resolution runs on every invocation, even when no
conversations are trashed.
This repairs `active_conversation_id` whenever it points to a manually deleted
directory or was reset to its default after a corrupt
`conversations/metadata.json` was removed.

### Lightweight `events.json` validation

`ConversationStream` deserialization is expensive because event files can span
thousands of lines.
`Workspace` uses `OnceCell` to load events lazily so it only pays this cost when
a conversation is accessed.

To preserve lazy loading, sanitization avoids deserializing the full
`ConversationStream` and instead runs a lightweight structural check modeled on
`load_count_and_timestamp_events` in `jp_storage::load`:

```rust
#[derive(Deserialize)]
struct RawEvent {
    timestamp: Box<serde_json::value::RawValue>,
}

fn validate_events_file(path: &Utf8Path) -> Result<(), String> {
    let file = fs::File::open(path)
        .map_err(|e| format!("cannot open events.json: {e}"))?;
    let reader = BufReader::new(file);
    let _events: Vec<RawEvent> = serde_json::from_reader(reader)
        .map_err(|e| format!("events.json: {e}"))?;
    Ok(())
}
```

This confirms:

1. The file exists and is readable.
2. The content is valid JSON.
3. The top-level structure is an array.
4. Each element is an object with a `timestamp` field.

This check does not validate individual event variants (tool calls, chat
requests, config deltas).
Variant-level errors are caught later during lazy deserialization, where the
`maybe_init_events` functions already log warnings and skip broken
conversations.

### Simplifying `load_conversations_from_disk`

`load_conversations_from_disk` has a fallback loop that retries loading through
conversation IDs when the active conversation is missing.
Once `sanitize()` runs first, it has already trashed conversations with missing
or corrupt metadata and reassigned the active conversation ID.
Removing the redundant fallback loop simplifies `load_conversations_from_disk`
and eliminates duplicate recovery paths.

Sanitization also resolves the `FIXME` at `workspace/lib.rs:222`, where a
corrupt `events.json` in the active conversation causes a hard failure.
Because `sanitize()` validates `events.json` before loading begins,
`load_conversations_from_disk` can assume the filesystem is clean and treat
remaining errors as unexpected failures.

Similarly, `sanitize()` validates the global `conversations/metadata.json`.
If the file contains corrupt JSON, the sanitizer deletes it (causing
`load_conversations_metadata` to return a default) and resolves the active
conversation as normal.
I/O errors when reading the file still propagate, as they indicate a system
failure rather than data corruption.

## Drawbacks

**I/O cost on every invocation.** Sanitization scans every conversation
directory, deserializes each `metadata.json`, and validates the top-level array
and `timestamp` fields of each `events.json` via `RawValue` instead of
deserializing the full `ConversationStream` (see [Lightweight events.json
validation](#lightweight-events-json-validation)).

For typical workspaces with tens of conversations, the added latency is
negligible.
For large workspaces with hundreds of conversations, file opens and JSON
tokenization dominate the cost.
If startup latency becomes measurable, sanitization can skip conversations whose
`events.json` `mtime` has not changed since the last successful run.

**Data loss via trashing.** Moving a conversation to `.trash/` hides it from the
CLI.
If the failure was caused by a transient condition (such as a concurrent write
from another process), the conversation becomes inaccessible until the user
manually restores it from `.trash/` using `TRASHED.md`.

## Alternatives

### Sanitize inside `load_conversations_from_disk`

Merge validation and repair into `load_conversations_from_disk` so every load
call also fixes corruption.

Rejected because it conflates loading state with repairing state on disk.
Callers do not expect a load method to have filesystem side effects like moving
directories.
Keeping `sanitize()` separate makes repair explicit and lets the caller decide
when to run it.

### Storage-level validation

Add a validate-and-repair mode to each `load_*` method in `jp_storage`.

Rejected because `jp_storage` is a generic persistence layer and should not
encode workspace domain invariants such as requiring both `metadata.json` and
`events.json` or resolving the active conversation ID.
Those invariants belong in `jp_workspace`, which owns the conversation model.

### Delete instead of trash

Delete corrupt conversations instead of moving them to `.trash/`.

Rejected because deletion is irreversible and conversations can contain hours of
LLM interaction.
Moving directories to `.trash/` uses no extra disk space and preserves a
recovery path.

## Non-Goals

- **Automatic recovery of corrupt JSON.** If `events.json` is truncated, the
  sanitizer does not attempt to salvage valid prefixes into a partial stream.
  Partial JSON recovery is complex and error-prone; users can recover data
  manually from `.trash/`.

- **Crash-safe writes.** Preventing corruption through atomic writes with rename
  is a separate concern and a candidate for a follow-up RFD.

- **Trash management.** Automatically pruning old `.trash/` entries (for
  example, after 30 days) is deferred.
  Because conversation data is small, unbounded `.trash/` growth is unlikely to
  be a problem in practice.

- **User-facing `jp workspace repair` command.** Sanitization runs automatically
  on startup; a manual CLI command for explicit or more aggressive repair is out
  of scope here.

## Risks and Open Questions

### Performance with large workspaces

Lightweight `events.json` validation avoids full deserialization, but still
opens and tokenizes every events file.
In a workspace with hundreds of conversations, this could add measurable startup
latency.
Implementation should benchmark with a realistic workspace (50+ conversations
with large event files) to confirm the cost is acceptable.

### User-storage and workspace-storage interaction

Conversations can live in workspace storage (`.jp/conversations/`) or user-local
storage (`$XDG_DATA_HOME/jp/workspace/<name>-<id>/conversations/`), and
`conversations/metadata.json` (which holds `active_conversation_id`) lives in
user storage when configured.
The sanitizer must scan both roots and update user-storage metadata when an
active conversation in workspace storage is trashed.

### Concurrent access

If two JP processes run concurrently in different terminals, one process's
sanitization pass could trash a conversation that the other process is actively
writing to.
JP does not yet lock conversation files; [RFD 020] addresses concurrent access
with file locking, and sanitization should respect locks once they exist.

## Implementation Plan

### Phase 1: Trash infrastructure

Add `.trash/` directory support to `jp_storage`:

- `Storage::trash_conversation(id, dirname, error)`: move a conversation
  directory to `.trash/` and write `TRASHED.md`.
- Handle the case where a trashed conversation with the same name already
  exists.
- Unit tests with temp directories.

**Dependency:** None.
**Mergeable:** Yes.

### Phase 2: `Workspace::sanitize()`

Implement the sanitization logic in `jp_workspace`:

- `SanitizeReport` and `TrashedConversation` types.
- The validation loop: parse dirname, deserialize metadata, validate events.
- Global `conversations/metadata.json` validation and recovery from corrupt
  JSON.
- Active conversation reassignment (runs unconditionally, not only after
  trashing).
- Fresh-workspace detection: if no conversations exist on disk and nothing was
  trashed, return immediately without touching metadata.
- Remove the redundant fallback loop and `FIXME` in
  `load_conversations_from_disk`.
- Unit tests covering each validation failure mode, including stale and corrupt
  global metadata.

**Dependency:** Phase 1.
**Mergeable:** Yes.

### Phase 3: CLI integration

Wire sanitization into `cli/lib.rs`:

- Call `workspace.sanitize()` after workspace construction, before
  `load_conversations_from_disk`.
- Log the sanitization report as warnings when repairs were made.

**Dependency:** Phase 2.
**Mergeable:** Yes.

## References

- [Issue #404][#404]: clean install fails due to unresolvable conversation ID.
- [PR #450][#450]: partial fix for fresh workspace handling.
- [RFD 006]: `ConversationStream::sanitize()`, the event-level precedent for
  this pattern.
- [RFD 020]: parallel conversations and file locking (relevant to concurrent
  access risk).
- [RFD 023]: resumable conversation turns and their interaction with
  `sanitize()`.

[#404]: https://github.com/dcdpr/jp/issues/404
[#450]: https://github.com/dcdpr/jp/pull/450
[RFD 006]: 006-turn-scoped-stream-mutations.md
[RFD 020]: 020-parallel-conversations.md
[RFD 023]: 023-resumable-conversation-turns.md
