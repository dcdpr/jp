<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D59: Text Addressing Within Events

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-10
- **Extends**: [RFD 097](../097-stable-event-identifiers.md)
- **Required by**: [RFD D52](D52-event-annotations.md)

## Summary

A `jp://` URL names a conversation.
This RFD extends it with a fragment that names a location inside one: a stream
entry, a run of text within one, or a range between two points.
Locations are anchored by quoted text, with an optional position that breaks
ties between repeated matches, so an address either resolves to exactly one
place or fails visibly.

## Terminology

- **Address**: a reference that names a location in a conversation: a stream
  entry, a run of text within one, or a range between two points.
- **Text quote selector**: naming a location by quoting the text there, along
  with the text immediately before and after.
- **Text position selector**: naming a location by its position, counted in
  Unicode code points from the start of the text.
- **Fragment**: the part of a URI following `#`, naming a part of the resource
  that the rest of the URI identifies.
- **Grapheme cluster**: what a reader perceives as one letter, which may be
  several Unicode code points.

The two selector names come from the Web Annotation Data Model ([WADM]).
This RFD takes the names and sets its own rules, which are stricter: each point
that specification leaves to the implementation is fixed here.

Terms this RFD borrows from the rest of the system:

- **Event ID**: the per-event identifier introduced by [RFD 097].
- **`jp://` scheme**: the URI scheme for JP-internal resources, of the form
  `jp://<jp-id>[?<query>]`, resolved by `jp_attachment_internal`.

## Motivation

`jp://jp-c17013123456?select=a:-1` is as precise as JP gets today: a
conversation, filtered by role and turn.
There is no way to point at a sentence.

Three things want one.
An annotation about a specific claim in a response ([RFD D52]) has nothing to
attach to.
Quoting a passage from another conversation means attaching the whole message.
A link into a rendered conversation cannot point at what it is about.

In addition to addressing any quote, it should be possible to address objects
like entire turns, or a compaction overlay with the same URI scheme.

[RFD 097] supplies half the answer by giving every stream entry a stable ID.
What remains is naming a location inside an entry, and doing it in a way that
survives the hand-editing RFD 097 was written for.
A character offset alone does not: insert a word earlier in a message and every
offset after it points somewhere else, with no signal that anything moved.

## Design

### The URL

An address is a `jp://` URL carrying a fragment:

```
jp://jp-c17013123456#k3m9x2a@/content:t=Y2hva2Vz
```

The part before `#` identifies the conversation, unchanged.
The fragment names the location: an entry, a run of text within one, or a range
between two points.
RFC 3986 puts resource identification in the query and part-of-resource
identification in the fragment, which is exactly this split.

The fragment is discarded today, asserted by
`uri_to_entry_ignores_path_and_fragment`, so this gives meaning to a component
that is currently free.

### Grammar

**Proposed here, not settled.**

```abnf
fragment = endpoint [ ".." endpoint ]
endpoint = entry-id [ "@" pointer [ ":" anchor ] ]
anchor   = "t=" exact [ ";p=" prefix ] [ ";s=" suffix ]
         / "o=" offset
```

```
jp://jp-c17013123456#k3m9x2a
jp://jp-c17013123456#k3m9x2a@/arguments/query
jp://jp-c17013123456#k3m9x2a@/content:t=Y2hva2Vz
jp://jp-c17013123456#k3m9x2a@/content:o=42..z4n8v6c@/content:t=ZW1wdHkgbWFw
```

An address stops wherever it wants to.
Name an entry and you have named the entry; add a field and you have named the
whole of that field; add an anchor and you have named a run of text inside it.

That matters because `TurnStart` holds no text at all, so naming the entry is
the only way to reach it.
It is also how you address a whole tool call without singling out a field, and a
whole turn by naming its `TurnStart`.

Entries that are not conversation events are reached the same way as any other.
A compaction's summary is generated prose, at `/summary/summary`, and a config
delta's values are strings.

None of the delimiters (`@`, `:`, `;`, `=`, `.`) appear in the base64url
alphabet, so an encoded quote can never be mistaken for structure.
`..` is the range separator because JP already writes ranges that way, inclusive
on both ends, in `--turn A..B`, the compaction DSL and timeline output.

### JSON form

An address is written two ways: as a fragment in a URL, and as JSON wherever one
is stored.
They carry the same information and convert both ways.

```json
{ "entry": "k3m9x2a" }

{ "entry": "k3m9x2a", "pointer": "/arguments/query" }

{ "entry": "k3m9x2a", "pointer": "/content",
  "quote": { "exact": "empty tables parse fine now" } }

{ "entry": "k3m9x2a", "pointer": "/content", "offset": 42 }

{ "start": { "entry": "k3m9x2a", "pointer": "/content", "offset": 42 },
  "end":   { "entry": "z4n8v6c", "pointer": "/content",
             "quote": { "exact": "empty map" } } }
```

`prefix` and `suffix` sit beside `exact` when present.
`quote` and `offset` are mutually exclusive.

One location is written flat and a range nests under `start` and `end`.
The key that is present says which: `entry` means one location, `start` means a
range.
A range is a different thing from a location, so it gets a different shape
rather than a `start` with nothing after it.

Text is written plainly here, not base64url encoded.
Encoding exists to stop a quote colliding with the fragment's delimiters, and
JSON has no such problem, so anyone running `jq` over a stored address sees what
it points at.

### Naming a string inside an entry

The pointer is a JSON Pointer ([RFC 6901]) into the entry's decoded JSON.

Any string value is addressable.
There is no list of event kinds and no per-kind default, so a chat request's
text, a tool call's `arguments`, a tool response's `content` and the thinking
signatures in an entry's `metadata` are all reached the same way.
Adding a new event kind requires no change here.

"Decoded" matters: `jp_conversation::storage` base64-encodes tool arguments,
tool response content and metadata at rest.
Pointers and offsets address the decoded values, never the stored encoding.

This RFD addresses **any** stream entry, not only conversation events.
RFD 097 gives config deltas, compactions and unrecognized entries stable IDs but
exposes only conversation events programmatically, leaving the rest "until a
consumer RFD defines the raw-entry view it needs."
This is that RFD, and it defines that view.

### Anchors

An anchor is either a quote or an offset.

A **quote** carries the exact text, and optionally the text immediately before
and after it.
Prefix and suffix are never required.
They raise the chance that a quote is unique without bounding it, so mandating
them would buy ceremony rather than a guarantee.

A quote is a predicate over the text, so it matches zero, one or many places.
That is its nature, not a defect.

An **offset** is a single number, counted in Unicode code points, starting at
zero, sitting *between* characters.
One number is enough because an anchor names a point, not a span.
Position zero is before the first character.

Offsets count code points rather than bytes, which differs from `jp conversation
grep --json`, whose `submatches` are byte offsets mirroring `rg --json`.
Those coordinates index a string emitted in the same document; these are durable
addresses that people write by hand, and a byte offset can land mid-character.
The library converts, so "grep, then annotate what I found" never asks a user to
do the arithmetic.

An address that would start or end inside a grapheme cluster is rejected when it
is created, so every stored address is well formed and resolution never handles
half a character.

### Ranges

Two endpoints, separated by `..`, name a range.
It runs from the **begin** of the first anchor's match to the **end** of the
second anchor's match, inclusive.
The endpoints need not use the same kind of anchor: a quote at one end and an
offset at the other is allowed.

When both endpoints name the same entry, the result is an ordinary span, with no
special case.

A range denotes a region of the stream, not a string.
Config deltas and compactions may lie between its endpoints, so anything
extracting content from a range receives a sequence of entries with partial
ends, not a single run of text.

The extent of a range is the entries lying between its endpoints in array order.
That part is not content-anchored, and RFD 097 forbids using entry IDs for
ordering, so reordering entries by hand changes what a range covers.
Quoting cannot fix this, because the middle is not quoted.
Two rules keep the failure visible: a range is unresolved if either endpoint
entry is gone, and unresolved if the end entry does not follow the start entry.

### Resolution

An address naming only an entry resolves if that entry is present.
One naming a field resolves if the pointer finds a string there.

Within a string, the quote is the anchor of record and the offset breaks ties.

```
1. Match the quote against the addressed string.  -> zero, one, or many
2. Exactly one candidate                          -> resolved
3. Several candidates, offset present             -> the candidate nearest it
4. Several candidates, no offset                  -> unresolved
5. No candidates                                  -> unresolved
6. Offset with no quote                           -> resolved at that point
```

Comparison is exact.
No Unicode normalization is applied to either side, so a single coordinate
system serves both quotes and offsets and nothing sits between the bytes on disk
and what an address means.
The cost is that a quote captured from a differently normalized source fails to
match text that looks identical, which is why RFD D52 has the library capture
quotes from the stored entry.

An offset must not win over a quote.
An offset always resolves, and after an edit it resolves to the wrong place
silently, which is the hazard RFD 097 exists to remove: a reference should
become "a *detectable* mismatch, never a silent positional aliasing."
Line 6 above is a deliberate exception with a stated property: an address with
no quote is fragile by construction, useful for pointing at something now, wrong
for anything durable.

Used as a tie-break rather than an authority, an offset also degrades
gracefully.
Small edits shift it, but the quote has already narrowed the field to real
occurrences, so the nearest candidate is still the right one.

### Encoding

Quote text is base64url encoded without padding, using
`base64::engine::general_purpose::URL_SAFE_NO_PAD`.
Its alphabet is `A-Za-z0-9-_`, all unreserved, so a fragment never needs
percent-encoding and a quote can hold `#`, `&`, newlines or anything else.

This differs deliberately from `jp_conversation::storage`, which uses
`STANDARD`.
That alphabet contains `+`, `/` and `=`, and `=` would collide with the grammar
above.

`exact`, `prefix` and `suffix` are encoded separately, so the delimiters between
them stay visible.
Offsets are plain integers: there is no Unicode in `42`, and encoding it would
hide the one part of an address a person can check by eye.

The encoding belongs to the URL.
Where an address is stored, as in RFD D52's `annotations.json`, the fields are
kept decoded, so `jq` over the file shows what was quoted.

### Interaction with `select=`

`select=` filters which parts of a conversation an attachment pulls in, by role
and turn.
Applied to an address, it intersects with the range:

- The fragment gives an ordered set of entries with a partial span at each end.
- `select=` filters that set.
  Absent means no filtering.
- It is an error for the filter to exclude an entry that an endpoint names.
  The caller's own filter contradicting the location they asked for is a bad
  request, distinct from an unresolved address, and the message should say
  which: "select=a excludes k3m9x2a, where the range starts."

### Where this differs from the Web Annotation Data Model

WADM is an interchange format for independent clients annotating documents they
do not control, so its matching rules are mostly advisory.
JP defines its own storage, so each of those points is fixed here.
JP addresses are a strict subset of conformant WADM selectors: emitting valid
WADM is possible, accepting everything WADM permits is not.

| WADM | Here |
| --- | --- |
| Range runs to the *beginning* of the end selector, excluding it | Runs to the end of it, inclusive |
| Start and end selectors SHOULD be the same class | Either end may be a quote or an offset |
| Multiple matches SHOULD select all of them | Nearest to the offset, or unresolved |
| Text MUST be normalized (markup stripped) before recording | No normalization; the text is the decoded stored value |
| A `State` is RECOMMENDED alongside a position selector | No `State`; the quote is the anchor of record |
| Selections SHOULD NOT split a grapheme cluster | Rejected at creation |

## Drawbacks

- **This modifies shipped behavior.** `jp_attachment_internal` currently
  discards the fragment, with a test asserting it.
  D59 stops being purely additive the moment that changes.
- **A range's extent is position-dependent** and no anchoring fixes it, so
  reordering entries changes what a range covers.
- **Two offset conventions now exist** in the user-facing surface: `grep --json`
  counts bytes, addresses count code points.
- **Encoded quotes are not readable in a URL.** The information survives, but
  eyeballing an address does not tell you what it points at.

## Alternatives

**Offsets only, no quotes.** Short, deterministic, always resolves.
Rejected because it always resolves *to something*, and after an edit that
something is wrong with no signal.
Determinism without correctness is worse than a visible failure.

**Quotes only, no offsets.** Removes the fragile form entirely.
Rejected because a repeated quote then has no tie-break short of extending
prefix and suffix (which is also not guaranteed to resolve the problem), and
because a short hand-written address has real value.

**Content-addressed anchors, hashing the target text.** Precise and tamper
evident, but any edit invalidates the anchor, which defeats the editing workflow
which was one of the motivations for the design.

**A separate address scheme rather than a `jp://` fragment.** Rejected because
`jp://` already identifies conversations and RFC 3986 already reserves the
fragment for addressing within a resource.

## Non-Goals

- **Validating what an address points at.** Resolution says where; it does not
  interpret.
- **Byte-level addressing.** WADM calls that a `DataPositionSelector`.
  Offsets here count code points.
- **Addressing rendered output.** Addresses name the stored text, not what a
  terminal or the web view drew.
- **Stability across conversations.** Entry IDs are unique within one stream,
  per RFD 097.
- **Rewriting addresses when content changes.** An address resolves or it does
  not.

## Risks and Open Questions

- **The grammar is proposed, not settled.** Everything else here is decided; the
  syntax is the part that most needs review.
- **A same-entry offset span has no short form.** `#e@/p:o=10..e@/p:o=20`
  repeats the entry and pointer.
  A shorthand such as `o=10..20` would help, and is not specified.
- **A lone offset anchor names a zero-width point.** Whether that is useful, or
  should be an error outside a range, is open.
- **JSON Pointer is proposed for the path.** It is standard and already
  understood, but it is one more syntax inside the fragment.
- **The raw-entry view has no shape yet.** RFD 097 deferred it; this RFD claims
  it but the accessor is designed in implementation.

## Implementation Plan

### Phase 1: the address type

The address, its endpoints and anchors, in `jp_conversation`, with serde and the
encode and decode helpers.
Grapheme-boundary rejection at construction.
Independent, mergeable on its own.

### Phase 2: parsing and formatting

Fragment grammar to and from the address type, with base64url on the text
fields.
Tests: round-trip; quotes containing `#`, `&` and newlines survive; malformed
fragments are rejected with a useful message.
Depends on Phase 1.

### Phase 3: resolution

The resolution ladder against a loaded stream, including the raw-entry view over
all stream entries, JSON Pointer lookup, and the two range rules.
Tests: each line of the ladder, including that a repeated quote with no offset
is unresolved and that a reordered range is unresolved.
Depends on Phase 2.

### Phase 4: `jp://` integration

`jp_attachment_internal` reads the fragment instead of discarding it, and
intersects it with `select=`, erroring when the filter excludes an endpoint.
Replaces `uri_to_entry_ignores_path_and_fragment`.
Depends on Phase 3.

### Phase 5: documentation

Add a character-offset section to `docs/architecture/indexing-conventions.md`,
which currently covers turn positions only, recording that offsets are 0-based
boundaries in code points and why that differs from `grep --json`.
Update the `jp_attachment_internal` README.
Depends on Phase 4.

## References

- [RFD 097], stable entry identifiers, which this RFD extends
- [RFD D52], the first consumer of these addresses
- [WADM], the Web Annotation Data Model, source of the selector vocabulary
- [RFC 6901], JSON Pointer
- `docs/architecture/indexing-conventions.md`, turn positions and counts

[RFD 097]: ../097-stable-event-identifiers.md
[RFD D52]: D52-event-annotations.md
[RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901
[WADM]: https://www.w3.org/TR/annotation-model/
