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

  The `Requires: RFD 031` edge is intentional: this design builds on 031's
  source-of-truth + projection storage model. It does not block promotion, since
  031 is Implemented.
-->

# RFD D49: Conversation-Local External File Attachments

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-04
- **Requires**: [RFD 031]

## Summary

This RFD adds support for attaching files from outside the workspace root (e.g.
`jp q --attach /tmp/data.json`) by snapshotting their content into the
conversation's own storage at attach time and referencing it through a new
`external:` URI scheme.
It is a deliberately scoped interim design that stays within JP's existing
reference-based attachment model — no blob store, no resource model migration
— and documents a clean migration path to the blob-backed design once that
infrastructure exists.

## Motivation

JP's file attachment handler resolves paths relative to the workspace root.
Any path that does not fall under the root is rejected before a handler is ever
consulted:

```
$ jp q -a /var/folders/ny/.../T/jp-cerebras-request-15516.json "..."
 error  Attachment error: Attachment path must be relative to the workspace:
        /var/folders/ny/.../T/jp-cerebras-request-15516.json
```

The rejection lives in `AttachmentUrlOrPath::parse` (`jp_cli/src/parser.rs`),
which `strip_prefix`-checks the cleaned path against the workspace root.

This blocks a common workflow: attaching a downloaded document, a spec from
another project, or — the motivating case here — an ephemeral debugging dump
under `/tmp` or `/var/folders/.../T/`.
Today the only workaround is to copy the file into the workspace first, which
pollutes the project directory with unrelated files.

For ephemeral files the path itself is pure noise: it won't exist tomorrow and
means nothing to anyone else.
What matters is the *content* at attach time.
That is exactly the semantic a snapshot provides.

### Why not just allow absolute paths?

Storing the raw absolute path as the canonical reference leaks the user's
filesystem structure (username, directory layout) into conversation state and is
meaningless on any other machine.
JP conversations are shareable via git, so the reference has to be portable.

### Why not wait for the blob-backed design?

The eventual design (RFD D03) routes external content into a content-addressable
blob store ([RFD 066]) recorded through a typed resource model ([RFD 065]), with
deduplication ([RFD 067]).
All three are still in Discussion and unbuilt, and D03's first phase depends on
them.
That makes external attachments hostage to a large migration.
This RFD delivers the capability now on the model JP already has, and treats the
blob-backed design as the documented evolution.

## Design

### User-facing behavior

No new flags.
The existing `--attach` flag accepts an out-of-workspace path:

```sh
jp q --attach /tmp/data.json "Parse this"
jp q --attach ~/Downloads/report.pdf "Summarize this"
```

Resolution rules:

- Path resolves **inside** the workspace root → `file:` URI, as today.
  No change.
- Path resolves **outside** the workspace root → the content is read and
  snapshotted now, and the attachment is recorded as an `external:` URI.

The content is captured once, at attach time.
External attachments are **not refreshable** — there is no live source to
re-read.
This matches the snapshot semantics every external attachment design assumes.

### The `external:` URI scheme

An external attachment is identified by an opaque id:

```
external:<id>
```

The `<id>` is a short, randomly generated, conversation-unique token.
The original absolute path is **not** stored anywhere in conversation state,
which keeps the user's filesystem structure private and the reference portable.

The original filename is preserved as attachment metadata (the `source` /
description field) for LLM readability and for `jp attachment ls`, but it is not
part of the canonical URI.

### Storage: the snapshot lives with the conversation

Under [RFD 031], a conversation's source of truth is the user-local store, and a
workspace projection under `.jp/conversations/` exists when the conversation is
projected (so it is git-visible and committable).
The snapshot is treated as **conversation content**: it is written alongside
`events.json` in the source-of-truth store and, whenever the conversation is
projected into the workspace, projected with it.
Projection must copy the snapshot sidecars, not just `events.json` — that is
the one hook this design adds to 031's storage machinery.

Resolution stays lazy and reference-based: at query time the `external` handler
reads the snapshot from the conversation's storage, exactly as the `file`
handler reads from the workspace today.
Content never enters the config or event stream.

An external attachment therefore shares the conversation's exact lifecycle and
portability:

| Conversation                            | External attachment                   |
| --------------------------------------- | ------------------------------------- |
| Local-only (`--local`, never projected) | Stays on this machine, never shared   |
| Projected and committed                 | Travels with the projection. Resolves |
| Absent on a machine                     | Neither conversation nor snapshot     |
| Deleted                                 | Reclaimed from both stores with it    |

Cleanup is free: there is no cross-conversation sharing to refcount, so deleting
a conversation reclaims its snapshots (in both stores) with no separate garbage
collection.

### Graceful degradation

If a snapshot is missing at resolution time (for example, the conversation was
shared but its storage directory was partially copied), the handler **warns and
skips** the attachment rather than aborting the query — the same tolerance
`load_conversation_attachments` already applies to unavailable `jp://` targets.
A missing external attachment must never fail an otherwise valid query.

### Components

- **`jp_attachment_external`** — a new handler crate registering the `external`
  scheme.
  Its `add()` reads and snapshots the source file; its `get()` reads the
  snapshot back from the conversation directory and returns an `Attachment`.
  It follows the existing `Handler` trait and registration pattern
  (`distributed_slice(HANDLERS)`).
- **Parser** (`jp_cli/src/parser.rs`) — out-of-workspace paths are routed to
  the external path instead of being rejected.
  The existing size guard (`MAX_BINARY_SIZE`, 10 MiB) and binary/text detection
  are reused unchanged.

## Drawbacks

**No deduplication.** Attaching the same file to two conversations stores two
copies.
For the interim this is acceptable; cross-conversation dedup is the job of the
future blob store ([RFD 067]).

**Snapshots inflate conversation storage.** Each external attachment is a copy
under the conversation directory.
Large attachments make conversations larger to store, and to commit if the
workspace is shared.
The existing size cap bounds the worst case per attachment.

**Uncommitted conversations don't share their attachments.** If a conversation's
`.jp` storage is not committed, a teammate has neither the conversation nor its
snapshots.
This is the same rule that already governs attachments to gitignored workspace
files, but it is worth stating plainly.

**Not refreshable.** Once attached, content is frozen.
If the source changes the user must re-attach.
Inherent to the snapshot model.

## Alternatives

### Inline the content in conversation config

Store the bytes directly in `conversation.attachments` config (base64).
Rejected: attachment changes are recorded as config deltas in `events.json`, so
this puts binary blobs in the durable, diffable event stream — bloating it,
breaking the "config is small and human-readable" property, and re-serializing
the blob on every delta.
Tolerable for tiny text, a trap for anything larger (Hyrum's Law: once it
exists, someone attaches a 10 MB PDF).

### Snapshot into the user-local store and gitignore it

Copy into `~/.local/share/jp/...` instead of the conversation directory.
Rejected: the conversation would travel via git but its attachment would not,
producing a conversation that loads yet is silently missing content on other
machines — the worst failure mode, because it is invisible until the attachment
is read back.

### Store the original absolute path

Use `file:///Users/jean/Downloads/report.pdf` as the reference.
Rejected: leaks filesystem structure into shared state and is meaningless on
other machines.

### Wait for the blob-backed design (RFD D03 / 065 / 066 / 067)

The durable destination, but blocked on three unbuilt RFDs.
This RFD ships the capability now and migrates into that design later.

## Non-Goals

**A content-addressable blob store or resource model.** Out of scope; that is
[RFD 066] / [RFD 065].

**Deduplication.** Out of scope; that is [RFD 067].

**Tool access to external files.** This RFD covers only user-initiated
`--attach`.
Tools remain restricted to the workspace.

**Cross-machine sharing of attachments to uncommitted conversations.** External
attachments travel exactly when their conversation does, and no further.

## Risks and Open Questions

**Projection must carry the sidecars.** [RFD 031]'s projection logic copies
conversation content into and out of the workspace store; the snapshot sidecars
must be included in that copy, or projected conversations lose their external
attachments on other machines.
The exact on-disk layout of the sidecar within the conversation directory is
pinned during implementation.

**Non-UTF-8 filenames.** The preserved filename metadata must tolerate or reject
non-UTF-8 names consistently with JP's existing `Utf8Path` requirement.

**Missing-snapshot UX.** Beyond warn-and-skip, `jp attachment ls` should make it
visible that an external attachment's content is unavailable on this machine, so
the absence is diagnosable rather than mysterious.

## Migration to the Blob-Backed Design

When [RFD 065] (resource model) and [RFD 066] (blob store) land, a D49-era
conversation migrates by a purely local re-storage:

1. For each `external:<id>` snapshot, hash the bytes (SHA-256) and write them
   into the blob store.
2. Rewrite the resource reference from "sidecar file" to a `$blob` checksum.
3. The `external:` URI surface the user and LLM see stays unchanged.

No original file is ever re-read (it is gone by design), so migration cannot
fail on a moved or deleted source.

Two compatibility facts worth recording:

- **The `external:` scheme is the stable surface.** Users and scripts that
  reference `external:...`, and the `external` handler keyed on that scheme,
  keep working before and after migration.
  Retaining the D49 handler as a reader for un-migrated sidecars is cheap
  insurance.
- **D49-era attachments cannot join path-based dedup.** D03's scheme keys dedup
  on a hash of the original parent directory, which D49 discards.
  Migrated snapshots keep opaque ids and simply don't participate in [RFD 067]
  dedup.
  This is acceptable: the motivating ephemeral-file case never benefited from
  dedup.

Pre-D49 conversations (no `external:` attachments) are unaffected throughout.

## Implementation Plan

### Phase 1: Relax the parser and add the handler

Route out-of-workspace paths in `AttachmentUrlOrPath::parse` to the external
path instead of rejecting them.
Add `jp_attachment_external` with `add()` snapshotting content into the
conversation directory and `get()` reading it back.
Reuse the existing size guard and binary/text detection.
Reviewable and mergeable on its own.

### Phase 2: Listing and degradation UX

Display external attachments in `jp attachment ls` with an indicator
distinguishing them from workspace files, and surface the unavailable-on-this-
machine state.
Wire the warn-and-skip path for missing snapshots.

## References

- [RFD 031: Durable Conversation Storage with Workspace Projection][RFD 031]
- [RFD 052: Workspace Data Store Sanitization][RFD 052]
- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- [RFD 066: Content-Addressable Blob Store][RFD 066]
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067]
- RFD D03: External Attachment URI Scheme (draft) — the blob-backed evolution
  of this design.

[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 052]: ../052-workspace-data-store-sanitization.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
