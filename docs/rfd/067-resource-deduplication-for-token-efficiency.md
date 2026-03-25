# RFD 067: Resource Deduplication for Token Efficiency

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-15

## Summary

This RFD introduces resource deduplication in JP's tool execution pipeline. When
a tool returns a resource that has already been delivered to the LLM — either as
an attachment or a previous tool call — JP detects the overlap and replaces the
redundant content with a reference to the earlier delivery. This avoids wasting
tokens on content the LLM already has in its context window.

## Motivation

A common pattern in JP conversations:

1. The user attaches `foo.rs` via `--attachment file://foo.rs`.
2. The user asks a question about the file.
3. The LLM calls `fs_read_file("foo.rs")` — re-reading a file whose contents are
   already in the conversation as an attachment.

The tool runs, produces the same content, and JP delivers it to the LLM. The LLM
now has the file contents twice in its context window: once from the attachment
and once from the tool call. For large files, this wastes significant tokens.

The same problem occurs across turns. The LLM reads `foo.rs` in turn 3, then
reads it again in turn 7. If the file hasn't changed, the second delivery is
redundant.

Today, JP has no way to detect this. Tools return opaque strings, and JP has no
resource identity information to compare against conversation history. With the
MCP-aligned content model (typed resource blocks with URIs), JP gains the
metadata needed to detect and eliminate redundant deliveries.

### Scale of the problem

In a typical coding session, `fs_read_file` is one of the most frequently called
tools. Files are often re-read after edits (to verify changes), after branch
switches (to check current state), or simply because the LLM lost track of what
it already has. Each redundant delivery of a 500-line source file costs roughly
2,000–3,000 tokens. Over a long conversation with dozens of tool calls, this
adds up.

## Design

### Overview

Every resource block delivered to the LLM — whether from an attachment or a tool
call — has a canonical URI ([RFD 065]) and a content checksum computed by the
blob store ([RFD 066]). When a new tool response arrives with resource blocks,
JP compares each `(canonical_uri, checksum)` pair against the conversation
history. Each resource block is evaluated independently: matched blocks are
replaced with a reference to the earlier delivery, while new or changed blocks
are formatted and delivered normally.

### Content identity

Dedup matching uses two properties already present on every resource in the
conversation stream:

- **Canonical URI** — from `Resource.uri` ([RFD 065]).
- **Content checksum** — the SHA-256 digest computed by the blob store on write
  ([RFD 066]), exposed via `BlobContent::Ref`.

No additional tagging infrastructure is needed. Tools and attachment handlers
return content as normal; the blob store computes and stores the checksum as
part of its content-addressing mechanism.

JP treats resource URIs as opaque identifiers. JP does not attempt to
canonicalize URIs across tools or fragment formats. Two tools that identify the
same content with different URIs will not deduplicate. This is acceptable — see
[Non-Goals](#non-goals).

### Matching algorithm

Dedup operates **per resource block**, not per response. Each resource block in
a tool response is evaluated independently:

1. For each resource block, extract the canonical URI from `Resource.uri` and
   the content checksum from `BlobContent::Ref` ([RFD 066]).
2. Search the conversation stream for a matching resource:
   - Check all `Resource` entries in `ChatRequest.resources` (attachments).
   - Check all `ContentBlock::Resource` entries in previous tool responses.
3. A resource block is a **match** if both the canonical URI and the checksum
   are equal to a resource in the history.

For each block in the response:

- **Match** → replace with a reference message (see below).
- **No match** → format and deliver the full content normally.

This works because JP formats resource blocks individually — each block has its
own URI and mimeType, and the final response is the concatenation of per-block
formatted output. There is no opaque combined string to decompose.

A response with 4 resource blocks where 3 are unchanged results in 3 short
reference messages and 1 fully delivered resource. `text` blocks pass through
unconditionally — see [Non-Goals](#non-goals) for rationale and a sketch of how
text block dedup could work in a future RFD.

### Replacement message

When JP deduplicates a resource block, it replaces that block's content with a
short reference message telling the LLM where to find the original:

**Match against attachment:**

> The content of file:///project/src/main.rs is identical to the attachment at
> turn N (checksum: sha256:a1b2c3). The file has not changed. Refer to the
> attached content.

**Match against previous tool call:**

> The content of file:///project/src/main.rs is identical to the result of tool
> call `call_7` in turn 3 (checksum: sha256:a1b2c3). The file has not changed.
> Refer to that earlier result for the full contents.

The replacement message includes:

- The resource URI (so the LLM knows what resource was requested).
- Where the original content lives (attachment or tool call ID + turn number).
- The checksum (for transparency).
- A clear statement that the content has not changed.

In a multi-resource response, each deduplicated block gets its own reference
message. Non-deduplicated blocks are formatted normally. The LLM sees a mix of
references and full content.

### How matching handles identity correctly

The combination of canonical URI *and* checksum prevents false positives:

- **Same file, different content** (file was edited between reads): URI matches
  but checksum does not → no dedup. Content is delivered normally.
- **Different files, same content** (two identical files): Checksums match but
  URIs do not → no dedup. The LLM asked for different resources.
- **Same file, same content** (file unchanged): Both match → dedup.
- **Attachment and tool for same file** (user attached `foo.rs`, LLM reads
  `foo.rs`): URI and checksum match → dedup. This is the primary use case.
- **Same line range, same content** (LLM reads lines 10–200 twice via the same
  tool, file unchanged): URIs match (same tool produces the same fragment),
  checksums match → dedup.
- **Line range vs full file** (LLM reads lines 10–200, full file already
  attached): Different URIs (`file:///…/foo.rs#L10-200` vs `file:///…/foo.rs`) →
  no dedup. See [Non-Goals](#non-goals).

### Pipeline integration

The dedup check runs in the `ToolCoordinator`, after a tool returns and before
the response is delivered to the LLM. The flow:

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

This runs after the tool has executed. The tool always runs — dedup saves
tokens, not execution cost. Skipping tool execution based on predicted output is
not feasible: tools may have side effects (file writes, git operations, network
requests) that must occur regardless of whether the returned content is
redundant.

### Configuration

Dedup is enabled by default for tools that return resource blocks. It can be
disabled per-tool in configuration:

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

### Minimum size threshold

Dedup only applies to resource blocks whose raw content exceeds 300 bytes. Below
this threshold, the replacement message itself approaches the size of the
original content, negating the token savings. A resource block of 200 bytes
costs roughly 50–75 tokens; the replacement message costs roughly 40–50 tokens.
The net savings are negligible and the LLM loses direct access to the content.

The threshold is configurable:

```toml
[conversation.deduplication]
min_bytes = 300
```

Resource blocks below the threshold are delivered normally regardless of whether
they match a previous delivery.

### Lookback window

Dedup only matches against resource deliveries within a configurable number of
recent turns. If the original delivery is older than the lookback window, JP
delivers the full content instead of a reference.

This mitigates the attention-degradation problem: in long conversations, the
LLM's ability to locate and attend to content from early turns diminishes. A
reference to "see tool call X in turn 3" is useful when the current turn is 5;
it is less useful when the current turn is 40.

```toml
[conversation.deduplication]
lookback_turns = 30
```

The default of 30 turns is a conservative starting point. It can be tuned based
on provider-specific context window characteristics and validated during Phase
3.

There is no plan to make deduplication configurable for attachment handlers in
this RFD.

## Drawbacks

**The tool still executes.** Dedup saves tokens but not execution time. For
`fs_read_file` this is negligible, but for `web_fetch` the network round-trip
still happens. Skipping tool execution is not feasible because tools may have
side effects that must occur regardless of whether the output is redundant.

**The LLM must locate the referenced content.** When JP replaces content with
"see the attachment" or "see tool call X in turn N", the LLM needs to find that
content in its context window. In long conversations, attention to earlier
content degrades. If the LLM can't find the reference, dedup actively hurts.
This risk is mitigated by the replacement message being explicit about where to
look, but it cannot be eliminated.

## Alternatives

### Tool-provided checksums

Have tools compute and return checksums alongside their content. JP matches
checksums without computing them.

**Rejected because:** It requires every tool to implement hashing, and tools and
attachment handlers must agree on the hashing convention. If a tool hashes the
raw file but the attachment handler hashes the encoded content, checksums won't
match. JP computing checksums itself eliminates this coordination problem.

### System prompt mitigation

Tell the LLM in the system prompt not to re-read files that are already
attached. No protocol changes needed.

**Rejected as the sole solution because:** LLMs frequently ignore such
instructions, especially in long conversations. This can be used as a
complementary measure but is not reliable enough to depend on.

### Tool-side conversation context access

Give tools access to their own invocation history so they can decide whether to
return content. The tool checks "did I already return this file?" and responds
with a "no changes" message instead.

**Rejected because:** It breaks tool statelessness. Tools become aware of
conversation context, which makes them harder to test, harder to reason about,
and introduces privacy/security concerns. JP is the natural place for this logic
since it already owns both the conversation stream and the tool execution
pipeline.

## Non-Goals

- **Cross-conversation dedup.** Resources are deduplicated within a single
  conversation only. Sharing resource checksums across conversations introduces
  state management complexity that is not justified by the use case.

- **Range-subset dedup** (detecting that lines 10–20 are contained within a full
  file already in context). Partial reads use fragment URIs
  (`file:///…/foo.rs#L10-200`) that are distinct from the full-file URI.
  Identical range requests deduplicate normally; cross-range overlap detection
  adds significant complexity (tracking per-resource coverage, handling
  overlapping ranges, invalidating on edits) for marginal token savings on
  already-small partial reads.

- **Cross-tool URI canonicalization.** Different tools (or different versions of
  the same tool) may represent the same resource with different URIs — for
  example, `#L10-200` vs `#line=9,200` for the same line range, or
  `file:///project/src/main.rs` vs `file:///project/./src/main.rs`. JP does not
  attempt to canonicalize across these conventions. Tools that use consistent
  URIs get dedup; tools that don't, don't. If measurement shows significant
  missed dedup from URI inconsistency, canonicalization can be addressed in a
  future RFD.

- **Text block dedup.** This RFD deduplicates `resource` blocks only. `text`
  blocks (informational tool output like compiler errors or command results)
  lack URIs and cannot use the `(uri, checksum)` identity model. A future RFD
  could introduce text block dedup using `(tool_name, args_hash,
  content_checksum)` as the identity key — same tool, same arguments, same
  output. This is deferred because the replacement message design is harder for
  text blocks: the LLM needs enough context to understand what the tool produced
  without the full output, and the right format (preview-based, summary-based,
  or reference-only) needs validation with actual LLM behavior. The blob store
  ([RFD 066]) already provides checksums for text blocks, so the storage
  infrastructure is in place when this is pursued.

## Risks and Open Questions

### Interaction with conversation compaction

[RFD 036] describes conversation compaction, which may drop old tool responses
to reduce context size. If a compacted conversation drops the tool response that
a dedup reference points to, the LLM receives "see tool call X in turn 3" but
that turn has been compacted away.

Mitigations:

- Compaction could preserve resource metadata (URIs, checksums) even when
  content is dropped.
- The lookback window (see [Lookback window](#lookback-window)) limits matching
  to recent turns, reducing overlap with compacted history.
- If the referenced content was compacted, JP falls through to full delivery.

The specifics depend on the compaction design. This interaction should be
addressed when compaction is implemented.

### Replacement message format

The exact wording and structure of the replacement message affects LLM behavior.
Too terse and the LLM can't find the reference. Too verbose and we negate the
token savings. The format proposed in this RFD is a starting point; it should be
validated with actual LLM behavior during implementation, and potentially made
configurable.

### Partial file reads and fragment URIs

`fs_read_file` with `start_line`/`end_line` returns a `resource` block with a
fragment URI (e.g., `file:///project/src/foo.rs#L10-200`). Since JP treats URIs
as opaque, repeated reads of the same range through the same tool deduplicate
normally (same URI string, same checksum). A partial read is never matched
against the full file or a different range — the fragment makes them distinct
URI strings.

## Implementation Plan

### Phase 1: Resource block migration

Update `fs_read_file` to return `resource` blocks (including fragment URIs for
partial rea:s).

Depends on [RFD 065] (`Resource` type), [RFD 066] (blob store checksums), and
[RFD 058] (`ContentBlock` type).

### Phase 2: Dedup matching

Implement the per-block matching algorithm in `ToolCoordinator`. After a tool
returns resource blocks, check each block's URI and checksum against
conversation history. Matched blocks get reference messages; unmatched blocks
are formatted normally.

Add configuration knobs (`deduplicate` per-tool, `conversation.deduplication`
namespace).

Depends on Phase 1.

### Phase 3: Validate and tune

Test with real conversations to validate:

- Replacement message format (does the LLM find the referenced content?).
- False positive rate (are there cases where dedup fires incorrectly?).
- Token savings (measure actual reduction in representative sessions).
- Minimum size threshold (validate the 300-byte default).
- Lookback window (validate the 30-turn default).

Adjust the replacement message format and thresholds based on findings.

Depends on Phase 2.

## References

- [MCP Tools Specification
  (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/server/tools.md)
  — defines typed content blocks that make resource identification possible.
- [MCP Resources Specification
  (2025-11-25)](https://modelcontextprotocol.io/specification/2025-11-25/server/resources.md)
  — defines resource URIs, content types, and annotations.
- [RFD 036: Conversation Compaction](036-conversation-compaction.md) — relevant
  for interaction between dedup references and compacted history.
- [RFD 058: Typed Content Blocks for Tool Responses][RFD 058] — defines the
  `ContentBlock` type that carries resource blocks in tool responses.
- [RFD 065: Typed Resource Model for Attachments][RFD 065] — defines the
  `Resource` type and places attachment content on `ChatRequest.resources`.
- [RFD 066: Content-Addressable Blob Store][RFD 066] — provides SHA-256
  checksums as content-address keys, reused by dedup for identity matching.

[RFD 036]: 036-conversation-compaction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
