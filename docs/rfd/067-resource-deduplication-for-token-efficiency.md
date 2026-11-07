# RFD 067: Resource Deduplication for Token Efficiency

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15
- **Requires**: [RFD 065], [RFD 066], [RFD 058]
- **Summary**: Replaces duplicate attachment and tool-result resources in LLM
  requests with compact references when URI and checksum match.

## Summary

This RFD deduplicates resource blocks in the tool execution pipeline.
When a tool returns a resource whose canonical URI and content checksum match an
earlier attachment or tool response in the conversation, JP replaces the
duplicate content with a short reference to that earlier delivery.

## Motivation

A common pattern in JP conversations:

1. The user attaches `foo.rs` via `--attachment file://foo.rs`.
2. The user asks a question about the file.
3. The LLM calls `fs_read_file("foo.rs")`, re-reading a file already in the
   conversation as an attachment.

The tool runs and returns the same content, placing the file in the context
window twice: once from the attachment and once from the tool call.
The same duplication occurs across turns when the LLM reads `foo.rs` in turn 3
and reads it again in turn 7 without the file changing.

In a coding session, `fs_read_file` is one of the most frequently called tools.
The LLM re-reads files to verify edits, check state after branch switches, or
recover context it lost track of.
Each redundant delivery of a 500-line source file costs roughly 2,000 to 3,000
tokens, which compounds over a long conversation.

When tools return opaque strings, JP has no resource identity to compare against
conversation history.
Typed resource blocks with URIs give JP the metadata needed to detect and
replace duplicate deliveries.

## Design

### Overview

Every resource block delivered to the LLM, whether from an attachment or a tool
call, has a canonical URI ([RFD 065]) and a content checksum computed by the
blob store ([RFD 066]).
When a tool response arrives, JP compares each resource block's `(canonical_uri,
checksum)` pair against the conversation history.
Matched blocks are replaced with a reference to the earlier delivery; new or
changed blocks are formatted and delivered in full.

### Content identity

Deduplication matches on two properties already present on every resource in the
conversation stream:

- **Canonical URI**: `Resource.uri` ([RFD 065]).
- **Content checksum**: the SHA-256 digest computed by the blob store on write
  ([RFD 066]) and exposed via `BlobContent::Ref`.

Tools and attachment handlers need no extra tagging; they return content as
usual, and the blob store computes the checksum when writing the blob.

JP compares resource URIs as opaque strings without normalizing across tools or
fragment formats.
Two tools that identify the same content with different URIs do not deduplicate
(see [Non-Goals](#non-goals)).

### Matching algorithm

Deduplication operates per resource block rather than per response.
Because each block carries its own URI and `mimeType`, and JP formats the final
tool response by concatenating per-block output, JP can replace individual
blocks without decomposing a combined string.

For each resource block in a tool response:

1. Extract the canonical URI from `Resource.uri` and the content checksum from
   `BlobContent::Ref` ([RFD 066]).
2. Search earlier turns in the conversation stream for a resource with the same
   URI and checksum across:
   - `Resource` entries in `ChatRequest.resources` (attachments).
   - `ContentBlock::Resource` entries in prior tool responses.
3. If both the canonical URI and the checksum match an earlier resource, replace
   the block with a reference message; otherwise, format and deliver the full
   content.

A response with four resource blocks where three are unchanged produces three
reference messages and one full resource.
`text` blocks pass through unchanged (see [Non-Goals](#non-goals)).

### Matching cases

Requiring both canonical URI and checksum to match prevents false positives:

- **Same file, different content** (file edited between reads): URI matches,
  checksum differs.
  Delivered in full.
- **Different files, same content** (two identical files): Checksum matches, URI
  differs.
  Delivered in full because the LLM requested distinct resources.
- **Same file, same content** (file unchanged): Both match.
  Deduplicated.
- **Attachment and tool call for the same file** (user attached `foo.rs`, LLM
  reads `foo.rs`): Both match.
  Deduplicated.
- **Same line range, same content** (`fs_read_file` with `start_line`/`end_line`
  reads lines 10 to 200 twice on an unchanged file): Both the fragment URI
  (e.g., `file:///project/src/foo.rs#L10-200`) and the checksum match.
  Deduplicated.
- **Line range vs. full file** (LLM reads lines 10 to 200 after the full file
  was attached): URIs differ (`file:///…/foo.rs#L10-200` vs.
  `file:///…/foo.rs`).
  Delivered in full (see [Non-Goals](#non-goals)).

### Replacement message

When JP deduplicates a resource block, it replaces the block's content with a
reference to the earlier delivery:

**Match against attachment:**

> The content of file:///project/src/main.rs is identical to the attachment at
> turn N (checksum: sha256:a1b2c3).
> The file has not changed.
> Refer to the attached content.

**Match against previous tool call:**

> The content of file:///project/src/main.rs is identical to the result of tool
> call `call_7` in turn 3 (checksum: sha256:a1b2c3).
> The file has not changed.
> Refer to that earlier result for the full contents.

Each replacement message states the requested resource URI, the location of the
original delivery (attachment turn, or tool call ID and turn), the checksum, and
that the content has not changed.

### Pipeline integration

Deduplication runs in `ToolCoordinator` after a tool returns and before JP
delivers the response to the LLM:

```txt
Tool executes
  → Tool returns content blocks
    → For each resource block:
        → JP computes (canonical_uri, checksum)
        → JP checks against conversation history
          → Match?    → Replace block with reference message
          → No match? → Format block normally
    → text blocks pass through as-is
    → Checksums are already persisted by the blob store (RFD 066)
```

The tool always executes first because JP needs its output checksum to match
against history, and because tools may have side effects (file writes, Git
operations, network requests) that must run even when the returned content is
unchanged.

### Configuration

Deduplication is enabled by default for tools that return resource blocks and
can be disabled per tool:

```toml
[tools.fs_read_file]
# Disable dedup for this tool
deduplicate = false
```

A conversation-level toggle is also available:

```toml
[conversation.deduplication]
enabled = false
```

Deduplication is not configurable per attachment handler.

### Minimum size threshold

JP only deduplicates resource blocks whose raw content exceeds `min_bytes`
(default `300`); smaller blocks are delivered in full even when they match an
earlier delivery.
A 200-byte resource costs roughly 50 to 75 tokens, while the replacement message
itself costs 40 to 50 tokens, so deduplicating small blocks trades away direct
access to the content for negligible savings.

```toml
[conversation.deduplication]
min_bytes = 300
```

### Lookback window

JP only matches against resource deliveries within the last `lookback_turns`
turns (default `30`).
If the prior delivery is older than this window, JP delivers the full content
again.

In long conversations, the LLM's attention to early turns degrades: a reference
to turn 3 is easy to follow in turn 5, but harder to resolve in turn 40.

```toml
[conversation.deduplication]
lookback_turns = 30
```

The 30-turn default is a conservative starting point to tune against
provider-specific context-window behavior in Phase 3.

## Drawbacks

- **Tools still execute.** Deduplication saves tokens, not execution time.
  The cost is negligible for `fs_read_file`, but tools like `web_fetch` still
  make a network round-trip before JP can compare the returned content.
- **The LLM must locate the referenced content.** Replacing a payload with a
  pointer to an earlier turn requires the LLM to retrieve that content from its
  context window.
  If attention to earlier turns degrades and the LLM fails to resolve the
  reference, deduplication hurts response quality.
  Explicit turn and tool-call identifiers in the replacement message and the
  lookback window reduce this risk, but do not eliminate it.

## Alternatives

### Tool-provided checksums

Have tools compute and return checksums alongside their content so JP can match
checksums without hashing.

**Rejected:** Every tool and attachment handler would have to implement hashing
and agree on what bytes to hash.
If a tool hashed raw file bytes while an attachment handler hashed encoded
content, their checksums would not match.
Computing checksums in the blob store removes this coordination problem.

### System prompt mitigation

Instruct the LLM in the system prompt not to re-read files that are already
attached, avoiding pipeline changes.

**Rejected as the sole solution:** LLMs often ignore prompt instructions,
especially in long conversations.
Prompt guidance can complement pipeline deduplication, but cannot replace it.

### Tool-side conversation context access

Give tools access to their invocation history so they can check whether they
already returned a file and reply that it has not changed.

**Rejected:** This breaks tool statelessness.
Context-aware tools are harder to test and reason about, and exposing
conversation history to tools raises privacy and security concerns.
JP already owns both the conversation stream and the tool execution pipeline, so
deduplication belongs in JP.

## Non-Goals

- **Cross-conversation deduplication.** Resources are deduplicated only within a
  single conversation.
  Sharing resource checksums across conversations adds state management
  complexity that the use case does not justify.

- **Range-subset deduplication.** Partial reads use fragment URIs
  (`file:///…/foo.rs#L10-200`) that are distinct from the full-file URI.
  Repeated reads of the exact same range deduplicate, but detecting that lines
  10 to 20 are already covered by an attached full file or a wider range would
  require tracking per-resource line coverage, merging overlapping ranges, and
  invalidating ranges on edits for minor token savings on already-small reads.

- **Cross-tool URI canonicalization.** Different tools (or tool versions) may
  format the same resource URI differently, such as `#L10-200` vs. `#line=9,200`
  or `file:///project/src/main.rs` vs. `file:///project/./src/main.rs`.
  JP does not normalize across these conventions; only tools that emit identical
  URIs deduplicate against each other.
  URI normalization can be proposed in a later RFD if inconsistent URIs cause
  significant missed matches.

- **Text block deduplication.** This RFD deduplicates `resource` blocks only.
  Informational `text` blocks (such as compiler errors or command output) have
  no URI and cannot use `(uri, checksum)` identity.
  A future RFD could key `text` blocks by `(tool_name, args_hash,
  content_checksum)`, which the blob store ([RFD 066]) already checksums.
  That work is deferred because replacing free-form tool output requires
  validating whether the LLM needs a preview, a summary, or a bare reference.

## Risks and Open Questions

### Interaction with conversation compaction

Conversation compaction ([RFD 036]) may drop older tool responses to shrink the
context window.
If compaction removes a tool response that a later deduplication reference
points to, the LLM receives a pointer to a turn that has been compacted away.

Mitigations to address when implementing compaction:

- Preserve resource metadata (URIs and checksums) during compaction even when
  content is dropped.
- Rely on the [lookback window](#lookback-window) to limit matching to recent
  turns, reducing overlap with compacted history.
- Fall back to full delivery if the referenced content has been compacted away.

### Replacement message format

The wording of the replacement message affects how reliably the LLM locates the
prior content: a message that is too terse may leave the LLM unable to find the
reference, while a verbose message erodes the token savings.
The proposed template is a starting point to validate against real LLM behavior
during implementation and potentially expose in configuration.

## Implementation Plan

### Phase 1: Resource block migration

Update `fs_read_file` to return `resource` blocks, including fragment URIs for
partial reads.

Depends on [RFD 065] (`Resource` type), [RFD 066] (blob store checksums), and
[RFD 058] (`ContentBlock` type).

### Phase 2: Deduplication matching

Implement per-block deduplication in `ToolCoordinator` and add the per-tool
`deduplicate` setting and `conversation.deduplication` configuration section.

Depends on Phase 1.

### Phase 3: Validation and tuning

Evaluate deduplication in real conversations and tune defaults based on the
results:

- Whether the LLM reliably locates referenced content using the replacement
  message format.
- False positive rate across tool calls and attachments.
- Net token savings in representative sessions.
- The 300-byte `min_bytes` default and 30-turn `lookback_turns` default.

Depends on Phase 2.

## References

- [MCP Tools Specification (2025-11-25)]: Typed content blocks for tool
  responses.
- [MCP Resources Specification (2025-11-25)]: Resource URIs, content types, and
  annotations.
- [RFD 036: Conversation Compaction][RFD 036]: Compaction of older conversation
  turns and its interaction with deduplication references.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058]: Defines the
  `ContentBlock` type that carries resource blocks in tool responses.
- [RFD 065: Typed Resource Model for Attachments][RFD 065]: Defines the
  `Resource` type and stores attachment content on `ChatRequest.resources`.
- [RFD 066: Content-Addressable Blob Store][RFD 066]: Provides the SHA-256
  checksums used to match resource content.

[MCP Resources Specification (2025-11-25)]: https://modelcontextprotocol.io/specification/2025-11-25/server/resources.md
[MCP Tools Specification (2025-11-25)]: https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md
[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
