# RFD D49: Conversation Export and Import

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-12
- **Summary**: Adds `jp conversation export` and `import` subcommands to
  transfer self-contained conversation JSON envelopes across workspaces.

## Summary

This RFD adds `jp conversation export` and `jp conversation import`.
`export` writes conversations to stdout as versioned, self-contained JSON
envelopes; `import` validates envelopes from stdin and writes them into a target
workspace.
Combined with the global `--workspace` flag, the two commands let users pipe
conversations directly between workspaces.

## Motivation

Conversations are bound to the workspace that created them.
There is no supported way to move a conversation to another workspace; copying
directories by hand bypasses validation, ID collision checks, and the storage
backend.

Conversation data is already portable.
Each conversation carries a full config snapshot (`base_config.json` plus
`ConfigDelta` events), and a resumed conversation's config layer masks the
workspace's file/env layer for every key it sets.
A relocated conversation behaves the same regardless of the target workspace's
configuration; it only lacks a supported transport.

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
using the same resolution as other conversation commands.
It is read-only and takes no conversation lock.

`import` reads envelopes from stdin until EOF, validates the entire batch, and
then writes each conversation through the target workspace's storage backend.
If any envelope fails to parse or any conversation ID already exists in the
target workspace, `import` exits with a non-zero code without writing anything.

Export copies conversations rather than moving them.
Because a pipe cannot report the import's outcome back to the exporting process,
`export` never deletes the source conversation.
Users remove the source explicitly with `jp c rm`.

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

- **Versioning**: An envelope without a `version` field is version 1.
  `import` treats a missing `version` as `1` and rejects unknown version
  numbers.
  Shipping this check in the first importer lets a future version 2 add an
  explicit `version` field that older binaries cleanly reject rather than
  misparsing.
  Because scripts will depend on the stdout format (Hyrum's Law), changes within
  version 1 are additive only.
- **Payload**: `metadata`, `base_config`, and `events` use the canonical JSON
  serialization from `jp_conversation` (the output of
  `ConversationStream::to_parts`), including base64-encoded content fields.
  Round-trips are lossless, and `import` reuses the tolerant storage
  deserialization path, which preserves or ignores unknown fields in events and
  config deltas.
- **Locality**: `projection` records the conversation's storage locality as the
  write intent for `import`.

### Preserving projection

[RFD 031] removes the stored `Conversation::user` field: storage locality is a
write intent (`Projection`) carried by the lock and derived at load time from
the storage roots that hold the conversation (`StoragePresence`).

`export` derives the envelope's `projection` field from the source presence:

| Source presence        | Envelope value |
| ---------------------- | -------------- |
| user-local only        | `local`        |
| projected (both roots) | `projected`    |
| workspace-only (`ext`) | `projected`    |

`ext` is a transitional `StoragePresence` for a conversation committed by
another contributor to the shared workspace directory.
Because the conversation was shared in the source workspace, `export` maps `ext`
to `projected` and never writes `ext` to the wire format.

`import` writes the conversation using the envelope's `projection` rather than
the target workspace's `conversation.start_local` setting.
A `local` conversation is often kept out of version control because it is
private; defaulting to `conversation.start_local` could silently write it to
`.jp/conversations/` where it might be committed.
Users can change locality after import with `jp c edit --local`.

### Excluded state

- **Session mappings and locks**: Per-workspace runtime state.
  `import` creates fresh state in the target workspace.
- **Mount approvals**: Approvals bind workspace-relative paths to canonical host
  paths in user-workspace storage.
  The target workspace prompts again on first use.
- **Tool definitions**: The config snapshot references tools by name, which the
  target workspace may not define.
  `import` warns when the envelope's config references tools unknown to the
  target workspace and still imports the conversation.

Attachment content is snapshotted at attach time ([RFD 065]) and travels inside
the conversation, so only paths resolved after relocation can break.

### Workspace targeting

`-w <id>` resolution follows the non-interactive rule in [RFD 087].
Because `import` reads envelopes from stdin, it cannot prompt on stdin: if a
workspace ID matches multiple live checkouts, `import` exits with an error
listing the matching roots.
Scripts that need deterministic targeting should pass a workspace path to `-w`.
Any future interactive prompts use the terminal channels defined in [RFD 048].

### Backend independence

Neither command requires changes to the storage backend traits, and no process
ever holds two storage backends.
`export` reads through the source workspace's `LoadBackend`; `import` checks
collisions via `load_conversation_index` and writes through the target
workspace's `PersistBackend` using the `Projection` argument from [RFD 031].

Because the envelope format is the public interchange contract, it serializes
domain objects (`Conversation` and `ConversationStream`) rather than backend
internals.
`LoadBackend` and `PersistBackend` already translate between backend storage and
domain objects, so cross-backend transfers (such as exporting from a future
sqlite-backed workspace into a filesystem-backed one) work without a
format-translation matrix.
Backend-specific dumps (such as SQL dumps from an sqlite backend) belong to
backup tooling rather than conversation interchange and are out of scope.

### Disambiguating "import"

[RFD 031] uses "import" internally for pulling workspace-committed (`ext`)
conversations into user-local storage.
This RFD uses `import` for the user-facing CLI subcommand.
The ubiquitous-language glossary distinguishes the two terms: *import (CLI)*
ingests conversation envelopes from stdin, while *import (projection)* is the
lazy adoption of `ext` conversations into user-local storage.

## Drawbacks

- **Permanent wire format**: Versioning bounds the maintenance cost of the
  envelope format, but every future storage change still has to account for the
  wire representation.
- **No atomic move**: Moving a conversation takes two steps (`export | import`,
  then `rm`), leaving a window where the conversation exists in both workspaces.
- **Base64 content encoding in the wire format**: Reusing the storage layer's
  base64 encoding leaks a storage detail into the envelope, though it keeps v1
  lossless and simple to implement.
  A future envelope version can switch to plain content if needed.

## Alternatives

- **In-process `jp c mv --to <workspace>`**: Bootstrapping two workspaces in one
  process would require restructuring `jp_cli` startup without adding
  capability.
  It also would not make the move atomic, since writing to the target and
  removing from the source remain separate filesystem operations.
- **Cross-backend copy method on backend traits**: In the pipe model, each
  process uses only its own workspace's backend, so there is no cross-backend
  operation to abstract.
- **Raw directory copy (documented manual procedure)**: Copying directories by
  hand bypasses ID collision checks, validation, and the backend abstraction,
  and breaks silently when the storage layout changes.

## Non-Goals

- **Move semantics**: `export` is non-destructive; removing the source
  conversation is a separate step.
- **Projection override flags on `import`**: `import` always honors the
  envelope's `projection`.
  Users change locality afterwards with `jp c edit --local`.
- **Collision resolution**: `import` refuses colliding conversation IDs.
  Re-ID or rename strategies are future work.
- **Cross-machine portability guarantees**: The config snapshot in an envelope
  may contain absolute paths and user-specific values from the source machine.
  The format does not preclude cross-machine use, but only same-machine transfer
  is supported.
- **Archived conversations**: `export` operates only on active conversations.
  Archived conversations must be unarchived first.

## Risks and Open Questions

- **Secrets in the envelope**: `base_config.json` snapshots resolved config,
  which can include sensitive values.
  The on-disk snapshot has the same exposure, but piping to stdout makes it easy
  to redirect output into files or paste it into chat messages.
  The command's help text should note this risk.
- **Envelope size**: Long conversations produce large single lines, and
  validating the full batch before writing requires buffering all parsed
  envelopes in memory during `import`.
  Typical conversation sizes make in-memory buffering acceptable for v1.
- **Sequencing**: This RFD is gated on [RFD 031] (write path, `Projection`, and
  `load_conversation_index`) and [RFD 087] (`-w <id>` resolution).
  Both gates are enforced by the RFD dependency mechanism.

## Implementation Plan

### Phase 1: `jp c export`

Serialize envelopes from `ConversationStream::to_parts`, metadata, and the
derived `projection`, and write them as NDJSON to stdout.
This phase is useful standalone for backups and inspection, and depends on [RFD
031] for `StoragePresence`.

### Phase 2: `jp c import`

Parse envelopes from stdin, check the format version, check for ID collisions
via `load_conversation_index`, write conversations via `PersistBackend::write`
with the envelope's `projection`, and warn on unknown tools.
This phase depends on Phase 1 for the envelope format and [RFD 087] for `-w
<id>` resolution.

### Phase 3: Documentation and glossary

Document the export/import workflow and add the glossary entry disambiguating
the two senses of "import".
This phase can merge with Phase 2.

## References

- [RFD 031], *Durable Conversation Storage with Workspace Projection*: defines
  the storage model that `import` writes against.
- [RFD 048], *Four-Channel Output Model*: explains why `import` never prompts on
  stdin.
- [RFD 065], *Typed Resource Model for Attachments*: explains why attachment
  content survives relocation.
- [RFD 087], *Session-Scoped Active Workspace*: defines `-w <id>` resolution
  semantics.

[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 087]: ../087-session-scoped-active-workspace.md
