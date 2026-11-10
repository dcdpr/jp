<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.

  DRAFT NOTE: This RFD deliberately records *no* `Requires`/`Extends` edge to
  RFD 065/066/067. Those gate promotion, and the whole point of this design is
  to ship external attachments without waiting on the blob-store migration. The
  relationship to RFD D03 / 065 / 066 is documented in prose (see "Migration to
  the Blob-Backed Design") and must be turned into proper links — or D03
  reconciled — before this draft is promoted, since published RFDs cannot link
  to drafts.

  The `Extends: RFD 031` edge is intentional: this design builds on 031's
  source-of-truth + projection storage model. It is `Extends` rather than
  `Requires` because 031 is Implemented — a `Requires` on an Implemented target
  is redundant and the docs build rejects it — and because the relationship is
  design lineage worth keeping past implementation.
-->

# RFD D56: Conversation-Local External File Attachments

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-04
- **Extends**: [RFD 031]
- **Summary**: Snapshots files attached from outside the workspace into
  conversation-local storage under an `external:` URI scheme.

## Summary

This RFD adds support for attaching files from outside the workspace root (such
as `jp q --attach /tmp/data.json`) by snapshotting their content into the
conversation's storage directory under an `external:` URI scheme.
It provides an interim implementation within JP's reference-based attachment
model and defines a migration path to the blob-backed resource model.

## Motivation

JP's file attachment handler resolves paths relative to the workspace root.
`AttachmentUrlOrPath::parse` (`jp_cli/src/parser.rs`) checks the cleaned path
against the workspace root with `strip_prefix` and rejects any path outside it
before consulting a handler:

```
$ jp q -a /var/folders/ny/.../T/jp-cerebras-request-15516.json "..."
 error  Attachment error: Attachment path must be relative to the workspace:
        /var/folders/ny/.../T/jp-cerebras-request-15516.json
```

This blocks attaching a downloaded document, a spec from another project, or an
ephemeral debugging dump under `/tmp` or `/var/folders/.../T/`.
The only workaround is copying the file into the workspace first, which pollutes
the project directory with unrelated files.

For ephemeral files, the path is temporary and local to the user's machine; only
the file's content at attach time matters, which a snapshot captures.

### Portable attachment references

Storing a raw absolute path as the canonical reference leaks the user's
filesystem layout (such as their username and directory structure) into
conversation state and cannot resolve on any other machine.
Because JP conversations are shared through Git, attachment references must be
portable.

### Sequencing with the blob-backed design

The long-term design (RFD D03) routes external content into a
content-addressable blob store ([RFD 066]) through a typed resource model ([RFD
065]) with deduplication ([RFD 067]).
All three RFDs are still in Discussion and unbuilt, and D03's first phase
depends on them.
Rather than blocking external attachments on that migration, this RFD implements
them on JP's current reference-based attachment model and defines a migration
path to the blob-backed design.

## Design

### User-facing behavior

The existing `--attach` flag accepts paths outside the workspace without adding
new flags:

```sh
jp q --attach /tmp/data.json "Parse this"
jp q --attach ~/Downloads/report.pdf "Summarize this"
```

Path resolution follows two rules:

- A path **inside** the workspace root resolves to a `file:` URI.
- A path **outside** the workspace root is read and snapshotted immediately, and
  recorded as an `external:` URI.

Content is captured once at attach time.
External attachments are **not refreshable** because no live source path is
retained to re-read.

### The `external:` URI scheme

An external attachment is identified by an opaque ID:

```
external:<id>
```

`<id>` is a short, randomly generated token unique within the conversation.
The original absolute path is **not** stored in conversation state, keeping the
user's filesystem layout private and the reference portable.

The original filename is preserved in attachment metadata (the `source` or
description field) for LLM readability and for `jp attachment ls`, but it is not
part of the canonical URI.

### Conversation-local snapshot storage

Under [RFD 031], a conversation's source of truth is the user-local store, and
projected conversations are mirrored under `.jp/conversations/` so they can be
committed to Git.
A snapshot is stored as **conversation content** alongside `events.json` in the
source-of-truth store and copied with the conversation whenever it is projected
into the workspace.
Copying snapshot sidecars during projection is the only change this design adds
to [RFD 031]'s storage layer.

Resolution remains lazy and reference-based: at query time, the `external`
handler reads the snapshot from the conversation's storage directory, just as
the `file` handler reads from the workspace.
File content never enters the configuration or event stream.

An external attachment therefore shares its conversation's lifecycle and
portability:

| Conversation                            | External attachment                                        |
| --------------------------------------- | ---------------------------------------------------------- |
| Local-only (`--local`, never projected) | Remains on the local machine and is never shared           |
| Projected and committed                 | Travels with the projection and resolves on other machines |
| Absent on a machine                     | Absent alongside the conversation                          |
| Deleted                                 | Reclaimed from both stores with the conversation           |

Because snapshots are not shared across conversations, deleting a conversation
reclaims its snapshots from both stores without reference counting or separate
garbage collection.

### Graceful degradation

If a snapshot is missing at resolution time (for example, when a shared
conversation's storage directory was only partially copied), the handler **warns
and skips** the attachment instead of aborting the query.
This matches how `load_conversation_attachments` handles unavailable `jp://`
targets.

### Components

- **`jp_attachment_external`**: A new handler crate registering the `external`
  scheme via `Handler` and `distributed_slice(HANDLERS)`.
  Its `add()` method reads and snapshots the source file into the conversation
  directory, and its `get()` method reads the snapshot back and returns an
  `Attachment`.
- **Parser** (`jp_cli/src/parser.rs`): Routes out-of-workspace paths to the
  `external` handler instead of rejecting them, reusing the existing size guard
  (`MAX_BINARY_SIZE`, 10 MiB) and binary/text detection.

## Drawbacks

- **No deduplication.** Attaching the same file to two conversations stores two
  copies.
  Cross-conversation deduplication is deferred to the blob store ([RFD 067]).
- **Increased conversation storage.** Each external attachment copies the source
  file into the conversation directory, increasing storage size and Git
  repository size when the conversation is projected and committed.
  `MAX_BINARY_SIZE` bounds the size of each attachment.
- **No sharing for uncommitted conversations.** If a conversation's `.jp`
  storage is not committed, teammates have neither the conversation nor its
  snapshots, matching the behavior of attachments to gitignored workspace files.
- **Not refreshable.** Snapshot content is frozen at attach time.
  If the source file changes, the user must re-attach it.

## Alternatives

### Inline content in conversation config

Storing base64-encoded bytes directly in `conversation.attachments` is
**rejected** because attachment changes are recorded as config deltas in
`events.json`.
Inlining content puts binary blobs into the durable, diffable event stream,
bloating `events.json`, making the configuration unreadable, and re-serializing
the blob on every delta.
Once supported, users inevitably attach large files such as 10 MB PDFs (Hyrum's
Law).

### Snapshot into the user-local store and gitignore it

Copying snapshots only into `~/.local/share/jp/...` instead of the conversation
directory is **rejected** because a committed conversation would travel via Git
without its attachments.
The conversation would load on other machines with silently missing content,
failing only when the attachment is read back.

### Store the original absolute path

Recording `file:///Users/jean/Downloads/report.pdf` as the attachment reference
is **rejected** because it leaks the user's filesystem structure into shared
state and cannot resolve on other machines.

### Wait for the blob-backed design (RFD D03 / 065 / 066 / 067)

The blob-backed architecture is the long-term destination, but it is blocked on
three unbuilt RFDs ([RFD 065], [RFD 066], and [RFD 067]).
This RFD ships external attachments now and migrates them into the blob-backed
design once those RFDs land.

## Non-Goals

- **Content-addressable blob store or resource model.** Covered by [RFD 065] and
  [RFD 066].
- **Deduplication.** Covered by [RFD 067].
- **Tool access to external files.** This RFD covers only user-initiated
  `--attach`; tools remain restricted to the workspace.
- **Cross-machine sharing of attachments for uncommitted conversations.**
  External attachments travel only when their conversation is projected and
  committed.

## Risks and Open Questions

- **Projection must include snapshot sidecars.** [RFD 031]'s projection logic
  copies conversation content into and out of the workspace store; snapshot
  sidecars must be included in that copy so projected conversations retain their
  external attachments on other machines.
  The exact on-disk layout of the sidecar within the conversation directory will
  be decided during implementation.
- **Non-UTF-8 filenames.** Preserved filename metadata must tolerate or reject
  non-UTF-8 names consistently with JP's `Utf8Path` requirement.
- **Missing-snapshot UX.** Beyond warning and skipping at query time, `jp
  attachment ls` should indicate when an external attachment's snapshot is
  unavailable on the local machine.

## Migration to the Blob-Backed Design

When [RFD 065] (resource model) and [RFD 066] (blob store) land, conversations
with sidecar snapshots migrate through a local storage rewrite:

1. Hash the bytes (SHA-256) of each `external:<id>` snapshot and write them into
   the blob store.
2. Rewrite the resource reference from the sidecar file to a `$blob` checksum.
3. Keep the `external:<id>` URI visible to the user and LLM unchanged.

Because migration reads the stored snapshot rather than the original file, it
cannot fail if the source file has moved or been deleted.

Migration preserves two compatibility properties:

- **Stable `external:` URI surface.** Users and scripts that reference
  `external:<id>`, along with the `external` handler keyed on that scheme,
  continue working before and after migration.
  Retaining the sidecar reader in the `external` handler supports unmigrated
  conversations.
- **Sidecar snapshots do not join path-based deduplication.** RFD D03 keys
  deduplication on a hash of the original parent directory, which sidecar
  snapshots discard.
  Migrated snapshots retain their opaque IDs and do not participate in [RFD 067]
  deduplication, which does not affect the ephemeral files that motivate this
  design.

Conversations without `external:` attachments are unaffected.

## Implementation Plan

### Phase 1: Parser and handler

Route out-of-workspace paths in `AttachmentUrlOrPath::parse` to the `external`
handler instead of rejecting them.
Add `jp_attachment_external` with `add()` snapshotting content into the
conversation directory and `get()` reading it back, reusing the existing size
guard and binary/text detection.
This phase is reviewable and mergeable independently.

### Phase 2: Listing and missing-snapshot UX

Display external attachments in `jp attachment ls` with an indicator
distinguishing them from workspace files, indicate when a snapshot is
unavailable on the local machine, and wire the warn-and-skip path for missing
snapshots.

## References

- [RFD 031: Durable Conversation Storage with Workspace Projection][RFD 031]
- [RFD 052: Workspace Data Store Sanitization][RFD 052]
- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- [RFD 066: Content-Addressable Blob Store][RFD 066]
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067]
- RFD D03: External Attachment URI Scheme (draft) defines the blob-backed
  evolution of this design.

[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 052]: ../052-workspace-data-store-sanitization.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
