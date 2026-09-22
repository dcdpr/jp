# RFD D34: Patch-Based Delivery for Changed Resources

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-07-01
- **Extends**: [RFD 067]

<!--
  The Extends field is recorded manually: `just rfd-extend` refuses targets
  that are still in Discussion. Run it (or let rfd-promote fill the
  back-link) once RFD 067 is Accepted.
-->

## Summary

[RFD 067] replaces a redundant resource delivery with a reference when the
content is identical to a prior delivery.
This RFD adds a third outcome for the case [RFD 067] falls through on: when the
content has *changed* and the prior delivery is still within the LLM's effective
context, JP sends a unified diff instead of the full content.

## Terminology

This RFD builds on vocabulary from RFDs that are still in Discussion and cannot
be assumed known.
Brief definitions follow; each links to the RFD that owns the full design.

- **Resource** — the typed unit of content JP delivers to the LLM: a file, a
  web page, command output ([RFD 065]).
- **Resource block** — one resource embedded in a tool response, per [RFD
  058]'s typed content blocks.
- **Canonical URI** — a resource's identity, produced by the handler or tool
  that resolved it, e.g. `file:///project/src/main.rs` ([RFD 065]).
  A partial read is identified by a *fragment URI*
  (`file:///project/src/main.rs#L10-200`), which is a distinct identity.
- **Checksum** — the SHA-256 digest of a resource's raw content, computed by
  the **blob store** — [RFD 066]'s content-addressed storage — when the
  content is persisted.
- **Delivery** — this RFD's term for the moment a resource's content enters the
  conversation for the LLM: as an attachment, a tool result, or a
  `refresh_resource` result.
- **Reference** — the replacement [RFD 067] emits for redundant content: a
  short message pointing at an earlier delivery ("identical to tool call
  `call_3` at turn 5").
- **Decision point** — the step in [RFD 067]'s tool pipeline (the
  `ToolCoordinator`) where each resource block about to be delivered is matched
  against conversation history and its delivery form is chosen.
- **`lookback_turns`**, **`min_bytes`** — [RFD 067] settings: matching
  considers only deliveries within the last N turns, and only content above a
  minimum size.
- **`refresh_resource`** — the built-in tool ([RFD 065]) with which the LLM
  re-resolves an attached resource through its original handler.

## Motivation

Under [RFD 067], every resource delivered to the LLM carries a canonical URI
([RFD 065]) and a content checksum ([RFD 066]).
When a resource about to be delivered matches a prior delivery on **both** —
same URI, same checksum — JP replaces the redundant content with a short
reference to the earlier delivery.
Anything else is delivered in full.

Consider the most common tool loop in a coding session:

1. The LLM reads `src/main.rs` at turn 5.
   Full content delivered — roughly 2,000–3,000 tokens for a 500-line file.
2. The LLM edits the file, changing five lines.
3. The LLM re-reads the file at turn 6 to verify the edit.

At step 3 the URI matches the turn-5 delivery but the checksum does not: the
content changed.
[RFD 067] deliberately treats this as no match — a reference would claim the
content is unchanged when it isn't — so the full file is delivered again, even
though it differs from the turn-5 delivery by five lines.
A unified diff of that edit costs under 100 tokens.

That edit-then-verify case dominates real coding sessions by frequency.

A second scenario shows the magnitude end: a conversation carries a large,
mostly-stable resource that is refreshed every turn — a project file listing,
`git status` output, a dependency report.
Each refresh changes a handful of entries out of hundreds.
Here the diff is not merely cheaper; it is the semantically useful answer ("what
changed since last time"), and the reassembly risk is low because the LLM rarely
needs the reconstructed full listing verbatim.

Without this RFD, deduplication only helps content that never changes; both
patterns pay full price on every iteration.

## Design

### A third outcome at [RFD 067]'s decision point

This RFD extends [RFD 067]'s per-block matching algorithm.
No new pipeline stage is added; the patch check is a new arm at the same
decision point in the `ToolCoordinator`, and applies to every path [RFD 067]
covers: tool responses, `refresh_resource` calls ([RFD 065]), and re-attached
resources.

For each resource block about to be delivered:

| Condition                                            | Outcome                             |
| ---------------------------------------------------- | ----------------------------------- |
| URI and checksum match a full delivery               | Reference ([RFD 067])           |
| URI and checksum match only patch deliveries         | **Full content** (see re-read rule) |
| URI matches, checksum differs, patch conditions hold | **Patch (this RFD)**                |
| Anything else                                        | Full content                        |

### Patch conditions

A patch is delivered only when **all** of the following hold:

1. A **full** delivery of the same canonical URI exists within `lookback_turns`
   — [RFD 067]'s window of recent turns eligible for matching.
   That delivery is the patch base; later patch deliveries of the same URI do
   not disqualify it (see base invariant below).
2. The base content is retrievable from the blob store ([RFD 066]).
   A failed retrieval — missing blob, checksum mismatch — fails this
   condition.
3. Both the base and the new content are text: their resources declare a text
   mimeType ([RFD 065]) and their bytes are valid UTF-8.
4. The new content exceeds `min_bytes` — [RFD 067]'s minimum content size,
   below which replacement overhead negates the savings.
5. The patch is small relative to the content: `patch_bytes / content_bytes <=
   max_patch_ratio`.

If any condition fails, JP delivers the full content.
This fall-through is a correctness invariant, not a tuning preference.
A reference that the LLM cannot locate degrades gracefully — the model
re-reads.
A patch without a locatable base fails dangerously — the model applies a diff
to content it cannot see and hallucinates the result.
JP never delivers a patch unless the exact base is present, verbatim, within the
lookback window.

### Base invariant: no patch-on-patch

Every patch is computed against the most recent **full** delivery of the
resource, never against a previous patch.
This guarantees that any patch in the context window applies to exactly one full
delivery that is also in the context window; the model never has to stack patch
applications.

The consequence: successive patches are computed against the same base, are each
**complete** — all changes since the base, so a later patch supersedes earlier
ones — and grow in size.
When the cumulative diff exceeds `max_patch_ratio` (or the base ages out of
`lookback_turns`), the conditions fail, JP delivers the full content, and that
delivery becomes the new base.
A resource that changes every turn — a file listing gaining a file per turn,
say — thus cycles: patches accumulate until a condition fails, a full delivery
resets the base, and between resets every patch costs at most `max_patch_ratio`
of the full content.

JP retrieves the base content from the blob store ([RFD 066]) using the checksum
recorded in the conversation history, and computes the diff at delivery time.
Handlers and tools are not involved — they return content as they do today.

### Delivery records

[RFD 067] records each delivery's canonical URI and checksum in the conversation
history.
Patching adds two requirements to that record:

- **The delivery form.** Each record carries how the content was delivered:
  full, reference, or patch.
  The base lookup considers only full deliveries, and the re-read rule below
  distinguishes full from patch deliveries; neither is answerable from `(uri,
  checksum)` alone.
- **The assembled checksum.** A patch delivery records the checksum of the
  assembled new content, not of the patch text.
  A later read returning identical content must match this record — that match
  is what triggers the re-read rule.

### Re-reads after a patch deliver full content

A patch serves change-awareness, but sometimes the LLM needs the assembled
current state — re-reading a file to orient itself, not to verify an edit.
Without an escape hatch that state is unobtainable: a re-read returns content
identical to the patch delivery's record, and the reference arm would answer
with a pointer to the patch.

The rule is state-based, not time-based.
A reference is emitted only when a **full** delivery with the same canonical URI
and checksum exists within `lookback_turns` — matching prefers full deliveries
over patch deliveries of the same content.
If the checksum matches only patch deliveries, the assembled content has never
been in the context window, and JP delivers it in full.
That delivery becomes the new patch base, so a subsequent identical read gets a
reference — the escape hatch fires once per patch state, not once per read.

The model self-selects per read: verify-intent reads are satisfied by the diff;
reorient-intent reads cost one extra round-trip and receive full content.
Reads of a different line range carry a different fragment URI and never match,
so they receive full content without invoking this rule.

### Patch format

Patches are unified diffs, prefixed by a message that names the base delivery
explicitly:

> The content of file:///project/src/main.rs has changed since the version
> delivered in tool call `call_3` at turn 5.
> Apply the following changes to that version to obtain the current content.
> These changes supersede any earlier change listing against that version:

```diff
--- file:///project/src/main.rs (as delivered at turn 5)
+++ file:///project/src/main.rs (current)
@@ -42,7 +42,7 @@
 fn main() {
-    println!("hello");
+    println!("hello, world");
 }
```

Unified diff is chosen because it is the diff format LLMs encounter most in
training data (git output pervades the corpus), it is compact, and it is
human-auditable in the conversation log.
The default of 3 context lines per hunk is a starting point; whether more
context improves the model's reconstruction reliability is a validation question
(see Implementation Plan).

How a patch delivery is displayed in the terminal is governed by the existing
tool call style configuration (`style.tool_call`), unchanged by this RFD.

### Configuration

The knobs live in [RFD 067]'s namespace, since patching is an arm of the same
decision:

```toml
[conversation.deduplication]
# Deliver unified diffs for changed resources (default: false until
# validated; see Implementation Plan).
patch = false

# Deliver full content instead of a patch when the patch exceeds this
# fraction of the full content size.
max_patch_ratio = 0.5
```

Disabling deduplication — per-tool `deduplicate = false` or
`conversation.deduplication.enabled = false` — disables patching too, since the
patch check lives inside the dedup decision point.

Patching can also be disabled per tool while keeping references:

```toml
[tools.fs_read_file]
# This tool's results are never delivered as patches. References and
# full delivery apply as usual.
patch = false
```

Because `refresh_resource` ([RFD 065]) is itself a tool, this axis already
separates code reads from attachment refreshes: an operator who finds patched
code reads unhelpful disables `patch` on `fs_read_file` and keeps it on
`refresh_resource`, with no source-specific semantics in the pipeline.

## Drawbacks

**The model must reconstruct state.** A reference points at intact content; a
patch demands that the model mentally apply a diff to an earlier delivery.
If the model does this unreliably, it reasons about a file state that does not
exist — strictly worse than delivering the full content.
This is the central risk of the proposal and the reason it ships disabled.

**Two representations of one resource.** After a patch, the context window
contains the full base content and the diff.
The model must prefer the patched state over the (more prominent) full base.
The prefix message mitigates this but cannot eliminate it.

**Pipeline complexity.** The delivery path gains a blob fetch, a diff
computation, and a third outcome to reason about.
Diff cost itself is negligible at the file sizes involved.

## Alternatives

### Handler-computed patches

A stateful handler ("smart file list") remembers or queries its prior delivery
and turn number, computes its own diff, and applies its own memorability logic
to decide between full content and a patch.

**Rejected because:** [RFD 067] already rejected producer-side conversation
awareness ("tool-side conversation context access") for breaking statelessness
and testability; this is the same alternative for attachment handlers.
It also requires `jp_attachment` to read conversation history — a new
cross-crate dependency — and reimplements the diff and gating logic once per
handler.
The one thing handler-side logic offers, semantically meaningful diffs ("added
foo.rs, removed bar.rs" instead of a text diff of a rendered listing), has a
boundary-preserving escape hatch: a handler may supply a pre-rendered
representation via the `formatted` field on `Resource` ([RFD 065]), while JP
retains the deliver-full-vs-patch decision.

### Patch only attachment-sourced resources

Restrict patching to attachment re-deliveries (`refresh_resource`, re-attach)
and exclude tool reads like `fs_read_file`, on the grounds that a model
re-reading a file after an edit may need assembled context, while a refreshed
listing benefits most from a diff.

**Rejected because:** the source is a proxy for the real variable — whether the
read wants change-awareness or assembled state — and the proxy is unreliable in
both directions.
[RFD 065] deliberately unified attachments and tool responses under one
`Resource` model; a source split here would special-case the axis that
unification removed.
The intent asymmetry is addressed directly instead: the re-read rule lets the
model recover full content when a patch is not enough, and per-tool `patch =
false` excludes tools whose reads are predominantly re-orientation.

### Structured edit lists

Deliver changes as a JSON list of edit operations instead of a unified diff.

**Rejected because:** models encounter unified diffs far more often in training
data, the JSON encoding costs more tokens, and the result is harder to audit in
the conversation log.

### Always deliver full content

The status quo under [RFD 067].

**Rejected as insufficient because:** it leaves the edit-verify loop — the
dominant source of redundant delivery — entirely unoptimized.
It remains the fallback whenever patch conditions fail.

## Non-Goals

- **Range-subset dedup.** Detecting that a partial read is contained in a full
  file already in context remains out of scope, as in [RFD 067].
  This RFD is temporal (same URI, new content), not spatial.
- **Binary patching.** Patches apply to text content only.
- **Patch chains.** Every patch applies to a full delivery; see the base
  invariant.
- **Rename detection.** A patch is never computed across different canonical
  URIs.
- **Changes to [RFD 067]'s reference mechanism when patching is inactive.** With
  `patch = false` (or this RFD absent), identical content behaves exactly as
  [RFD 067] specifies.
  With patching active, the one deliberate deviation is the re-read rule:
  identical content whose only in-window match is a patch delivery is delivered
  in full.

## Risks and Open Questions

### Can models reliably apply in-context patches?

This is the viability question for the entire RFD.
The expectation is that patches pay off for large files with small, localized
changes and degrade as changes grow or scatter — `max_patch_ratio` bounds the
second case but not model fallibility in the first.
If validation shows unreliable reconstruction, this RFD is abandoned and the
pipeline falls back to [RFD 067] behavior unchanged; the design isolates
patching behind one condition arm precisely so the failure mode is removal, not
rework.

### Interaction with compaction

Conversation compaction — dropping or summarizing old events to shrink the LLM
request ([RFD 036], [RFD 064]) — can remove the base delivery a patch was
computed against, leaving the patch dangling.
The same concern exists for [RFD 067]'s references, but here it is a correctness
problem, not an inconvenience.
Compaction must either preserve full deliveries that serve as patch bases within
the lookback window, or JP must treat a compacted base as failing condition 1
and deliver full content.
The second is the safe default and requires only that the base lookup consult
the post-compaction stream.

### Patch-then-reread thrashing

If a model habitually follows every patch with a full re-read, patching costs an
extra tool round-trip and delivers full content anyway — worse than delivering
full content up front.
Phase 2 validation measures the re-read rate per tool; a tool that thrashes
ships with `patch = false`.

### Tuning defaults

`max_patch_ratio = 0.5` and 3 context lines are starting points, validated in
Phase 2.

## Implementation Plan

### Phase 1: Patch arm and configuration

Implement the patch conditions, base lookup via blob store, diff computation,
and the prefix message format in the `ToolCoordinator` dedup decision point.
Extend [RFD 067]'s delivery record with the delivery form and the assembled
content checksum (see Delivery records).
Add `patch` and `max_patch_ratio` to `conversation.deduplication`.
Ships with `patch = false`.

Depends on [RFD 067] Phase 2 (per-block matching) and [RFD 066] (blob retrieval
by checksum).
Can be reviewed and merged independently.

### Phase 2: Validation

With `patch = true` in test workspaces, measure:

- Reconstruction reliability: does the model act on the patched state or the
  stale base?
  Across providers and model sizes.
- Token savings in representative edit-verify sessions.
- Sensitivity to `max_patch_ratio` and context-line count.
- Re-read rate per tool: how often a patch is followed by a full re-read of the
  same resource (patch-then-reread thrashing).

Depends on Phase 1.

### Phase 3: Default flip or abandonment

If Phase 2 validates reliability, flip the default to `patch = true`.
If it does not, abandon this RFD; Phase 1's code is removed and [RFD 067]
behavior is unaffected.

## References

- [RFD 065: Typed Resource Model for Attachments][RFD 065] — canonical URIs,
  the `Resource` type, the `formatted` escape hatch, and `refresh_resource`.
- [RFD 066: Content-Addressable Blob Store][RFD 066] — checksum-addressed
  retrieval of base content for diff computation.
- [RFD 067: Resource Deduplication for Token Efficiency][RFD 067] — the
  decision point, identity model, and configuration namespace this RFD extends.
- [RFD 036: Conversation Compaction][RFD 036] and [RFD 064: Non-Destructive
  Conversation Compaction][RFD 064] — interaction with dropped base deliveries.

[RFD 036]: ../036-conversation-compaction.md
[RFD 058]: ../058-typed-content-blocks-for-tool-responses.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
[RFD 066]: ../066-content-addressable-blob-store.md
[RFD 067]: ../067-resource-deduplication-for-token-efficiency.md
