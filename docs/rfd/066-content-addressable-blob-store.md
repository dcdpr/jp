# RFD 066: Content-Addressable Blob Store

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-17

## Summary

This RFD introduces a content-addressable blob store that externalizes all
content payloads from `events.json`. Resource content (attachments and tool
responses) is stored as gzip-compressed blobs keyed by SHA-256 checksum.
`events.json` carries compact blob references instead of inline content, keeping
it a small structural skeleton of conversation metadata. A background task
garbage-collects unreferenced blobs on every JP invocation.

## Motivation

[RFD 065] places resource content inline in `events.json` — both on
`ChatRequest.resources` (attachments) and in `ToolCallResponse` content blocks
(tool results). As conversations grow, this creates several problems.

### Readability and editability of `events.json`

A core goal of JP is that users can read and hand-edit `events.json` files. With
large base64-encoded content payloads inline, the file becomes difficult to
navigate. A single attached source file can add thousands of characters of
base64 to what is otherwise a readable JSON structure. Users regularly
copy-paste parts of a conversation into a new conversation, and inline blobs
make that unwieldy.

### Disk space and duplication

A `--attach ./src` on a modest project can resolve to hundreds of files. Each
file's content is serialized into `events.json`. Multiple snapshots of the same
resource — from re-attachments, refreshes, or the same file read by multiple
tool calls — multiply the stored content. Team members sharing conversations via
Git transfer this bulk content inline, with no deduplication across
conversations that reference the same files.

### Context window pollution for future tools

If JP gains the ability for LLMs to search conversation history (a planned
feature), inline content payloads would be pulled wholesale into the context
window. An LLM searching for a conversation about a design decision does not
need the raw bytes of every file that was attached — it needs metadata (URIs,
MIME types, timestamps) and should fetch content separately only when relevant.

### Parse time

Every `jp query` invocation deserializes `events.json`. JP already uses
`serde_json::RawValue` for fast-path operations (conversation listing, metadata
display), so not all operations pay the full deserialization cost. But
operations that do need the full event stream — building the LLM request,
forking, compaction — are slower with megabytes of inline content.

## Design

### Overview

All content payloads — resource content from attachments, tool response text,
and tool response resource blocks — are stored in a content-addressable blob
store at `.jp/blobs/`. `events.json` carries compact references (checksum +
size) instead of inline content. The blob store is workspace-scoped, enabling
cross-conversation deduplication. Blobs are gzip-compressed on disk. Content is
loaded lazily, only when building the LLM request.

### Blob storage

Blobs are stored with a two-level directory prefix (2 + 2 characters) derived
from the SHA-256 checksum:

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

The two-level prefix provides 65,536 directory buckets (256 × 256), giving
headroom for long-lived workspaces with heavy tool use. Each leaf directory
contains blob files whose checksums share the same 4-character prefix.

Each blob file contains the gzip-compressed raw content bytes. No base64, no
JSON wrapping. The checksum in the filename is the integrity check — to verify,
re-hash the decompressed content and compare.

### Gzip compression

All blobs are gzip-compressed on disk. This serves two purposes:

1. **Search opacity.** Compressed files do not match text searches in `rg`,
   `grep`, or editor search-and-replace. Content payloads are currently
   base64-encoded in `events.json` for this same reason; gzip compression
   preserves the property when content moves to separate files.

2. **Size reduction.** Text content (source code, tool output) typically
   compresses 60–80% with gzip. This is especially valuable since blobs are
   committed to Git for team sharing (see below).

### Always external

All content payloads are externalized to the blob store, with no size threshold.
A tool response of "check succeeded" (16 bytes) is stored as a blob, same as a
500KB source file.

This avoids conditional logic ("is this content big enough to externalize?") and
prevents inline content from appearing in contexts where it causes problems:

- **Copy-pasting conversation history.** Users sharing conversation excerpts
  with colleagues or other LLMs would include inline content, wasting space and
  leaking potentially sensitive file contents.
- **Future search-over-history tools.** An LLM tool that searches conversation
  history would pull inline content into its context window, wasting tokens on
  raw file contents when it only needs metadata.

The filesystem cost of small blobs (a gzip-compressed 16-byte string is ~36
bytes on disk, plus one inode) is negligible. The implementation simplicity of
one code path outweighs the marginal storage overhead.

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

The `$blob` field contains the SHA-256 hex digest. The `size` field records
the uncompressed content size in bytes, enabling display and token estimation
without reading the blob.

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
`text`, or `blob`. Deserialization maps these to `BlobContent::Ref`,
`ResourceContent::Text`, and `ResourceContent::Blob` respectively.

#### Why inline variants matter

Users hand-edit `events.json` to fix conversations — for example, changing a
failed tool call to a success requires editing both `is_error` and the response
content. With only blob references, the user would need to create a
gzip-compressed file with the correct checksum filename. The inline `text` and
`blob` variants let users write content directly in `events.json`.

JP always writes `$blob` references. When JP reads an `events.json` that a
user has edited with inline content, it deserializes the inline data normally.
On the next write (e.g., when the conversation advances), JP re-externalizes
all content to the blob store. The inline forms are a user convenience for
reads and hand-edits, not a format JP produces.

Existing conversations without the `content` wrapper (pre-blob-store format)
are handled by backward-compatible deserialization — bare `text` strings and
base64 `blob` fields are read as inline content.

### What stays in `events.json`

`events.json` remains the source of truth for conversation structure. It
contains all metadata — URIs, MIME types, checksums, tool call IDs, question
blocks, annotations, config deltas, turn boundaries — everything except raw
content payloads. The file stays small and fast to parse regardless of how much
content the conversation references.

### Content addressing

The blob store uses SHA-256 as its content-address key. When JP writes a
resource to the store:

1. Compute SHA-256 of the raw content bytes.
2. Check the blob store — if a blob with that hash already exists, skip the
   write.
3. If new, gzip-compress and write to `blobs/<prefix>/<hash>.blob.gz`.
4. Write the blob reference to `events.json`.

The checksum is computed once on write and exposed via `BlobContent::Ref` for
any consumer that needs content identity.

### Cross-conversation sharing

The blob store is shared across all conversations in a given storage location.
If two conversations attach the same file (same content, same SHA-256), only
one blob exists on disk. This is a natural consequence of content-addressing
— no additional dedup logic is needed at the storage layer.

JP has two storage locations:

- **Workspace storage** (`.jp/` in the project directory): conversations
  shared with the team via Git. The blob store lives at `.jp/blobs/`. Blobs
  are committed to Git alongside `events.json` — team members need blob
  content to read, continue, or fork conversations. Without the blobs,
  `events.json` contains dangling references and conversations are
  unreadable.

- **User-local storage** (`~/.local/share/jp/workspace/<project>/` or
  platform equivalent): conversations private to the user, not committed to
  Git. The blob store lives at the corresponding `blobs/` directory within
  this location.

Each storage location has its own independent blob store. Cross-conversation
dedup works within each store (workspace blobs dedup with other workspace
conversations, user-local blobs dedup with other user-local conversations).
Dedup across the two stores is a non-goal — the storage locations serve
different purposes and sharing blobs between them would complicate the
ownership model.

#### Viewing content

`jp conversation print` resolves content references transparently and renders
the full conversation with actual content inline. The user sees file contents
and tool responses, not checksums. The blob store is invisible during normal
conversation viewing.

#### Selective VCS staging

The shared content store means that staging a single conversation for version
control requires knowing which files it depends on. `jp conversation show
--files <id>` lists all filesystem paths for a conversation (events.json plus
referenced content files). Users pipe this to their VCS of choice:

```sh
git add $(jp conversation show --files <id>)
hg add $(jp conversation show --files <id>)
```

JP remains VCS-agnostic: it outputs paths, the user's toolchain consumes them.

### Lazy loading

Blob content is loaded only when needed — primarily when building the LLM
request that sends content to the provider. Operations that only need
conversation metadata (listing conversations, displaying titles, forking,
reading event structure) work with `events.json` alone and never touch the blob
store.

The deserialization layer produces a lazy wrapper type instead of eagerly
loading content:

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

`ResourceContent` and `ContentBlock::Text` use `BlobContent` internally. Code
that needs the actual bytes calls a `resolve` method that reads from the blob
store on first access. Code that only needs metadata (URI, MIME type, checksum)
never triggers a load.

### Writing blobs

When JP writes a new event containing content (a `ChatRequest` with resources,
or a `ToolCallResponse` with content blocks):

1. For each content payload, compute SHA-256.
2. Check if the blob exists in the appropriate store (workspace or user-local,
   matching the conversation's storage location).
3. If not, gzip-compress the raw bytes and write to a temporary file in the same
   directory, then atomically rename to the final path. The rename is atomic on
   POSIX, so readers never see a partially-written blob.
4. Write `events.json` with `$blob` references.

If JP crashes after writing the blob but before writing `events.json`, the
blob is orphaned — an unreferenced file in the store. The garbage collector
handles this (see below).

### No locking required

Blobs are immutable and content-addressed. This eliminates most concurrency
concerns:

- **Concurrent writes of the same checksum** produce identical bytes. The atomic
  rename means one process wins; the other's temp file is cleaned up. The result
  is correct either way.
- **Reads during writes** are safe because the final path either doesn't exist
  yet (blob not visible) or is complete (rename is atomic). Readers never see
  partial content.
- **GC races** are the one real hazard: the GC sweep can delete a blob between
  the time a write creates it and the time `events.json` records the reference.
  The write path mitigates this by verifying blob existence after writing
  `events.json` and re-creating the blob if it was deleted. Since blobs are
  immutable and content-addressed, re-creation produces the identical file. The
  worst case is a redundant write, not data loss.

### Garbage collection

Unreferenced blobs accumulate when conversations are deleted, forked (old
references dropped), or compacted (turns removed). A background task runs on
every JP invocation to clean up orphans. The sweep runs independently for each
storage location (workspace and user-local):

1. Scan all conversations' `events.json` files in this storage location and
   collect every referenced checksum into a `HashSet`.
2. List all blob files in this location's `blobs/` directory.
3. Delete any blob whose checksum is not in the referenced set.

This is a full sweep, not an incremental scan. It is cheap because `events.json`
files are small metadata-only skeletons (all content is in the blob store). For
a workspace with 100 conversations and 1000 blobs, the sweep reads 100 small
files and scans 1000 filenames — milliseconds of work.

The sweep runs as a background task using JP's existing task infrastructure. LLM
queries take seconds to minutes; the GC sweep completes unnoticed in the
background. No manual `jp gc` command, no refcount state, no cursor tracking.
One task, one `HashSet`, one directory walk.

## Drawbacks

**Every content access requires a filesystem read.** Even a 16-byte "check
succeeded" message requires opening a file, decompressing, and reading. In
practice this is microseconds per blob, and content is only loaded when building
LLM requests — not during conversation listing or metadata operations. The cost
is measurable but not meaningful.

**Inode overhead for small blobs.** Each blob consumes one inode and one
directory entry. A conversation with 500 tool calls produces 500 blob files.
Modern filesystems handle this without issue, but it is more filesystem pressure
than a single `events.json`.

**Git repository size.** Blobs must be committed for team sharing. A workspace
with extensive conversation history accumulates blob files in Git. Gzip
compression helps (text blobs compress 60–80%), and cross-conversation dedup
avoids storing the same content twice. But long-lived workspaces with heavy tool
use will grow their `.jp/blobs/` directory over time. Git LFS is an option for
teams where this becomes a problem, but is not designed in this RFD.

**Full GC sweep scales linearly.** The sweep reads all conversations'
`events.json` on every invocation. With thousands of conversations this could
take noticeable time. For typical workspaces (tens to low hundreds of
conversations), the cost is negligible. If scaling becomes a problem, a refcount
index can be layered on without changing the blob store format.

## Alternatives

### Size threshold for externalization

Externalize only content above a threshold (e.g., 4KB), keeping small payloads
inline in `events.json`.

**Rejected because:** Conditional logic adds two code paths for every
serialization and deserialization site. Small inline content still appears in
copy-pasted history and search-over-history tools. The marginal storage savings
for small blobs do not justify the implementation complexity.

### Uncompressed blob storage

Store blobs as raw bytes without gzip compression.

**Rejected because:** Uncompressed text blobs appear in workspace-wide text
searches (`rg`, `grep`, editor find-and-replace). Gzip makes blobs opaque to
text tools regardless of Git or editor ignore configuration. The compression
CPU cost is negligible for the blob sizes involved.

### SQLite blob store

Store blobs in a SQLite database (e.g., `.jp/blobs.db`) with a
`(checksum, content)` table.

**Rejected because:** The filesystem already provides O(1) lookup by checksum
(the path is deterministic), crash-safe writes (write to temp file, rename),
and Git-compatible storage. SQLite adds a dependency and a binary format that
is harder to inspect, debug, and share via Git. A database would be justified
if we needed richer queries over blob metadata, but we do not — the only
operation is "read blob by checksum."

### Refcount-based garbage collection

Maintain a separate refcount index tracking which conversations reference each
blob. Decrement on conversation delete, delete blobs with zero references.

**Rejected for now because:** With `events.json` being small metadata-only
files, a full sweep is cheap enough that the refcount's added complexity
(crash consistency between refcount file and events.json, handling of orphaned
refcount entries) is not justified. A refcount index can be added later as an
optimization if the full sweep becomes expensive with thousands of
conversations.

### Sidecar file per conversation

Store a single append-only sidecar file per conversation mapping checksums to
content.

**Rejected because:** Linear scan for lookups (O(n)), no cross-conversation
deduplication, base64 encoding still needed for binary content in a text-based
format, and line-based framing requires escaping for binary content.

## Non-Goals

- **Blob encryption.** Blobs are stored as gzip-compressed content without
  encryption. Workspace-level encryption is a separate concern.

- **Git LFS integration.** Large blob files in Git are a potential concern for
  long-lived workspaces. Integration with Git LFS or similar large-file
  storage is deferred.

- **Blob deduplication across workspaces.** Cross-conversation dedup within a
  storage location is handled by content-addressing. Dedup across separate
  workspaces or between workspace and user-local storage is out of scope.

- **Streaming blob access.** Blobs are read fully into memory on access. For
  the content sizes involved (source files, tool output), this is appropriate.
  Streaming access for very large blobs (video, large datasets) is not
  designed here.

## Risks and Open Questions

### Blob file count on resource-constrained filesystems

A workspace with heavy tool use accumulates many small blob files. On
filesystems with limited inodes (e.g., some default ext4 configurations), this
could theoretically exhaust inodes before exhausting disk space. In practice,
default inode counts are generous enough that this is unlikely for typical
development workspaces. If it becomes a problem, a packed blob format
(multiple blobs in one file with an index) could replace individual files.

### Concurrent writes from parallel conversations

[RFD 020] introduces parallel conversations that may write blobs
simultaneously. Content-addressing makes this safe — two processes writing the
same checksum produce the same file. For different checksums, the directory
fanout makes contention unlikely. The write pattern (write to temp file,
rename to final path) is atomic on POSIX filesystems.

### GC race with in-progress writes

The GC sweep could delete a blob between the time a write computes its
checksum and the time it writes the events.json reference. Mitigation: the
write path checks blob existence and re-creates it if missing before writing
events.json. Since blobs are immutable and content-addressed, re-creating a
deleted blob produces the identical file.

### Migration from inline conversations

Existing conversations store content inline in `events.json`. The
deserialization layer handles both formats (inline and blob-ref). On first
access after migration, existing conversations continue to work with inline
content. A migration tool (`jp migrate-blobs` or automatic on next write)
could extract inline content to the blob store and rewrite `events.json` with
blob references, but is not required — both formats coexist.

## Implementation Plan

### Phase 1: Blob store and write path

Create the `.jp/blobs/` directory structure. Implement blob write (SHA-256,
gzip, write to `<prefix>/<hash>.blob.gz`). Implement the `BlobContent` lazy
type. Update `events.json` serialization to write `$blob` references for all
content payloads.

Can be merged independently. Existing conversations with inline content
continue to work via the backward-compatible deserializer.

### Phase 2: Lazy deserialization

Update `events.json` deserialization to detect `$blob` fields and produce
`BlobContent::Ref` values. Implement the `resolve` method that reads and
decompresses from the blob store on first access.

Depends on Phase 1.

### Phase 3: Background garbage collection

Implement the GC sweep as a background task: scan conversations, build
referenced set, delete orphans. Register the task in JP's task infrastructure
so it runs on every invocation.

Depends on Phase 2.

### Phase 4: Migration tooling

Implement optional migration for existing conversations: read `events.json`
with inline content, extract payloads to blob store, rewrite with blob
references. This can be automatic (on next conversation write) or manual
(`jp conversation migrate`).

Depends on Phase 2.

## References

- [RFD 065: Typed Resource Model for Attachments][RFD 065] — defines the
  `Resource` type whose content payloads are externalized by this RFD.
  Identifies inline content bloat as a blocking concern requiring this solution.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058] — defines
  `ContentBlock` and `Resource` types whose content payloads are externalized.
- [RFD 036: Conversation Compaction][RFD 036] — compaction drops old turns,
  potentially orphaning blobs that GC cleans up.
- [RFD 020: Parallel Conversations][RFD 020] — parallel writes to the blob
  store are safe due to content-addressing.

[RFD 020]: 020-parallel-conversations.md
[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
