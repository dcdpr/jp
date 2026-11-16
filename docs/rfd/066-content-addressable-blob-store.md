# RFD 066: Content-Addressable Blob Store

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17
- **Requires**: [RFD 065], [RFD 058]
- **Required by**: [RFD 067]
- **Summary**: Stores attachment and tool-response payloads in a gzip-compressed
  SHA-256 blob store and keeps compact references in `events.json`.

## Summary

This RFD introduces a content-addressable blob store that moves attachment and
tool-response payloads out of `events.json` into gzip-compressed files keyed by
SHA-256 checksum.
`events.json` retains only compact blob references and structural metadata, and
a background task garbage-collects unreferenced blobs.

## Motivation

[RFD 065] stores resource content inline in `events.json`, both on
`ChatRequest.resources` (attachments) and in `ToolCallResponse` content blocks
(tool results).
As conversations grow, inline payloads cause four problems.

### Readability and editability of `events.json`

JP designs `events.json` to be read and hand-edited directly.
Large base64-encoded payloads obscure that structure: a single attached source
file adds thousands of base64 characters, making the file hard to navigate and
unwieldy when copy-pasting excerpts into a new conversation.

### Disk space and duplication

Running `--attach ./src` on a modest project can serialize hundreds of files
into `events.json`.
Repeated snapshots of the same resource (from re-attachments, refreshes, or
multiple tool calls reading the same file) multiply the stored bytes.
When team members share conversations via Git, they transfer this bulk content
inline without deduplication across conversations that reference the same files.

### Context window pollution for future tools

When LLMs gain a tool to search conversation history (a planned feature), inline
payloads would pull raw file contents wholesale into the context window.
An LLM searching for a past design decision only needs metadata (URIs, MIME
types, timestamps) and should fetch file content separately when relevant.

### Parse time

Every `jp query` invocation deserializes `events.json`.
Fast-path operations like conversation listing and metadata display use
`serde_json::RawValue` to skip full deserialization, but operations that read
the full event stream (building the LLM request, forking, compaction) still
parse megabytes of inline content.

## Design

### Overview

All content payloads (attachment resources, tool response text, and tool
response resource blocks) live in a workspace-scoped, content-addressable blob
store at `.jp/blobs/`, which deduplicates content across conversations.
Blobs are gzip-compressed on disk and loaded lazily when building an LLM
request; `events.json` stores only compact references (checksum and byte size).

### Blob storage

Blobs are stored under a two-level directory prefix (2 + 2 hex characters)
derived from the SHA-256 checksum of the raw content bytes:

```
.jp/
  blobs/
    a1/
      b2/
        c3d4e5f6789...abc.blob.gz
    fe/
      dc/
        0123456789...xyz.blob.gz
```

The 4-character prefix provides 65,536 directory buckets (256 × 256), giving
headroom for long-lived workspaces with heavy tool use.
Each `.blob.gz` file holds the gzip-compressed raw bytes without base64 encoding
or JSON wrapping.
The filename checksum is also the integrity check: verifying a blob only
requires hashing its decompressed bytes and comparing the digest.

### Gzip compression

Gzip-compressing blobs on disk serves two purposes:

1. **Search opacity.** Compressed files do not match plain-text searches in
   `rg`, `grep`, or editor search-and-replace.
   Base64 encoding in `events.json` served the same purpose; gzip preserves that
   property when payloads live in standalone files.

2. **Size reduction.** Text payloads such as source code and tool output
   typically compress by 60-80% with gzip, which reduces repository growth when
   blobs are committed to Git for team sharing.

### Always external

Every content payload is written to the blob store regardless of size: a 16-byte
`"check succeeded"` tool response is stored as a blob just like a 500KB source
file.

Externalizing all payloads avoids size-threshold branching in serialization and
keeps inline content out of `events.json` entirely:

- **Copy-pasting conversation history.** Shared excerpts never include inline
  payloads that waste space or leak sensitive file contents.
- **Future search-over-history tools.** History searches never pull raw payload
  bytes into the LLM context window when only metadata is needed.

A gzip-compressed 16-byte string takes ~36 bytes on disk and one inode, so a
single unconditional code path outweighs the negligible storage overhead.

### Content representation in `events.json`

Content payloads in `events.json` are wrapped in a `content` object that
supports three variants:

**Blob reference** (JP's default for all writes):

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/src/main.rs",
    "mimeType": "text/x-rust",
    "content": {
      "$blob": "a1b2c3d4e5f6789...abc",
      "size": 12345
    }
  }
}
```

The `$blob` field contains the SHA-256 hex digest.
The `size` field records the uncompressed content size in bytes, enabling
display and token estimation without reading the blob.

**Inline text** (for user hand-edits):

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/src/main.rs",
    "mimeType": "text/x-rust",
    "content": {
      "text": "fn main() {}"
    }
  }
}
```

**Inline binary** (base64-encoded, for user hand-edits of binary content):

```json
{
  "type": "resource",
  "resource": {
    "uri": "file:///project/logo.png",
    "mimeType": "image/png",
    "content": {
      "blob": "<base64-encoded data>"
    }
  }
}
```

The same three variants apply to `ContentBlock::Text` tool responses:

```json
{
  "type": "text",
  "content": {
    "$blob": "fe98dc0123456789...xyz",
    "size": 847
  }
}
```

Or after a user hand-edit:

```json
{
  "type": "text",
  "content": {
    "text": "cargo check returned without errors"
  }
}
```

The `content` object always contains exactly one discriminant key: `$blob`,
`text`, or `blob`.
Deserialization maps these to `BlobContent::Ref`, `ResourceContent::Text`, and
`ResourceContent::Blob` respectively.

#### Why inline variants matter

Users hand-edit `events.json` to repair conversations; for example, turning a
failed tool call into a success requires editing both `is_error` and the
response payload.
Without inline variants, the user would have to gzip-compress the replacement
payload and name the file after its SHA-256 digest.
The `text` and `blob` variants let users write payloads directly in
`events.json`.

Inline variants are an input convenience, not an output format: JP deserializes
them normally and externalizes them to `$blob` references on the next write
(such as when the conversation advances).
For backward compatibility, the deserializer also accepts pre-blob-store events
without a `content` wrapper, reading bare `text` strings and base64 `blob`
fields as inline content.

### What stays in `events.json`

`events.json` remains the source of truth for conversation structure.
It retains all metadata (URIs, MIME types, checksums, tool call IDs, question
blocks, annotations, config deltas, and turn boundaries), omitting only raw
content payloads so the file stays small and fast to parse regardless of
referenced payload volume.

### Cross-conversation sharing

All conversations in a storage location share a single blob store.
Because blobs are content-addressed, two conversations that attach the same file
(identical bytes and SHA-256 digest) share one blob on disk without extra
deduplication logic.

JP maintains an independent blob store in each of its two storage locations:

- **Workspace storage** (`.jp/` in the project directory): conversations shared
  with the team via Git, with blobs at `.jp/blobs/`.
  Blobs must be committed alongside `events.json`; without them, `$blob`
  references dangle and teammates cannot read, continue, or fork the
  conversation.

- **User-local storage** (`~/.local/share/jp/workspace/<project>/` or the
  platform equivalent): private conversations not committed to Git, with blobs
  in that directory's `blobs/` subdirectory.

Deduplication happens within each store, not across them.
Sharing blobs between workspace and user-local storage is a non-goal because the
two locations have different visibility and ownership rules.

#### Viewing content

`jp conversation print` resolves `$blob` references transparently and renders
file contents and tool responses inline rather than printing checksums.

#### Selective VCS staging

Because workspace conversations share `.jp/blobs/`, staging a single
conversation requires knowing which blobs it references.
`jp conversation show --files <id>` prints every filesystem path a conversation
depends on (`events.json` and its referenced blob files) so users can pipe the
list to their VCS:

```sh
git add $(jp conversation show --files <id>)
hg add $(jp conversation show --files <id>)
```

JP stays VCS-agnostic: it outputs paths, and the user's toolchain consumes them.

### Lazy loading

Blob content is read from disk only when needed, primarily when building the LLM
request for the provider.
Metadata-only operations (listing conversations, displaying titles, forking,
inspecting event structure, or reading a resource's URI, MIME type, and
checksum) use `events.json` alone and never touch the blob store.

Deserialization produces a lazy wrapper instead of eagerly reading blobs:

```rust
/// Content that may be loaded lazily from the blob store.
pub enum BlobContent {
    /// Content loaded in memory.
    Loaded(Vec<u8>),
    /// Reference to content in the blob store, not yet loaded.
    Ref {
        checksum: String,
        size: u32,
    },
}
```

`ResourceContent` and `ContentBlock::Text` wrap `BlobContent` internally.
The SHA-256 checksum is computed once on write and exposed on `BlobContent::Ref`
for any consumer that needs content identity; callers that need the raw bytes
call a `resolve` method, which reads and decompresses the blob on first access.

### Writing blobs

When JP writes an event with content payloads (a `ChatRequest` with resources or
a `ToolCallResponse` with content blocks):

1. Compute the SHA-256 digest of each payload's raw bytes.
2. Check whether `blobs/<prefix>/<hash>.blob.gz` already exists in the
   conversation's blob store (workspace or user-local); if so, skip the blob
   write.
3. If the blob is missing, gzip-compress the raw bytes to a temporary file in
   the target leaf directory and atomically rename it to
   `blobs/<prefix>/<hash>.blob.gz`.
   Because POSIX rename is atomic, readers never observe a partially written
   blob.
4. Write `events.json` with `$blob` references.

If JP crashes after step 3 but before step 4, the unreferenced file is an
orphaned blob that the garbage collector cleans up on a later run.

### Lock-free concurrency

Because blobs are immutable and content-addressed, the store needs no locks:

- **Concurrent writes of the same checksum** produce identical bytes.
  One process wins the atomic rename and the other cleans up its temporary file,
  leaving a valid blob either way.
- **Reads during writes** either miss the path before the atomic rename or read
  the complete file after it, never partial bytes.
- **GC races** are the only hazard: a garbage-collection sweep can delete a
  newly written blob before `events.json` records its reference.
  To prevent data loss, the write path verifies that the blob still exists after
  writing `events.json` and re-creates it if the sweep removed it.
  At worst, a race causes one redundant write.

### Garbage collection

Orphaned blobs accumulate when conversations are deleted, forked (dropping
earlier references), or compacted (removing turns).
On every invocation, JP runs a background task on its existing task
infrastructure to sweep each storage location (workspace and user-local)
independently:

1. Scan every conversation's `events.json` in the storage location and collect
   all referenced checksums into a `HashSet`.
2. List all blob files in that location's `blobs/` directory.
3. Delete any blob whose checksum is not in the `HashSet`.

Because `events.json` holds only metadata, a full sweep is cheap: in a workspace
with 100 conversations and 1,000 blobs, reading 100 small JSON files and
scanning 1,000 filenames takes only milliseconds and finishes in the background
during an LLM query.
This avoids a manual `jp gc` command, reference-count state, or incremental
cursor tracking.

## Drawbacks

**Every content access requires a filesystem read.** Reading even a 16-byte
`"check succeeded"` response requires opening and decompressing a file.
Because blobs are resolved only when building LLM requests (not when listing
conversations or reading metadata), this adds only microseconds per blob.

**Inode overhead for small blobs.** Each unique payload consumes one inode and
one directory entry, so a conversation with 500 distinct tool responses creates
500 files.
Modern filesystems handle this easily, though it uses more directory entries
than a single `events.json`.

**Git repository size.** Sharing workspace conversations requires committing
`.jp/blobs/` to Git.
Gzip compression (60-80% smaller for text) and cross-conversation deduplication
reduce growth, but `.jp/blobs/` still grows over time in active workspaces.
Teams that hit repository size limits can use Git LFS, though Git LFS
integration is outside the scope of this RFD.

**Full GC sweep scales linearly.** Every invocation reads `events.json` for all
conversations in the store.
That cost is negligible for typical workspaces (tens to a few hundred
conversations), and if workspaces with thousands of conversations ever make the
sweep slow, a reference-count index can be added without changing the on-disk
blob format.

## Alternatives

### Size threshold for externalization

Externalize only payloads above a size threshold (such as 4KB) and keep smaller
payloads inline in `events.json`.

**Rejected because:** A threshold requires branching at every serialization and
deserialization site, and small inline payloads still pollute copy-pasted
excerpts and history searches.
The minor storage savings do not justify the extra code paths.

### Uncompressed blob storage

Store blobs as uncompressed raw bytes.

**Rejected because:** Uncompressed text blobs match workspace-wide searches in
`rg`, `grep`, and editor find-and-replace unless ignore rules are configured
everywhere.
Gzip keeps blobs opaque to text search tools at negligible CPU cost.

### SQLite blob store

Store blobs in a SQLite database (such as `.jp/blobs.db`) with a `(checksum,
content)` table.

**Rejected because:** The filesystem already provides deterministic O(1) lookup
by checksum, atomic writes via temporary-file rename, and Git-friendly per-blob
files.
SQLite adds a dependency and a monolithic binary file that is harder to inspect,
debug, and merge in Git, while the store only needs lookup by checksum.

### Refcount-based garbage collection

Maintain a reference-count index tracking which conversations reference each
blob, decrementing counts on deletion and removing blobs that reach zero.

**Rejected for now because:** Metadata-only `events.json` files are small enough
that a full sweep is fast without the crash-consistency complexity of keeping a
separate reference-count index synchronized with `events.json`.
A reference-count index can be added later if workspaces grow to thousands of
conversations.

### Sidecar file per conversation

Store an append-only sidecar file alongside each conversation mapping checksums
to payloads.

**Rejected because:** Lookups require an O(n) linear scan, identical payloads
are duplicated across conversations, and a text-framed sidecar still requires
escaping or base64-encoding binary data.

## Non-Goals

- **Blob encryption.** Blobs are gzip-compressed without encryption;
  workspace-level encryption is a separate concern.

- **Git LFS integration.** Integrating `.jp/blobs/` with Git LFS or other
  large-file storage is deferred.

- **Deduplication across stores or workspaces.** Content addressing deduplicates
  blobs within a single store; deduplicating across separate workspaces or
  between workspace and user-local stores is out of scope.

- **Streaming blob access.** Blobs are read into memory in full, which fits
  source files and tool output; streaming very large blobs (such as video or
  large datasets) is out of scope.

## Risks and Open Questions

### Blob file count on resource-constrained filesystems

Heavy tool use creates many small blob files, which on filesystems with tight
inode limits (such as some ext4 configurations) could exhaust inodes before disk
space.
Default inode counts suffice for typical development workspaces; if inode
pressure becomes a problem, a packed blob format (multiple blobs per file with
an index) can replace individual files.

### Concurrent writes from parallel conversations

Parallel conversations ([RFD 020]) may write blobs concurrently.
Content addressing and atomic POSIX renames from temporary files make concurrent
writes safe: identical checksums produce identical files, and the 65,536-bucket
directory fanout minimizes contention across distinct checksums.

### GC race with in-progress writes

A background GC sweep can delete a newly written blob before `events.json`
records its `$blob` reference.
To mitigate this, the write path verifies that the blob exists on disk and
re-creates it if missing; because blobs are immutable and content-addressed,
re-creating the file produces identical bytes.

### Migration from inline conversations

Because the deserializer accepts both inline payloads and `$blob` references,
existing conversations continue to work without an upfront migration.
Payloads can be extracted into the blob store either automatically on the next
conversation write or via a dedicated `jp migrate-blobs` command, while both
formats coexist on disk.

## Implementation Plan

### Phase 1: Blob store and write path

Create the `.jp/blobs/` directory layout, implement gzip-compressed SHA-256 blob
writes to `<prefix>/<hash>.blob.gz`, introduce the `BlobContent` lazy type, and
update `events.json` serialization to emit `$blob` references for all content
payloads.

This phase can merge independently; existing conversations with inline content
continue to load through the backward-compatible deserializer.

### Phase 2: Lazy deserialization

Update `events.json` deserialization to parse `$blob` fields into
`BlobContent::Ref` values, and implement the `resolve` method to read and
decompress blobs on first access.

Depends on Phase 1.

### Phase 3: Background garbage collection

Implement the background GC sweep (scan conversations, build the referenced
checksum set, delete orphaned blobs) and register it in JP's task infrastructure
to run on every invocation.

Depends on Phase 2.

### Phase 4: Migration tooling

Implement optional migration that extracts inline payloads from existing
`events.json` files into the blob store and rewrites them as `$blob` references,
either automatically on the next conversation write or via `jp conversation
migrate`.

Depends on Phase 2.

## References

- [RFD 065: Typed Resource Model for Attachments][RFD 065]: defines the
  `Resource` type whose payloads this RFD externalizes, and identifies inline
  payload bloat as the motivating problem.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058]: defines the
  `ContentBlock` and `Resource` types whose payloads this RFD externalizes.
- [RFD 036: Conversation Compaction][RFD 036]: drops old turns during
  compaction, which can orphan blobs that garbage collection cleans up.
- [RFD 020: Parallel Conversations][RFD 020]: introduces concurrent conversation
  writes, which remain safe in a content-addressed blob store.

[RFD 020]: 020-parallel-conversations.md
[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
