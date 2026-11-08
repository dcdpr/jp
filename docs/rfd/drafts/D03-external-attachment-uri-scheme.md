# RFD D03: External Attachment URI Scheme

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-01
- **Requires**: [RFD 065], [RFD 066]
- **Summary**: Introduces an `external:` URI scheme to attach and snapshot files
  from outside the workspace without leaking host paths.

## Summary

This RFD introduces the `external:` URI scheme for attaching files from outside
the workspace directory (e.g. `jp q --attach ~/Downloads/report.pdf`).
External attachments snapshot file content at attachment time, omit absolute
host paths from conversation state, and integrate with [RFD 066]'s blob store
and [RFD 067]'s deduplication.

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

Keeping the workspace self-contained and shareable is intentional, but rejecting
external paths blocks a common workflow: attaching a downloaded document, a
design spec from another project, or a data file from a shared drive.
Users must copy the file into the workspace first, which is inconvenient and
pollutes the project directory with unrelated files.

### Why not allow absolute paths?

Conversations are shared across team members via Git.
An absolute path like `/Users/jean/Downloads/report.pdf` is meaningless on a
teammate's machine and leaks the author's filesystem structure (username and
directory layout) into shared conversation state, creating both privacy and
portability problems.

Although [RFD 065]'s snapshot model captures file content at attachment time so
teammates can still read the conversation, the canonical URI remains visible:
the LLM reads it, `jp attachment ls` displays it, and [RFD 067] uses it for
deduplication.

### Requirements

An external attachment URI scheme must:

1. Preserve content via snapshot without re-resolving from the original path.
2. Hide the user's filesystem structure.
3. Produce the same URI when attaching the same file from the same location
   twice, enabling deduplication.
4. Produce distinct URIs for files with the same name in different directories,
   avoiding false deduplication.
5. Integrate with [RFD 066]'s blob store and garbage collection.
6. Support [RFD 067]'s `(canonical_uri, checksum)` deduplication matching.

## Design

### The `external:` URI scheme

External attachments use the `external:` scheme with a hashed directory
component and the original filename:

```
external:<sha256-of-canonical-parent-directory>/<filename>
```

| Original path                            | Canonical parent        | URI                             |
| ---------------------------------------- | ----------------------- | ------------------------------- |
| `~/Downloads/report.pdf`                 | `/Users/jean/Downloads` | `external:a1b2c3.../report.pdf` |
| `~/Desktop/report.pdf`                   | `/Users/jean/Desktop`   | `external:f4e5d6.../report.pdf` |
| `~/Downloads/report.pdf` (same as first) | `/Users/jean/Downloads` | `external:a1b2c3.../report.pdf` |

The directory component is the SHA-256 digest of the canonicalized parent
directory path (with `~` expanded, symlinks resolved, and `..` collapsed),
matching the hash algorithm used by [RFD 066]'s content-addressable blob store.

### URI construction

When the file attachment handler receives a path outside the workspace root:

1. Canonicalize the full path (`std::fs::canonicalize`).
2. Split the path into parent directory and filename.
3. Compute `sha256(parent_directory_as_utf8_bytes)`.
4. Construct `external:<hex-digest>/<filename>`.

Preserving the unhashed filename gives the LLM useful context about the
resource: it sees `external:a1b2c3.../report.pdf` rather than an opaque
identifier.

### Deduplication

[RFD 067] deduplicates resources by `(canonical_uri, checksum)`.
Because `external:` URIs are deterministic for a given `(directory, filename)`
pair, deduplication behaves as follows:

| Scenario                                     | Same URI?                  | Same checksum? | Dedup? |
| -------------------------------------------- | -------------------------- | -------------- | ------ |
| Same file, attached twice, content unchanged | yes                        | yes            | yes    |
| Same file, attached twice, content changed   | yes                        | no             | no     |
| Same filename, different directories         | no                         | maybe          | no     |
| Same file, different machines                | no (different parent hash) | maybe          | no     |

In the last row, two users attaching the same file from their own machines
perform independent attachment acts, and each attachment is snapshotted
separately.

### Privacy

Conversation state stores no absolute paths, and the one-way SHA-256 hash of the
parent directory cannot be reversed to recover the original path.
Preserving the filename reveals that a file of that name (e.g. `report.pdf`) was
attached, which is required for LLM usability and exposes no more information
than the `source` field on workspace-internal attachments.

### Snapshot-only semantics

The `external:` scheme marks a resource as a static snapshot in the blob store
with no source to re-resolve or refresh from.
Calling [RFD 065]'s `refresh_resource` tool on an `external:` URI returns an
error because the original file may have moved, been deleted, or never existed
on the current machine.

### Blob store and garbage collection

External attachment content is stored in the [RFD 066] blob store identically to
workspace attachment content, and the `ChatRequest.resources` entry in
`events.json` references each blob by its SHA-256 checksum.

Garbage collection works without changes.
Because GC scans `events.json` for `$blob` checksum references without
inspecting URI schemes, deleting a conversation removes its blob reference and
allows GC to clean up the blob.

### Attachment handler changes

The `file` attachment handler (`jp_attachment_file_content`) relaxes the
workspace-root validation in `add()` (which previously rejected external paths
via `strip_prefix`) to accept absolute and `~/` paths.
In `get()`, the handler reads and snapshots the file at attachment time and
constructs the canonical URI stored in the resource metadata based on the
resolved path's location:

1. Inside the workspace root: produce a `file:///` URI.
2. Outside the workspace root: produce an `external:` URI.

### CLI usage

The existing `--attach` flag accepts external paths without new flags:

```sh
jp q --attach ~/Downloads/report.pdf "Summarize this document"
jp q --attach /tmp/data.csv "Parse this data"
```

Relative paths resolve against the current working directory, using the
`file:///` scheme when the resolved path falls inside the workspace and the
`external:` scheme when it falls outside.

## Drawbacks

**External attachments are not refreshable.** Because snapshots are frozen at
attachment time as deterministic conversation records, the user must re-attach a
file if its source changes.

**Filename collisions in display.** Because the hashed directory prefix is not
human-readable, two external attachments with the same filename from different
directories (e.g. two `config.yaml` files) appear identical in `jp attachment
ls` unless the user inspects the full URI.
The LLM can still distinguish them by content and by their position in the
conversation.

## Alternatives

### Store the original absolute path

Use the raw filesystem path as the canonical URI (e.g.
`file:///Users/jean/Downloads/report.pdf`).

Rejected because raw host paths leak potentially sensitive filesystem structure
into shared conversation state and are meaningless on teammates' machines.

### UUID-based URIs

Use `external:<uuid>/<filename>` with a random UUID per attachment act.

Rejected because generating a new URI on each attachment prevents [RFD 067] from
deduplicating repeated attachments of the same file with identical content.
Hashing the parent directory preserves deduplication for repeated attachments of
a file from the same location.

### Content-hash URIs

Use `blob:<sha256-of-content>/<filename>` with the content hash as the
identifier.

Rejected because two distinct files with identical content and the same filename
receive the same URI, causing [RFD 067] to deduplicate them into a single
resource so the LLM loses track of which file is which.

## Non-Goals

**Tool access to external files.** This RFD covers only user-initiated
attachments via `--attach`.
Tools remain restricted to the workspace root (or the `ProjectFiles` VFS in a
future design), as expanding tool access to arbitrary filesystem paths has
separate security implications.

**Cross-machine re-resolution.** External attachments are snapshots; teammates
cannot refresh an external attachment from their own copy of the file.
A "shared external resource" mechanism is left to a future RFD if needed.

## Risks and Open Questions

**Symlinks and mount points.** Because `std::fs::canonicalize` resolves
symlinks, `~/Downloads/report.pdf` and `~/Dropbox/Downloads/report.pdf` (if
`Downloads` is a symlink) produce different canonical parents and therefore
different URIs.
This is a low-risk edge case, though it may surprise users who expect
deduplication across symlinks.

**Filename encoding.** The URI scheme cannot represent non-UTF-8 filenames, so
the attachment handler rejects them.
This matches JP's existing UTF-8 requirement (`camino::Utf8Path`).

## Implementation Plan

### Phase 1: Allow external paths in the file handler

Relax the workspace-root restriction in `jp_attachment_file_content` and
construct `external:` URIs for out-of-workspace files.
Reading and snapshotting the file at attachment time work unchanged regardless
of location.

This phase depends on [RFD 065] (typed resource model, which stores the
`external:` URI on `Resource.uri`) and [RFD 066] (blob store, which persists
external attachment content rather than storing it inline).

### Phase 2: Display and listing

Update `jp attachment ls` to display external attachments with a visual
indicator (e.g. a different prefix or icon) so users can distinguish workspace
and external resources at a glance.

## References

- [RFD 065: Typed Resource Model for Attachments][RFD 065]
- [RFD 066: Content-Addressable Blob Store][RFD 066]
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067]

[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
