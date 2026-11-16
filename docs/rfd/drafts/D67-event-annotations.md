<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D67: Event Annotations

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-10
- **Requires**: [RFD D68]
- **Summary**: Stores schema-tagged JSON annotations addressed to conversation
  locations in `annotations.json` without sending them to the model.

## Summary

This RFD introduces **annotations**: structured JSON records attached to
locations in a conversation.
Annotations are stored in `annotations.json` alongside `events.json` and are
never sent to the model.
Each annotation declares the schema of its payload with a `schema_id`, so tools
can skip formats they do not recognize.

## Terminology

- **Annotation**: structured data attached to a location in a conversation.
- **Address**: a reference to a location in a conversation (an entry, a text
  span within an entry, or a range between two points), defined by [RFD D68].
- **Stream entry**: an element in a conversation's ordered stream, either a
  conversation event or an overlay.
- **Overlay**: a position-independent stream entry that governs how surrounding
  events are interpreted ([RFD 064]).
- **Event ID**: the stable per-event identifier introduced by [RFD 097].
- **Thread**: the rendered form of a conversation sent to an LLM provider,
  assembled at query time from the event stream.
- **Storage root**: either of the two directories a conversation is written to:
  the user's data directory or the workspace.
- **User-local copy**: the copy of a conversation in the user's data directory,
  which is the source of truth for its contents.
- **Workspace projection**: the copy of a conversation written to
  `.jp/conversations/` in the workspace so it can be committed to version
  control.
- **Trash**: moving a corrupt or invalid conversation directory into `.trash/`,
  preserving its contents alongside a `TRASHED.md` file recording the reason.

## Motivation

The event stream records what happened in a conversation, but provides no place
to record conclusions about it: that a tool result was wrong, that a specific
paragraph is worth keeping, or that a claim needs verification before anyone
acts on it.

Today, users have two workarounds.
Editing `events.json` injects the note into the conversation stream, where the
model reads it on the next turn and changes its behavior.
Keeping notes outside the workspace preserves the conversation stream, but the
link is fragile: a note referencing "the third tool call" breaks as soon as
entries move.
Commentary either contaminates the conversation or drifts away from it.

Annotations solve both problems.
They live alongside the conversation, travel with it into version control, point
at a specific address in the stream, and never enter the model's input.

## Design

### Annotation Record

```json
{
  "id": "a7k2m9x",
  "target": {
    "entry": "k3m9x2a",
    "pointer": "/content",
    "quote": {
      "exact": "empty tables parse fine now"
    }
  },
  "schema_id": ".jp/schemas/review-note/v1.json",
  "data": {
    "verdict": "wrong",
    "note": "still panics on a table holding only a comment"
  }
}
```

- `id`: unique identifier for the annotation, used to edit or remove individual
  records.
  Because multiple annotations can share the same `target` and `schema_id`,
  `target` cannot serve as the record's identity.
- `target`: an address naming a stream entry, a text span within an entry, or a
  range between `start` and `end` points, as defined by [RFD D68].
  Quotes are stored as plain text rather than URL-encoded, so inspecting the
  file with `jq` shows the quoted text directly.
- `schema_id`: required identifier for the schema of `data`, written as an
  absolute URI or a path relative to the workspace root.
- `data`: arbitrary JSON payload.
  JP stores `data` opaquely without inspecting its contents.

**JP does not validate `data`.** `schema_id` is a declaration by the writer, not
a contract JP enforces: JP never resolves, fetches, or parses the referenced
schema.
Consumers that recognize the `schema_id` parse `data` accordingly; consumers
that do not recognize it skip the annotation.
Any format-specific metadata, such as who wrote the annotation, belongs in
`data` as declared by its schema.

### File Layout

Annotations live in `annotations.json` in the conversation directory alongside
`metadata.json`, `base_config.json`, and `events.json`.

Annotations travel with the conversation.
`Projection::Projected` writes them to both the user-local copy and the
workspace projection, so they are committed alongside the events they describe.

`annotations.json` is a flat JSON array, matching the structure of
`events.json`:

```json
[
  {
    "id": "a7k2m9x",
    "target": {
      "entry": "k3m9x2a",
      "pointer": "/content",
      "quote": {
        "exact": "empty tables parse fine now"
      }
    },
    "schema_id": ".jp/schemas/review-note/v1.json",
    "data": {
      "verdict": "wrong",
      "note": "still panics on a table holding only a comment"
    }
  },
  {
    "id": "b2p8w4t",
    "target": {
      "start": {
        "entry": "m1v5r7d"
      },
      "end": {
        "entry": "q9t3k6b"
      }
    },
    "schema_id": ".jp/schemas/review-note/v1.json",
    "data": {
      "verdict": "digression",
      "note": "abandoned approach, skip this on a reread"
    }
  }
]
```

The second annotation targets a range of entries rather than a text span, so its
`start` and `end` endpoints specify only `entry` IDs.

Storing a flat array rather than a map keyed by `target` avoids using range
addresses as map keys and lets callers remove an annotation by `id` without
searching nested lists.

### Loading and Saving with the Stream

A conversation's stream and its annotations load and save as a single unit,
extending the existing invariant in `jp_storage::load`: "The stream
(`base_config.json` + `events.json`) is one unit: load both."

This coupling is required by [RFD 097].
`ConversationStream` tracks entry IDs duplicated at load in private, load-scoped
state, and RFD 097 requires consumers to resolve ambiguous references within the
same load cycle before saving.
Loading annotations separately would lose that duplication state, causing an
annotation to silently rebind to the wrong copy of a duplicated entry.

`ConversationStream::from_parts` and `to_parts` gain annotations as a third
part, so `PersistBackend::write` and `LoadBackend::load_conversation_stream`
keep their existing signatures without new backend methods.

Annotations remain in a separate file rather than inside `events.json` because
`stream_mtime` (the newer modification time of `events.json` and
`base_config.json`) drives conversation ordering.
Excluding `annotations.json` from `stream_mtime` ensures that writing a note
does not count as conversation activity or move a conversation to the top of `jp
conversation ls`.

### Write Ordering

`write_json` is atomic per file, and no atomic write spans all three stream
files.
`persist_conversation_to` writes `annotations.json` last, after `events.json`.
If a save is interrupted, the conversation at worst loses a newly written
annotation rather than persisting an annotation whose target entry was never
written.

### Missing or Corrupt Files

A missing `annotations.json` means the conversation has no annotations.

A malformed `annotations.json` is renamed to `annotations.json.invalid`, and the
conversation loads with no annotations.
Unlike a missing or corrupt `events.json`, which causes `validate_entry` to
trash the conversation directory, a malformed `annotations.json` **never**
trashes the conversation.
Renaming the file preserves the user's data from being overwritten on the next
save, and because `cleanup_tmp_files` removes only paths ending in `.tmp`,
`annotations.json.invalid` survives startup cleanup.

### Unresolvable Addresses

[RFD D68] defines when an address resolves.
At load time, any annotation whose target address does not resolve is dropped
and logged.

This matches how `ConversationStream::retain` drops compaction overlays when an
entry removal could have invalidated their anchors, except that annotations
validate against entry IDs and quotes rather than turn ranges.

### Forking

`jp conversation fork` copies annotations to the new conversation.
When `--last N` drops earlier stream entries, any annotation whose target
address no longer resolves against the forked stream is dropped with them.

### Workspace Schema Convention

For project-local formats, `schema_id` can be a path relative to the workspace
root:

```
.jp/schemas/review-note/v1.json
```

Placing workspace schemas under `.jp/schemas/` gives tools a shared convention
for locating schema files.
JP does not create `.jp/schemas/`, read from it, or validate against anything in
it.

Whether `schema_id` is a relative path or an absolute URI, it should include a
version identifier.
Skipping unrecognized schemas fails if a format changes incompatibly without
changing its `schema_id`.

### Library and CLI

The write path is a library API in `jp_conversation`, with the CLI as a thin
wrapper around it.

Because RFD D68 compares quoted text by exact byte match, the library captures
quotes directly from the stored entry rather than accepting caller-supplied
text.
Hand-pasted quotes can carry different bytes than the stored text and fail to
match visually identical content.

## Drawbacks

- **Every whole-conversation operation gains a third file.** Archive and
  unarchive are directory renames and need no changes, but fork, export, and the
  plugin protocol must each handle `annotations.json`.
- **Malformed `data` payloads are detected only at read time.** Because JP does
  not validate `data`, a buggy writer can persist invalid payloads that fail
  only when a consumer parses them.
- **Annotations load with the stream.** Opening a conversation always
  deserializes `annotations.json`, even when the caller never reads annotations.
- **`schema_id` conventions are unenforced.** Nothing prevents two tools from
  using the same `schema_id` for different schemas, or a tool from changing its
  schema without updating the identifier.

## Alternatives

**A fifth `EventPayload` variant in `events.json`.** Because RFD 097 makes every
stream entry ID-addressable, annotations could be stored as another event
payload kind.
This was rejected for two reasons: annotating a conversation would update
`stream_mtime` and reorder `jp conversation ls`, and it would mix metadata the
model never sees into the file used to build the model's input.

**A schema defined by JP.** Typed annotations with fixed fields would let JP
validate, search, and render them directly.
This was rejected because JP cannot anticipate every annotation workflow, and
any fixed schema would be too narrow for some uses.
Tagging payloads with `schema_id` gives external tools structured data without
requiring JP to own the schema vocabulary.

**Conversation-level notes.** Attaching notes to the conversation as a whole is
simpler, requires no text addressing, and removes the dependency on RFD D68.
This was rejected because the value of an annotation lies in its target link:
recording that a specific tool result is wrong is useful, whereas recording that
"something in this conversation was wrong" is not.

**Independently loadable annotations.** Loading and saving `annotations.json`
independently of `ConversationStream` would let background processes write
annotations without loading the stream.
This is rejected for now because it bypasses RFD 097's load-scoped duplicate-ID
resolution and creates two writers over cross-referenced files without atomic
multi-file writes.
This can be revisited if plugin or background write paths require it.

## Non-Goals

- **Sending annotations to the model.** Annotations never enter the Thread.
- **Validating `data`.** `schema_id` is an unenforced declaration by the writer.
- **Assistant or plugin authorship.** Only the `jp_conversation` library API and
  the CLI write annotations in this RFD.
  A plugin protocol message belongs in a later RFD, and [RFD 077] owns the trust
  policy it would require.
- **Searching inside `data`.** `jp conversation grep` searches only the
  conversation stream.
- **Rendering.** How and where annotations are displayed is left open.

## Risks and Open Questions

- **No atomic write spans the files.** Writing `annotations.json` after
  `events.json` ensures an interrupted save at worst loses a note rather than
  leaving an annotation pointing at a missing entry, but a crash mid-save can
  still leave the files out of sync.
- **The annotation ID format is unsettled.** Reusing the `EventId` format
  (short, opaque, unique within its file, assigned on insertion) is the natural
  choice and costs nothing to adopt, but it has not been finalized.
- **`.jp/schemas/` is an unenforced convention.** Because JP does not inspect
  the directory, tools can ignore it.

## Implementation Plan

### Phase 1: Record Type

Define `Annotation` in `jp_conversation` with its ID, target address,
`schema_id`, and `data` fields, plus `serde` support.
Depends on RFD D68 for the address type and can merge on its own.

### Phase 2: Storage

Extend `from_parts` and `to_parts` to carry annotations, and write
`annotations.json` after `events.json`.
Treat a missing `annotations.json` as empty, rename a malformed file to
`annotations.json.invalid` without trashing the conversation, and drop
annotations whose address does not resolve at load.

Tests cover round-trip serialization through both files, renaming a malformed
file while still loading the conversation, and dropping annotations when their
target entry is removed.

Depends on Phase 1.

### Phase 3: Library and CLI

Add the `jp_conversation` API to write, edit, and remove annotations, with the
CLI wrapping it.
Quote capture reads directly from the stored entry rather than accepting pasted
text.

Depends on Phase 2.

### Phase 4: Fork and Documentation

Carry annotations through `fork`, dropping any whose address no longer resolves.
Document `.jp/schemas/`, and add "Annotation" to
`docs/wiki/00dxybd-ubiquitous-language.md`.

Depends on Phase 3.

## References

- [RFD 064], compaction overlays, whose drop-on-mutation rule annotations follow
- [RFD 072], the plugin protocol that a future plugin write path would extend
- [RFD 077], plugin configuration and trust policy
- [RFD 097], stable stream entry identifiers
- [RFD D68], the text addressing scheme used by `target`

[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 077]: ../077-plugin-configuration-and-trust-policy.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D68]: D68-text-addressing-within-events.md
