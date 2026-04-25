# RFD D03: External Attachment URI Scheme

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01

## Summary

This RFD introduces support for attaching files from outside the workspace
directory (e.g. `jp q --attach ~/Downloads/report.pdf`) and defines the
`external:` URI scheme for identifying these resources. External attachments
are content-snapshotted on attach, privacy-safe (no absolute paths stored in
conversation state), and compatible with [RFD 066]'s blob store and [RFD 067]'s
deduplication.

## Motivation

JP's file attachment handler resolves paths relative to the workspace root and
rejects files outside it:

```rust
// jp_attachment_file_content/src/lib.rs
let Ok(rel) = path.strip_prefix(cwd) else {
    warn!(path = %path, "Attachment path outside of working directory, skipping.");
    return None;
};
```

This is intentional — the workspace is self-contained and shareable. But it
prevents a common workflow: attaching a document downloaded from the web, a
design spec from another project, or a data file from a shared drive.

Today, the workaround is to copy the file into the workspace first. This is
inconvenient and pollutes the project directory with unrelated files.

### Why not just allow absolute paths?

Conversations are shareable across team members via Git. If I attach
`/Users/jean/Downloads/report.pdf`, that path is meaningless on my teammate's
machine. Worse, it leaks my filesystem structure (username, directory layout)
into shared conversation state.

With [RFD 065]'s snapshot model, the content is captured at attachment time, so
teammates can read the conversation — but the canonical URI still matters. It is
what the LLM sees, what appears in `jp attachment ls`, and what [RFD 067] uses
for deduplication matching. Storing raw absolute paths as canonical URIs creates
both a privacy problem and a portability problem.

### What we need

A URI scheme for external attachments that:

1. Preserves content via snapshot (no re-resolution from the original path).
2. Does not leak the user's filesystem structure.
3. Supports deduplication: attaching the same file twice from the same location
   produces the same URI.
4. Avoids false deduplication: attaching files with the same name from different
   directories produces different URIs.
5. Works with [RFD 066]'s blob store and garbage collection.
6. Works with [RFD 067]'s `(canonical_uri, checksum)` dedup matching.

## Design

### The `external:` URI scheme

External attachments use the `external:` scheme with a hashed directory
component and the original filename:

```
external:<sha256-of-canonical-parent-directory>/<filename>
```

Examples:

| Original path | Canonical parent | URI |
|--------------|-----------------|-----|
| `~/Downloads/report.pdf` | `/Users/jean/Downloads` | `external:a1b2c3.../report.pdf` |
| `~/Desktop/report.pdf` | `/Users/jean/Desktop` | `external:f4e5d6.../report.pdf` |
| `~/Downloads/report.pdf` (same as first) | `/Users/jean/Downloads` | `external:a1b2c3.../report.pdf` |

The hash is SHA-256 of the canonicalized parent directory path (symlinks
resolved, `..` collapsed, `~` expanded). SHA-256 is chosen for consistency with
[RFD 066]'s blob store, which uses the same algorithm for content addressing.

### URI construction

When the file attachment handler receives a path outside the workspace root:

1. Canonicalize the full path (`std::fs::canonicalize`).
2. Split into parent directory and filename.
3. Compute `sha256(parent_directory_as_utf8_bytes)`.
4. Construct `external:<hex-digest>/<filename>`.

The filename is preserved verbatim (not hashed) for LLM readability. The LLM
sees `external:a1b2c3.../report.pdf` rather than a fully opaque identifier —
the filename provides useful context for reasoning about the resource.

### Deduplication behavior

[RFD 067] deduplicates by `(canonical_uri, checksum)`. The `external:` scheme
produces deterministic URIs for the same `(directory, filename)` pair, which
gives the correct dedup behavior:

| Scenario | Same URI? | Same checksum? | Dedup? |
|----------|-----------|---------------|--------|
| Same file, attached twice, content unchanged | yes | yes | yes |
| Same file, attached twice, content changed | yes | no | no |
| Same filename, different directories | no | maybe | no |
| Same file, different machines | no (different parent hash) | maybe | no |

The last row is correct: two people attaching the same file from their own
machines are independent attachment acts. The content is snapshotted
independently for each.

### Privacy

No absolute path is stored in conversation state. The parent directory hash is
opaque — it cannot be reversed to recover the original path. The filename is
preserved, which could be considered a minor leak (it reveals that a file named
`report.pdf` was attached), but this is unavoidable for LLM usability and no
worse than the current `source` field on workspace-internal attachments.

### Snapshot-only semantics

External attachments are not re-resolvable. The `external:` scheme signals to
JP that the content exists only as a snapshot in the blob store. There is no
source to refresh from.

If [RFD 065]'s `refresh_resource` tool is called on an `external:` URI, it
returns an error explaining that external resources cannot be refreshed — the
original file may have moved, been deleted, or may not exist on the current
machine.

### Blob store and garbage collection

External attachment content is stored in the blob store ([RFD 066]) identically
to workspace attachment content. The blob is referenced by SHA-256 checksum from
the `ChatRequest.resources` entry in `events.json`.

Garbage collection works unchanged. GC scans `events.json` for referenced
checksums and deletes unreferenced blobs. The URI scheme is metadata in
`events.json` — GC never inspects URIs, only `$blob` checksum references. When
a conversation containing an external attachment is deleted, the blob reference
disappears and GC cleans up the blob.

### Attachment handler changes

The `file` attachment handler (`jp_attachment_file_content`) currently rejects
paths outside the workspace. This RFD changes the behavior:

1. If the resolved path is inside the workspace root: produce a `file:///`
   URI as today. No change.
2. If the resolved path is outside the workspace root: produce an `external:`
   URI using the scheme described above.

In both cases, the content is read and snapshotted at attachment time. The
difference is only in the canonical URI stored in the resource metadata.

The handler's `add()` method currently validates against the workspace root
implicitly (via `strip_prefix`). This validation is relaxed to allow absolute
paths and `~/` paths, while the `get()` method handles URI construction.

### CLI interface

No new flags are needed. The existing `--attach` flag accepts the path:

```sh
jp q --attach ~/Downloads/report.pdf "Summarize this document"
jp q --attach /tmp/data.csv "Parse this data"
```

Relative paths are resolved against the current working directory (as today).
If the resolved path falls outside the workspace, the `external:` scheme is
used. If inside, the `file:///` scheme is used.

## Drawbacks

**External attachments are not refreshable.** Once attached, the content is
frozen. If the source file changes, the user must re-attach it. This is
inherent to the snapshot model and consistent with the design principle that
conversations are deterministic records.

**Filename collisions in display.** Two external attachments from different
directories with the same filename (e.g. two different `config.yaml` files)
appear identical in `jp attachment ls` unless the user inspects the full URI.
The hashed directory prefix is not human-readable. In practice, the LLM can
distinguish them by content and by their position in the conversation.

## Alternatives

### Store the original absolute path

Use the raw filesystem path as the canonical URI (e.g.
`file:///Users/jean/Downloads/report.pdf`).

Rejected because it leaks the user's filesystem structure into shared
conversation state. Teammate machines have different paths. The URI becomes
meaningless and potentially sensitive.

### UUID-based URIs

Use `external:<uuid>/<filename>` with a random UUID per attachment act.

Rejected because it breaks same-file deduplication. Attaching the same file
twice produces two different URIs, so [RFD 067] cannot deduplicate them even
when the content is identical. The directory-hash approach preserves dedup for
repeated attachments of the same file from the same location.

### Content-hash URIs

Use `blob:<sha256-of-content>/<filename>` with the content hash as the
identifier.

Rejected because it causes false deduplication. Two different files with
identical content and the same filename would get the same URI, and [RFD 067]
would treat them as the same resource. The LLM would lose track of which file
is which.

## Non-Goals

**Tool access to external files.** This RFD only covers user-initiated
attachments via `--attach`. Tools remain restricted to the workspace root (or
the `ProjectFiles` VFS in a future design). Expanding tool access to arbitrary
filesystem paths is a separate concern with different security implications.

**Cross-machine re-resolution.** External attachments are snapshots. There is
no mechanism for a teammate to refresh an external attachment from their own
copy of the file. If this becomes a need, a future RFD could introduce a
"shared external resource" concept, but that is out of scope here.

## Risks and Open Questions

**Symlinks and mount points.** `std::fs::canonicalize` resolves symlinks, which
means `~/Downloads/report.pdf` and `~/Dropbox/Downloads/report.pdf` (if
`Downloads` is a symlink) produce different canonical parents and thus different
URIs. This is technically correct (they are different paths) but may surprise
users who expect dedup to work across symlinks. The risk is low — this is an
edge case.

**Filename encoding.** Non-UTF-8 filenames cannot be represented in the URI
scheme. This is consistent with JP's existing UTF-8 requirement (the project
uses `camino::Utf8Path` throughout). Non-UTF-8 filenames are rejected at the
attachment handler level.

## Implementation Plan

### Phase 1: Allow external paths in the file handler

Relax the workspace-root restriction in `jp_attachment_file_content`. Construct
`external:` URIs for out-of-workspace files. Content reading and snapshotting
work unchanged — the file is read at attachment time regardless of location.

Depends on [RFD 065] (typed resource model) being implemented, since the
`external:` URI is stored on the `Resource.uri` field.

Depends on [RFD 066] (blob store) being implemented, since external attachment
content must be persisted in the blob store rather than inline.

### Phase 2: Display and listing

Update `jp attachment ls` to display external attachments with a visual
indicator (e.g. a different prefix or icon) so users can distinguish workspace
and external resources at a glance.

## References

- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- [RFD 066: Content-Addressable Blob Store][RFD 066]
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067]

[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
