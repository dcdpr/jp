# RFD 052: Workspace Data Store Sanitization

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-18

## Summary

This RFD introduces a `Workspace::sanitize()` method that validates and repairs
the `.jp` data store on every CLI invocation. Conversations that fail validation
are moved to `.jp/conversations/.trash/` with a `TRASHED.md` file explaining the
error. The method guarantees four invariants before any command touches
workspace state: at least one conversation exists, the active conversation ID
resolves, every conversation directory is well-formed, and every conversation
has valid `metadata.json` and `events.json` files.

## Motivation

The `.jp/conversations/` directory is the primary data store for JP. Every CLI
command depends on loading this state correctly. When any piece is corrupt — a
truncated `events.json`, a directory with an unparseable name, a `metadata.json`
that references a deleted conversation — the CLI either crashes or silently
operates on empty default state.

JP's data store is intentionally transparent: plain JSON files in a
well-documented directory layout. Users are expected to inspect, edit, and
script against this data — renaming conversations, pruning events, copying
conversation directories between workspaces, or integrating with external tools.
This is a feature, not an accident. But it also means the data store will
encounter states that JP itself would never produce: partial edits, hand-written
JSON with typos, directories left behind by interrupted scripts, or files
modified by tools that don't understand the full schema. The sanitizer must
handle these gracefully — trashing what it can't load, explaining what went
wrong, and continuing to operate on the data that is still valid. A single bad
file should never prevent the user from accessing the rest of their workspace.

Today the code handles some of these cases, but the defenses are scattered and
inconsistent:

- `load_conversations_from_disk` has a retry loop that falls back through
  conversation IDs when the active conversation's `metadata.json` is missing.
  But a corrupt (not missing) `metadata.json` is a hard error that propagates
  up.

- The active conversation's `events.json` is loaded eagerly. If it's corrupt or
  missing, the load fails entirely (the FIXME at `workspace/lib.rs:222`
  documents this). Non-active conversations are lazily loaded via `OnceCell` and
  silently skipped on failure — different failure behavior for the same kind of
  data.

- In `cli/lib.rs`, `load_conversations_from_disk` errors are caught and logged,
  but execution continues with default (empty) state. The `persist()` call in
  `Workspace::Drop` is safe in this case — the empty `TombMap` has no dead
  keys, so no existing conversations are deleted, and the active conversation
  has no events, so nothing is written. If the user runs a query after the
  failed load, the new conversation is persisted alongside the existing ones.
  No data is destroyed, but the user experience is poor: the load is
  all-or-nothing, so a single corrupt conversation hides the entire
  conversation list. A workspace with hundreds of valid conversations shows
  just one (the fresh default) in `jp conversation ls`. The user sees an
  `ERROR` log but has no way to identify which conversation is broken or
  fix it without manually inspecting JSON files on disk.

- Conversation directories with unparseable names (e.g. manually created, left
  over from a bug) are logged as warnings and ignored forever.

The blast radius is the core problem: one bad file takes down the whole
conversation list. The defenses that exist are either too aggressive (hard
error on corrupt active conversation) or too lenient (silent skip on corrupt
non-active conversation), and none of them tell the user what went wrong or
how to fix it.

Issue [#404] is one symptom — a clean install fails because
`load_conversations_from_disk` can't find a conversation that `metadata.json`
references. The fix in [PR #450][#450] addresses the specific case of a fresh
workspace, but the general problem of corrupt data causing cascading failures
remains.

We need a single, systematic pass that enforces data store invariants before any
command runs. When something is broken, it should be fixed or moved aside with
an explanation — not silently ignored or allowed to corrupt other data.

## Design

### Invariants

The sanitizer enforces four invariants on the `.jp/conversations/` directory
(and its user-storage counterpart):

1. **Well-formed directories.** Every entry in `conversations/` is either a
   known file (`metadata.json`) or a directory whose name starts with a valid
   `ConversationId` (parseable via `try_from_dirname`).

2. **Valid conversation data.** Every conversation directory contains a
   `metadata.json` that deserializes as `Conversation` and an `events.json` that
   is structurally valid JSON (see [Lightweight events.json
   validation](#lightweight-eventsjson-validation)).

3. **Valid conversations metadata.** The `conversations/metadata.json` file (the
   global metadata that holds `active_conversation_id`) is structurally valid
   JSON that deserializes as `ConversationsMetadata`. If the file is missing,
   the system uses a default. If the file is corrupt, the sanitizer deletes it
   so that subsequent loads use a default.

4. **Resolvable active conversation.** The `active_conversation_id` from the
   conversations metadata resolves to a conversation that passes checks 1 and
   2. This check runs unconditionally — not only when conversations are trashed,
      but also when the metadata points to a conversation ID that has no
      corresponding directory on disk (e.g., stale metadata from a previous
      session, or a corrupt metadata file that was just reset to default).

5. **Graceful empty state.** If all conversations are trashed (or none existed),
   the stale metadata file is removed and `load_conversations_from_disk` handles
   this as a fresh workspace with default state. No conversation is written to
   disk during sanitization.

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
`.jp/conversations/.trash/<original-dirname>/`. The leading dot makes the trash
directory invisible to the rest of the system: `load_conversation_id_from_entry`
skips entries whose names start with `.`, so the trash directory is never
attempted as a `ConversationId` parse and generates no warnings.

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

The format is intentionally human-readable Markdown, not JSON. This is a
recovery aid for the user, not machine-readable state.

If a conversation with the same directory name already exists in `.trash/` (from
a previous sanitization run), the new trashed version gets an integer suffix
(e.g. `17457886043-my-chat-1`).

### API

A new public method on `Workspace`:

```rust
impl Workspace {
    /// Validate and repair the `.jp/conversations/` data store.
    ///
    /// Trashes conversations that fail validation and ensures the workspace
    /// invariants hold. Returns a report of actions taken.
    pub fn sanitize(&mut self) -> Result<SanitizeReport>;
}
```

The `SanitizeReport` captures what happened:

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
constructed. It runs against both the workspace storage root and the user
storage root (if configured).

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

After sanitization completes, `load_conversations_from_disk` is called as usual.
Because the filesystem is now clean, the load should succeed. If it still fails,
the error propagates — sanitization is best-effort repair, not an infinite
retry.

### Call site in `cli/lib.rs`

The sanitization runs between workspace construction and conversation loading:

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

The existing `persist()` behavior after a failed load is actually safe — the
empty `TombMap` has no dead keys, so no existing conversations are deleted from
disk. If the user runs a query, the new conversation is written alongside the
existing ones. No data is lost in either case.

With sanitization in place, load failures after sanitize should be rare
(permission errors, races with other processes). If one does occur, the CLI
continues with a fresh default conversation and the user's session work is
persisted normally.

Note that the metadata resolution runs unconditionally — not only when
conversations are trashed, but whenever the `active_conversation_id` doesn't
match a valid conversation on disk. This covers stale metadata that points to a
conversation whose directory was manually deleted, or a corrupt metadata file
that was just reset to its default value.

### Lightweight `events.json` validation

`ConversationStream` deserialization is expensive — event files can be thousands
of lines. The `OnceCell`-based lazy loading in `Workspace` exists specifically
to defer this cost until a conversation is actually accessed.

Sanitization must not defeat this design. Instead of deserializing the full
`ConversationStream`, it uses a lightweight structural check modeled on the
existing `load_count_and_timestamp_events` function in `jp_storage::load`:

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

This does not validate individual event variants (tool calls, chat requests,
config deltas). Variant-level errors are caught later during lazy
deserialization, where per-conversation failure handling already exists (the
`maybe_init_events` functions log warnings and skip broken conversations).

### Simplifying `load_conversations_from_disk`

`load_conversations_from_disk` currently has its own fallback loop that retries
loading through conversation IDs when the active conversation is missing. With
sanitization running first, this fallback logic is dead code — sanitize has
already trashed conversations with missing or corrupt metadata and reassigned
the active conversation ID if needed. The fallback loop should be removed to
keep the load path simple and avoid two code paths that handle the same failure
mode differently.

The FIXME at `workspace/lib.rs:222` (corrupt `events.json` for the active
conversation causing a hard failure) is also resolved: sanitize validates
`events.json` before load ever runs. The load method can assume the filesystem
is clean and treat any remaining errors as unexpected failures.

The global `conversations/metadata.json` is also validated during sanitization.
If it contains corrupt JSON, the sanitizer deletes it (causing
`load_conversations_metadata` to return a default) and proceeds with the normal
resolution logic. I/O errors reading the file still propagate — those indicate a
system problem, not data corruption.

## Drawbacks

**I/O cost on every invocation.** Sanitization scans all conversation
directories, deserializes each `metadata.json`, and performs a lightweight
structural check on each `events.json` (see [Lightweight events.json
validation](#lightweight-eventsjson-validation)). The lightweight check avoids
full `ConversationStream` deserialization — it parses the array structure and
`timestamp` fields using `RawValue`, which is substantially cheaper than
deserializing every event variant.

For most workspaces (tens of conversations), this adds negligible latency. For
large workspaces (hundreds of conversations), the cost is dominated by file
opens and JSON tokenization. If this becomes measurable, a future optimization
could skip validation for conversations whose `events.json` mtime hasn't changed
since the last successful sanitization run.

**Data loss via trashing.** Moving a conversation to `.trash/` makes it
invisible to the CLI. If the corruption was a transient issue (e.g. a concurrent
write from another process), the user loses access to their conversation. The
`.trash/` directory and `TRASHED.md` file mitigate this — the data is
recoverable — but it requires manual intervention.

## Alternatives

### Sanitize inside `load_conversations_from_disk`

Merge validation and repair into the existing load method. Every load call would
also fix corruption.

Rejected because it conflates two responsibilities — loading state and repairing
state — and makes the load method harder to reason about. Callers don't expect a
"load" to have filesystem side effects like moving directories. A separate
method makes the repair explicit and lets the caller decide when and whether to
run it.

### Storage-level validation

Push validation into `jp_storage`. Each `load_*` method gains a "validate and
repair" mode.

Rejected because `jp_storage` is a generic persistence layer. It shouldn't know
domain-level invariants like "a conversation must have both metadata.json and
events.json" or "the active conversation ID must resolve." Those invariants
belong in `jp_workspace`, which understands the conversation model.

### Delete instead of trash

Remove corrupt conversations instead of moving them.

Rejected because deletion is irreversible. Conversations may contain hours of
LLM interaction that the user values. The `.trash/` directory is cheap (no
additional disk usage beyond the move) and gives users a recovery path.

## Non-Goals

- **Automatic recovery of corrupt JSON.** If `events.json` is truncated, the
  sanitizer does not attempt to parse what it can and reconstruct a partial
  stream. That level of recovery is complex and error-prone. The user can
  attempt manual recovery from `.trash/`.

- **Crash-safe writes.** Preventing corruption in the first place (e.g. via
  atomic writes with rename) is a separate concern. This RFD handles the
  consequences of corruption, not its causes. Atomic writes would be a
  worthwhile follow-up.

- **Trash management.** Automatically cleaning up old `.trash/` entries (e.g.
  after 30 days) is deferred. The trash directory can grow without bound, but
  conversation data is small and this is unlikely to be a problem in practice.

- **User-facing `jp workspace repair` command.** A CLI command that explicitly
  triggers sanitization (and possibly more aggressive repair) is a natural
  follow-up but out of scope here. The sanitizer runs automatically; the command
  would be for manual use.

## Risks and Open Questions

### Performance with large workspaces

The lightweight `events.json` validation avoids full deserialization, but still
requires opening and tokenizing every events file. For a workspace with hundreds
of conversations, this could add measurable startup latency. Implementation
should benchmark with a realistic workspace (50+ conversations with large event
files) to confirm the cost is acceptable.

### User-storage and workspace-storage interaction

Conversations can live in either the workspace storage (`.jp/conversations/`) or
user-local storage (`$XDG_DATA_HOME/jp/workspace/<name>-<id>/conversations/`).
The sanitizer must scan both roots. The `conversations/metadata.json` (which
holds `active_conversation_id`) lives in user storage when configured. If the
active conversation is in workspace storage but has been trashed, the
user-storage metadata needs updating. The implementation must handle both roots
consistently.

### Concurrent access

If two JP processes run simultaneously (e.g. in different terminals), one
process's sanitization could trash a conversation that the other process is
actively writing to. This is an existing problem (no file locking today), and
sanitization doesn't make it worse — but it doesn't fix it either. [RFD 020]
addresses concurrent access with file locking; sanitization should be aware of
locks if/when they exist.

## Implementation Plan

### Phase 1: Trash infrastructure

Add the `.trash/` directory support to `jp_storage`:

- `Storage::trash_conversation(id, dirname, error)` — moves a conversation
  directory to `.trash/` and writes `TRASHED.md`.
- Handle the case where a trashed conversation with the same name already
  exists.
- Unit tests with temp directories.

**Dependency:** None.
**Mergeable:** Yes.

### Phase 2: `Workspace::sanitize()`

Implement the sanitization logic in `jp_workspace`:

- `SanitizeReport` and `TrashedConversation` types.
- The validation loop: parse dirname, deserialize metadata, validate events.
- Global `conversations/metadata.json` validation and recovery from corrupt JSON.
- Active conversation reassignment (runs unconditionally, not only after
  trashing).
- Fresh-workspace detection: if no conversations exist on disk and nothing was
  trashed, return immediately without touching metadata.
- Remove the fallback loop and FIXME in `load_conversations_from_disk` —
  sanitize makes them redundant.
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

- [Issue #404][#404] — clean install fails due to unresolvable conversation ID.
- [PR #450][#450] — partial fix for fresh workspace handling.
- [RFD 006] — `ConversationStream::sanitize()`, the event-level precedent for
  this pattern.
- [RFD 020] — parallel conversations and file locking (relevant to concurrent
  access risk).
- [RFD 023] — resumable conversation turns, interaction with `sanitize()`.

[#404]: https://github.com/dcdpr/jp/issues/404
[#450]: https://github.com/dcdpr/jp/pull/450
[RFD 006]: 006-turn-scoped-mutations.md
[RFD 020]: 020-parallel-conversations.md
[RFD 023]: 023-resumable-conversation-turns.md
