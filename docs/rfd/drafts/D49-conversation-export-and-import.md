# RFD D49: Conversation Export and Import

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-12
- **Requires**: [RFD 031], [RFD 087]

## Summary

This RFD adds `jp conversation export`, which writes conversations to stdout as
versioned, self-contained JSON envelopes, and `jp conversation import`, which
reads envelopes from stdin and writes them into the target workspace.
Combined with the global `--workspace` flag, the pair enables copying
conversations between workspaces:

```sh
jp c export jp-c123 jp-c456 | jp -w <workspace-id> c import
```

## Motivation

Conversations are bound to the workspace that created them.
There is no supported way to move a conversation to another workspace — the
only workaround is copying directories by hand, which bypasses validation, ID
collision checks, and the storage backend entirely.

Conversations are already self-contained where it matters: each one carries a
full config snapshot (`base_config.json` plus `ConfigDelta` events), and a
resumed conversation's config layer masks the workspace's file/env layer for
every key it sets.
A conversation therefore behaves identically after relocation, regardless of how
different the target workspace's configuration is.
What is missing is not portability of the data but a supported transport for it.

## Design

### Command surface

```sh
# Export the active conversation to stdout.
jp c export

# Export specific conversations.
jp c export jp-c17528832001 jp-c17528832002

# Copy into another workspace.
jp c export jp-c17528832001 | jp -w <workspace-id> c import

# Or with a workspace path, which avoids multi-checkout ambiguity in scripts.
jp c export jp-c17528832001 | jp -w ~/projects/other c import
```

`export` defaults to the session's active conversation when no IDs are given,
following the same resolution as other conversation commands.
It is read-only and takes no conversation lock.

`import` reads envelopes from stdin until EOF, validates all of them, then
writes each conversation through the target workspace's storage backend.
Validation happens before any write: if any envelope fails to parse or any
conversation ID already exists in the target, the import aborts with a non-zero
exit code and writes nothing.

This is a **copy, not a move**.
The pipe cannot report the import's outcome back to the exporting process, so
export never deletes anything.
Removing the source conversation afterwards is an explicit `jp c rm`.

### Envelope format

Export writes one JSON object per conversation, newline-delimited (NDJSON):

```json
{
  "id": "jp-c17528832001",
  "projection": "local",
  "metadata": { ... },
  "base_config": { ... },
  "events": [ ... ]
}
```

- The format is implicitly versioned: an envelope without a `version` field is
  version 1.
  Import treats a missing `version` as `1` and rejects any `version` value it
  does not know.
  This reader-side rule ships with the first importer — it is what allows a
  future version 2 to add an explicit `version` field and be cleanly rejected by
  older binaries instead of silently misparsed.
  The moment this format hits stdout, scripts depend on it (Hyrum's Law);
  evolution within version 1 is additive only.
- `metadata`, `base_config`, and `events` carry the canonical JSON serialization
  defined by `jp_conversation` (the same values `ConversationStream::to_parts`
  produces), including the base64 encoding of content fields.
  This makes the round-trip trivially lossless and reuses the existing tolerant
  deserialization path (unknown fields in events and config deltas are preserved
  or ignored exactly as they are when loading from storage).
- `projection` records where the conversation lives, as the write intent for
  import (see below).

### Projection travels with the conversation

[RFD 031] removes the stored `Conversation::user` field: storage locality is a
write intent (`Projection`) carried by the lock and derived at load time from
which storage roots hold the conversation (`StoragePresence`).

Export derives the envelope's `projection` field from the source presence:

| Source presence        | Envelope value |
| ---------------------- | -------------- |
| user-local only        | `local`        |
| projected (both roots) | `projected`    |
| workspace-only (`ext`) | `projected`    |

`ext` is a transitional presence, not an intent — a conversation committed by
another contributor was in the shared directory, so shared is its last known
state.
It never appears in the wire format.

Import uses the envelope's `projection` as the write intent.
This is deliberate: a `local` conversation is often local *on purpose* (private,
kept out of version control).
If import defaulted to the target workspace's `conversation.start_local`, a
private conversation could silently land in `.jp/conversations/` and be
committed.
Preserving the exported state avoids that leak.
To change locality after import, use `jp c edit --local`.

### What does not travel

- **Session mappings and locks.** Per-workspace runtime state; import creates
  fresh state in the target.
- **Mount approvals.** Approvals bind workspace-relative paths to canonical host
  paths in user-workspace storage; the target workspace re-prompts on first use.
- **Tool definitions.** The config snapshot references tools by name; the target
  workspace may not define them.
  Import warns when the envelope's config references tools unknown to the
  target, and the conversation otherwise imports normally.

Attachment portability is already bounded by [RFD 065]: attachment content is
snapshotted at attach time and travels inside the conversation, so only paths
resolved *after* the move can break.

### Interaction with workspace targeting

`-w <id>` resolution follows [RFD 087]: a piped import is non-interactive (stdin
carries data, not answers), so a workspace ID with multiple live checkouts is an
error listing the roots, per 087's non-interactive rule.
Scripts that need determinism should pass a workspace path.
Import never prompts on stdin; any future interaction goes through the terminal
channels per [RFD 048].

### No backend trait changes

No process ever holds two storage backends.
Export reads through the source workspace's `LoadBackend`; import writes through
the target workspace's `PersistBackend` (with the `Projection` argument from
[RFD 031]) and checks collisions via `load_conversation_index`.
Both commands work with any backend implementation through the existing trait
surface.
The new public contract is the envelope format, not a trait method.

The envelope is deliberately **backend-independent**: it serializes domain
objects (`Conversation`, `ConversationStream`), not backend internals.
The backends are the adapters — `LoadBackend` produces domain objects from
whatever the source stores, and `PersistBackend` translates them into whatever
the target stores.
This is what makes cross-backend transfer (e.g. a future sqlite-backed workspace
exporting into a filesystem-backed one) work without any format-translation
matrix: one interchange format, N backend adapters that already exist as the
load/persist traits.
A per-backend export format (e.g.
SQL dumps from an sqlite backend) would be backup tooling for that backend, not
conversation interchange, and is out of scope here.

### Naming: "import" is now two things

[RFD 031] uses "import" internally for pulling workspace-committed (`ext`)
conversations into user-local storage.
This RFD claims the word for the CLI surface, which has the stronger claim on
user-facing vocabulary.
The ubiquitous-language glossary gains an entry distinguishing the two: *import
(CLI)* ingests envelopes from stdin; *import (projection)* is the lazy
user-local adoption of `ext` conversations.

## Drawbacks

- The envelope format is a forever contract.
  Versioning contains the cost but does not remove it: every future storage
  change must consider the wire format.
- No atomic move.
  Users who want move semantics perform two steps (`export | import`, then
  `rm`), with a window where the conversation exists in both workspaces.
- The base64 content encoding leaks a storage concern into the wire format.
  Accepted for losslessness and implementation simplicity; a future envelope
  version can switch to plain content if needed.

## Alternatives

- **In-process `jp c mv --to <workspace>`.** Rejected: requires bootstrapping
  two workspaces in one process, a structural change to `jp_cli` startup for no
  capability gain — and it cannot be more atomic than the pipe in practice,
  since the write to the target and the removal from the source are separate
  filesystem operations either way.
- **Expanding the backend traits with a cross-backend copy operation.**
  Rejected: the pipe model means each side uses its own backend; there is no
  cross-backend operation to abstract.
- **Raw directory copy (documented manual procedure).** Rejected: bypasses ID
  collision checks, validation, and the backend abstraction; breaks silently
  when the storage layout changes.

## Non-Goals

- **Move semantics.** Export is non-destructive; source removal is an explicit
  separate step.
- **Projection override flags on import.** Import always honors the envelope's
  `projection`; locality is changed afterwards with `jp c edit --local`.
- **Collision resolution.** Import refuses colliding conversation IDs; re-ID or
  rename strategies are future work.
- **Cross-machine portability guarantees.** The envelope may contain absolute
  paths and user-specific values from the source machine's config snapshot.
  Same-machine transfer is the supported case; the format does not preclude
  cross-machine use, but this RFD makes no promises about it.
- **Archived conversations.** Export operates on active conversations; unarchive
  first.

## Risks and Open Questions

- **Secrets in the envelope.** `base_config.json` snapshots resolved config,
  which can include sensitive values.
  This is the same exposure the on-disk snapshot already has, but a pipe invites
  redirection to files and chat messages.
  Worth a note in the command's help text.
- **Envelope size.** Long conversations produce large single lines.
  NDJSON consumers handle this fine, but `import` should stream-parse rather
  than buffer all envelopes when memory matters.
  Validation-before-write requires buffering parsed envelopes; for v1 the
  conversation sizes involved make this acceptable.
- **Sequencing.** This RFD is gated on [RFD 031] (write path, `Projection`,
  `load_conversation_index`) and [RFD 087] (robust `-w <id>` resolution).
  Both gates are enforced by the RFD dependency mechanism.

## Implementation Plan

### Phase 1: `jp c export`

Envelope serialization from `ConversationStream::to_parts` plus metadata and
derived projection; NDJSON to stdout.
Useful standalone (backups, inspection).
Depends on [RFD 031] for `StoragePresence`.

### Phase 2: `jp c import`

Stdin parsing, version check, collision check via `load_conversation_index`,
writes via `PersistBackend::write` with the envelope's projection, unknown-tool
warnings.
Depends on Phase 1 for the format and [RFD 087] for `-w <id>` resolution.

### Phase 3: Documentation and glossary

User documentation for the export/import workflow and the glossary entry
disambiguating the two senses of "import".
Can merge with Phase 2.

## References

- [RFD 031]: Durable Conversation Storage with Workspace Projection — the
  storage model import writes against.
- [RFD 048]: Four-Channel Output Model — why import never prompts on stdin.
- [RFD 065]: Typed Resource Model for Attachments — why attachment content
  survives relocation.
- [RFD 087]: Session-Scoped Active Workspace — `-w <id>` resolution semantics.

[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 087]: ../087-session-scoped-active-workspace.md
