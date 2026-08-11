<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D52: Event Annotations

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-10
- **Requires**: [RFD D59](D59-text-addressing-within-events.md)

## Summary

A conversation records what happened.
This RFD adds a place to record what you concluded about it: annotations, JSON
records attached to a location in a conversation, kept in `annotations.json`
beside `events.json` and never sent to the model.
Each names the shape of its own data with a `schema_id`, so a tool that does not
recognize a format can skip it.

## Terminology

- **Annotation**: structured data about a location in a conversation.
- **Stream entry**: one element of the ordered sequence that makes up a
  conversation.
  Conversation events and overlays are both stream entries.
- **Overlay**: a stream entry that applies to the conversation regardless of its
  position, governing how the events around it are interpreted.
  Introduced by [RFD 064].
- **Event ID**: the per-event identifier introduced by [RFD 097].

Terms this RFD borrows from the rest of the system:

- **Address**: a reference naming a location in a conversation, defined by [RFD
  D59].
- **Thread**: the rendered form of a conversation sent to an LLM provider,
  assembled at query time from the event stream.
- **Storage root**: either of the two directories a conversation is written to,
  the user's data directory or the workspace.
- **User-local copy**: the copy of a conversation in the user's data directory,
  and the source of truth for its contents.
- **Workspace projection**: the copy of a conversation written into the
  workspace's `.jp/conversations/` directory so it can be committed alongside
  the project.
- **Trash**: to move a conversation directory into `.trash/`, preserving its
  contents, with a `TRASHED.md` recording the reason.

## Motivation

The event stream records what happened.
Nothing in it holds what you concluded about it: that a tool result was wrong,
that one paragraph of a response is the part worth keeping, that a claim needs
checking before anyone acts on it.

Two workarounds exist and both cost something.
Editing `events.json` puts the note inside the conversation, so the model reads
it on the next turn and it changes what happens next.
Keeping notes outside the workspace preserves the conversation but loses the
link: the note says "the third tool call" and then the third tool call moves.

Doing nothing leaves those as the only options.
Commentary either contaminates the thing it is about or drifts away from it.

Annotations are a third place.
They live with the conversation, travel with it into version control, point at a
specific location, and stay out of the model's input.

## Design

### The record

```json
{
  "id": "a7k2m9x",
  "target": {
    "entry": "k3m9x2a",
    "pointer": "/content",
    "quote": { "exact": "empty tables parse fine now" }
  },
  "schema_id": ".jp/schemas/review-note/v1.json",
  "data": {
    "verdict": "wrong",
    "note": "still panics on a table holding only a comment"
  }
}
```

- `id` identifies the annotation, so one can be edited or removed on its own.
  Two annotations may share a target and a schema, so the target is not an
  identity.
- `target` is an address.
  It names a stream entry, a run of text within one, or a range between two
  points.
  [RFD D59] defines the shape, including the `start` and `end` form a range
  takes.
  The quote is stored as written rather than encoded, so `jq` over this file
  shows what was pointed at.
- `schema_id` names the shape of `data`.
  Required.
  Either an absolute URI or a path relative to the workspace root.
- `data` is JSON.
  JP stores it and never looks inside.

**JP does not validate `data`.** `schema_id` is a promise made by whoever wrote
the annotation, not a contract JP enforces.
JP never resolves it, fetches it, or parses anything it points at.
A reader that recognizes the `schema_id` knows what it is looking at, and one
that does not skips the annotation.
That is the whole mechanism.

Anything a format needs to record about itself, including who wrote the
annotation, is a field in `data` that its schema declares.

### Where annotations live

`annotations.json`, in the conversation directory beside `metadata.json`,
`base_config.json` and `events.json`.

Annotations travel with the conversation.
`Projection::Projected` writes them to the user-local copy and to the workspace
projection, so they are committed alongside the events they describe.

The file is a flat array, matching the shape of `events.json`:

```json
[
  { "id": "a7k2m9x",
    "target": { "entry": "k3m9x2a", "pointer": "/content",
                "quote": { "exact": "empty tables parse fine now" } },
    "schema_id": ".jp/schemas/review-note/v1.json",
    "data": { "verdict": "wrong",
              "note": "still panics on a table holding only a comment" } },

  { "id": "b2p8w4t",
    "target": { "start": { "entry": "m1v5r7d" },
                "end": { "entry": "q9t3k6b" } },
    "schema_id": ".jp/schemas/review-note/v1.json",
    "data": { "verdict": "digression",
              "note": "abandoned approach, skip this on a reread" } }
]
```

The second annotation covers a stretch of the conversation rather than a piece
of text, so both of its endpoints name entries and stop there.

Flat rather than a map keyed by target, because a range address does not make a
usable key, and because removing one annotation should not mean finding it
inside a nested list.

### One unit, two files

A conversation's stream and its annotations load and save together.
`jp_storage::load` already states this about the two files it has: "The stream
(`base_config.json` + `events.json`) is one unit: load both."
Annotations join that unit.

This follows from [RFD 097].
The set of entry IDs that were duplicated at load is private, load-scoped state
on `ConversationStream`, and RFD 097 requires a consumer to resolve ambiguous
references within the same load cycle, before any save.
Loading annotations separately would put that signal out of scope, and an
annotation would silently rebind to the wrong copy of a duplicated entry.

`ConversationStream::from_parts` and `to_parts` already map parts to files, so
annotations become a third part.
`PersistBackend::write` and `LoadBackend::load_conversation_stream` keep their
signatures and no backend gains a method.

Two files rather than one, because `stream_mtime` is the newer of `events.json`
and `base_config.json` and it drives conversation ordering.
Annotations stay out of that calculation: writing a note is not conversation
activity and should not move a conversation to the top of `jp conversation ls`.

### Writing

`write_json` is atomic per file and no atomic write spans the three.
`persist_conversation_to` writes `annotations.json` last, after `events.json`.
An interrupted save then loses a note, rather than leaving an annotation that
points at an entry which was never persisted.

### A missing or corrupt file

A missing `annotations.json` means the conversation has no annotations.

A malformed one is renamed to `annotations.json.invalid` and the conversation
loads with no annotations.
It is **not** trashed.
`validate_entry` trashes a conversation directory for a missing or corrupt
`events.json`, and a bad notes file must never cost someone their conversation.
The rename preserves the user's bytes, which the next save would otherwise
overwrite.
`cleanup_tmp_files` removes only paths ending in `.tmp`, so the renamed file
survives startup.

### When an annotation stops pointing at anything

[RFD D59] defines when an address resolves.
An annotation whose address does not resolve is dropped at load, and each drop
is logged.

This is the same shape of rule `ConversationStream::retain` already applies to
compaction overlays, which are dropped when a removal could have invalidated
their anchors.
Annotations track their own validity, keyed by entry ID rather than by turn
range.

### Forking

`jp conversation fork` carries annotations to the new conversation.
`--last N` drops entries, and any annotation whose address no longer resolves
against the forked stream is dropped with them.

### Schemas in the workspace

A `schema_id` may be a path relative to the workspace root, for a format local
to one project:

```
.jp/schemas/review-note/v1.json
```

`.jp/schemas/` is a convention, so that two tools in the same workspace do not
invent two places to look.
JP does not create the directory, read from it, or validate against anything in
it.

The version belongs inside the identifier, whichever form it takes.
"Skip what you do not recognize" fails the moment a format changes without its
identifier changing.

### Library and CLI

The write path is a library API in `jp_conversation`, with the CLI as a shell
over it.

Addresses quote text from the stored stream, and RFD D59 compares those quotes
exactly, so the library captures the quote from the entry itself.
A hand-pasted quote may carry different bytes than the stored text and would
then fail to match text that looks identical.

## Drawbacks

- **Every whole-conversation operation gains a third artifact.** Fork, export,
  and the plugin protocol each grow a file to remember.
  Archive and unarchive are directory renames, so they are free; the rest are
  not.
- **A malformed `data` is found only by whoever reads it.** JP validates
  nothing, so a writer that gets its own format wrong produces annotations that
  look fine until a reader chokes on them.
- **Annotations load with the stream**, so a large `data` value is paid on every
  open of that conversation, whether or not anything reads it.
- **`schema_id` conventions are unenforced.** Nothing stops two tools using one
  identifier for different shapes, or one tool changing its shape without
  changing its identifier.

## Alternatives

**A fifth `EventPayload` variant in `events.json`.** RFD 097 makes every stream
entry ID-addressable, so a new payload kind would fit.
Rejected on two counts: annotating would change `stream_mtime` and reorder `jp
conversation ls`, and it would put a concern the model never sees into the file
the model's input is built from.

**A payload structured by JP.** Typed annotations with known fields would let JP
render, search and validate them.
Rejected because JP cannot anticipate what people annotate with, and every fixed
field would be wrong for someone.
`schema_id` gives any tool that wants structure the same benefit, without JP
owning the vocabulary.

**Conversation-level notes.** Simpler, no addressing, no dependency on RFD D59.
Rejected because the value is in the link.
"This tool result is wrong" is worth recording; "something in this conversation
was wrong" is not.

**Independently loadable annotations.** Loading and saving annotations on their
own would let a long-running process write them without touching the stream.
Rejected for now: it puts RFD 097's ambiguity signal out of scope and creates
two writers over data that reference each other with no atomic write between
them.
Worth revisiting if plugin or background writes arrive.

## Non-Goals

- **Sending annotations to the model.** They never enter the Thread.
- **Validating `data`.** `schema_id` is a promise.
- **Assistant or plugin authorship.** The write path is the library and the CLI.
  A protocol message for plugins is a later RFD, and [RFD 077] owns the trust
  policy it would need.
- **Searching inside `data`.** `jp conversation grep` searches the stream.
- **Rendering.** How and where annotations display is left open.

## Risks and Open Questions

- **No atomic write spans the files.** The write order makes the worst case a
  lost note rather than an annotation pointing at nothing, but the window
  exists.
- **The annotation ID form is unsettled.** Following `EventId` (short, opaque,
  unique within its file, assigned on insertion) is the obvious answer and costs
  nothing to adopt, but it has not been decided.
- **`.jp/schemas/` is a convention JP does not enforce**, so nothing stops it
  being ignored.

## Implementation Plan

### Phase 1: the record type

`Annotation` in `jp_conversation`, carrying its ID, address, `schema_id` and
`data`, with serde.
Depends on RFD D59 for the address type.
Mergeable on its own.

### Phase 2: storage

Extend `from_parts` and `to_parts` to carry annotations.
Write `annotations.json` after `events.json`.
Add the missing-file and malformed-file behavior, and drop annotations whose
address does not resolve at load.

Tests: round-trip through both files; a malformed file is renamed and the
conversation still loads; removing an entry drops the annotations that named it.

Depends on Phase 1.

### Phase 3: library and CLI

The write, edit and remove API, with the CLI over it.
Quote capture reads from the stored entry rather than accepting pasted text.
Depends on Phase 2.

### Phase 4: fork and documentation

Carry annotations through `fork`, dropping any that no longer resolve.
Document `.jp/schemas/`, and add "Annotation" to
`docs/architecture/ubiquitous-language.md`.
Depends on Phase 3.

## References

- [RFD 064], compaction overlays, whose drop-on-mutation rule annotations follow
- [RFD 072], the plugin protocol a future write path would extend
- [RFD 077], plugin trust policy
- [RFD 097], stable entry identifiers
- [RFD D59], the addressing scheme `target` uses

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 077]: ../077-plugin-configuration-and-trust-policy.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D59]: D59-text-addressing-within-events.md
