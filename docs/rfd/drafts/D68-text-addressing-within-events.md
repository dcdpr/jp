<!--
  This template is a starting point, not a constraint. Delete sections that
  don't apply, add sections that do, or restructure entirely. The only
  requirement is the metadata header (Status, Authors, Date).

  Use HTML comments like this one for draft-time notes and review markers.
  They do not appear in the rendered output and can be removed when the RFD
  advances to Discussion status.
-->

# RFD D68: Text Addressing Within Events

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-10
- **Extends**: [RFD 097]
- **Required by**: [RFD D52]
- **Summary**: Extends `jp://` URLs with quote- and position-anchored fragments
  that address stream entries, text spans, and ranges.

## Summary

A `jp://` URL identifies a conversation.
This RFD adds a URL fragment that addresses a location within that conversation:
a stream entry, a JSON field or text span inside an entry, or a range between
two endpoints.
Text locations are anchored by a quote with an optional character offset to
break ties between repeated matches, so an address either resolves to a single
location or fails explicitly.

## Terminology

- **Address**: A reference to a location in a conversation: a stream entry, a
  field or text span within an entry, or a range between two endpoints.
- **Text quote selector**: A selector that matches a substring by its exact text
  and optional surrounding prefix and suffix context.
- **Text position selector**: A selector that identifies a point in a string by
  its Unicode code-point offset from the start of the string.
- **Fragment**: The component of a URI following `#`, which identifies a
  location within the resource named by the URI.
- **Grapheme cluster**: A user-perceived character, which may consist of one or
  more Unicode code points.
- **Event ID**: The stable per-entry identifier introduced by [RFD 097].
- **`jp://` scheme**: The URI scheme for internal JP resources
  (`jp://<jp-id>[?<query>]`), resolved by `jp_attachment_internal`.

The selector terms come from the Web Annotation Data Model ([WADM]), but this
RFD defines stricter resolution rules where WADM leaves behavior to the
implementation.

## Motivation

`jp://jp-c17013123456?select=a:-1` is the most specific reference JP supports: a
conversation filtered by role and turn.
There is no way to address a passage within an event, a single stream entry such
as a compaction overlay, or a whole turn by its `TurnStart` entry.

Several features need finer-grained addressing:

- **Event annotations ([RFD D52])**: An annotation on a specific claim in a
  response needs a target span to attach to.
- **Cross-conversation quotes**: Referencing a passage from another conversation
  currently requires attaching the entire message.
- **Deep links**: A link into a rendered conversation cannot target a specific
  entry or text span.

[RFD 097] provides the first half of the mechanism by assigning a stable ID to
every stream entry.
The remaining problem is addressing a location inside an entry in a way that
survives manual edits to the conversation file.
Character offsets alone are too fragile: inserting a word near the start of a
message shifts every subsequent offset, silently pointing existing references at
the wrong text.

## Design

### URL Format

An address is a `jp://` URL with a fragment:

```
jp://jp-c17013123456#k3m9x2a@/content:t=Y2hva2Vz
```

The URI before `#` identifies the conversation, and the fragment identifies the
target location within it.
This follows RFC 3986, which assigns resource identification to the path and
query components and sub-resource identification to the fragment.
`jp_attachment_internal` currently ignores the fragment (tested by
`uri_to_entry_ignores_path_and_fragment`), so the component is unused.

### Fragment Grammar

This grammar is a starting proposal for review:

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

Each component after `entry-id` is optional:

- `entry-id` addresses the entire stream entry.
- `entry-id@pointer` addresses an entire JSON field within the entry.
- `entry-id@pointer:anchor` addresses a text span or character offset within
  that field.

Entry-level addressing covers entries that contain no text, such as `TurnStart`
(which also addresses an entire turn), and references to a whole tool call
without selecting a single field.
Non-event stream entries use the same syntax: `/summary/summary` addresses a
compaction's generated summary prose, and pointers into a config delta address
its string values.

None of the grammar delimiters (`@`, `:`, `;`, `=`, `.`) appear in the base64url
alphabet, so encoded quote strings cannot collide with structural delimiters.
The `..` range operator matches the inclusive range syntax used in `--turn
A..B`, the compaction DSL, and timeline output.

### JSON Representation

An address has two lossless representations: a URL fragment for transport and a
JSON object for storage.

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

Inside `quote`, optional `prefix` and `suffix` fields appear alongside `exact`.
`quote` and `offset` are mutually exclusive.

A single-endpoint address is serialized as a flat object with an `entry` key,
whereas a range wraps two endpoint objects under `start` and `end`.
Using distinct top-level shapes distinguishes a single location from a range
without representing a single location as a degenerate `start`-only range.

Quote strings in JSON are stored as plain UTF-8 rather than base64url.
Base64url encoding is only needed to prevent delimiter collisions in URL
fragments; keeping stored JSON unencoded leaves quotes readable in tools like
`jq`.

### Field Pointers and Stream Entries

The `pointer` component is a JSON Pointer ([RFC 6901]) evaluated against the
entry's decoded JSON representation.

Any string field in an entry can be addressed.
The addressing model defines no allowlist of event kinds and no per-kind default
field: a chat request's `content`, a tool call's `arguments`, a tool response's
`content`, and thinking signatures inside `metadata` are all selected by their
JSON Pointer path.
Adding a new event kind requires no changes to the addressing rules.

Pointers and offsets operate on decoded JSON values rather than the on-disk wire
format.
In `jp_conversation::storage`, tool `arguments`, tool response `content`, and
`metadata` are base64-encoded at rest; those fields are decoded before the JSON
Pointer is resolved.

Addresses can target any stream entry, not only conversation events.
[RFD 097] assigns stable IDs to config deltas, compactions, and unrecognized
entries, but deferred programmatic access "until a consumer RFD defines the
raw-entry view it needs."
This RFD introduces that raw-entry view so resolvers can inspect every entry in
the stream.

### Anchors

An anchor locates text within a string field using either a quote or an offset.

A **quote** specifies the `exact` target substring and optional `prefix` and
`suffix` context immediately surrounding it.
`prefix` and `suffix` are always optional: surrounding context makes a match
more likely to be unique, but cannot guarantee uniqueness in repeated text.
Because a quote acts as a substring predicate, it can match zero, one, or
multiple locations in the target string.

An **offset** is a 0-based index measured in Unicode code points that identifies
a boundary between characters (offset `0` precedes the first code point).
A single integer suffices because an offset anchor identifies a point rather
than a span.

Counting Unicode code points differs from `jp conversation grep --json`, whose
`submatches` report byte offsets to match `rg --json`.
Byte offsets work for ephemeral indexing within a single command's output, but
stored addresses may be written by hand and must not land inside a multi-byte
UTF-8 sequence.
The library provides conversion helpers so callers chaining `grep` output into
annotations do not compute code-point offsets manually.

Address constructors reject quotes or offsets that split a Unicode grapheme
cluster, guaranteeing that every valid address aligns to grapheme boundaries.

### Ranges

Two endpoints separated by `..` define an inclusive range running from the start
of the first endpoint to the end of the second.
The two endpoints can use different anchor types (for example, an offset at the
start and a quote at the end) and may target either the same stream entry or
different entries.

A range addresses a slice of the conversation stream, not a single string.
Because config deltas, compactions, and multiple events may appear between the
two endpoints, resolving a range yields an ordered sequence of entries whose
boundary fields may be partially sliced.

Intermediate entries are selected by their position in the stream array.
Because intermediate entries are not content-anchored and [RFD 097] forbids
deriving order from entry IDs, manually reordering entries in the stream file
changes which entries fall inside a cross-entry range.
To surface broken ranges rather than silently returning corrupt slices, a range
fails to resolve if either endpoint fails to resolve or if the end entry
precedes the start entry in stream order.

### Resolution

An entry-only address resolves if the entry exists in the stream.
A field address without an anchor resolves if the JSON Pointer selects a string
value in the decoded entry.

When an anchor is present, the quote is authoritative and the offset serves only
as a tie-breaker:

```
1. Match the quote against the addressed string.  -> zero, one, or many
2. Exactly one candidate                          -> resolved
3. Several candidates, offset present             -> the candidate nearest it
4. Several candidates, no offset                  -> unresolved
5. No candidates                                  -> unresolved
6. Offset with no quote                           -> resolved at that point
```

Substring matching is byte-exact without Unicode normalization, so quotes and
code-point offsets share a single coordinate system over the decoded string.
Consequently, a quote copied from a differently normalized source will not match
visually identical text in the entry; consumers such as [RFD D52] avoid this by
extracting quotes directly from the stored entry.

An offset never overrides a non-matching quote.
Because any in-bounds offset resolves unconditionally, falling back to the
offset after an edit would silently alias unrelated text, violating [RFD 097]'s
requirement that broken references produce "a *detectable* mismatch, never a
silent positional aliasing."
Rule 6 (an offset with no quote) is an explicit opt-in to positional addressing
for ad-hoc queries, and is not intended for durable storage.

When used strictly to disambiguate multiple quote matches (Rule 3), an offset
tolerates minor edits: shifting surrounding text moves the offset slightly, but
the quote filter still restricts candidates to actual occurrences of the target
phrase, so the nearest candidate typically remains the intended match.

### Fragment Encoding

In URL fragments, quote strings (`exact`, `prefix`, and `suffix`) are encoded
individually using unpadded base64url
(`base64::engine::general_purpose::URL_SAFE_NO_PAD`).
Because the base64url alphabet (`A-Za-z0-9-_`) consists entirely of RFC 3986
unreserved characters, fragments never require percent-encoding, and quotes can
contain `#`, `&`, newlines, or arbitrary Unicode while leaving the `;p=` and
`;s=` delimiters unencoded.

This differs from `jp_conversation::storage`, which uses standard base64
(`STANDARD`).
Standard base64 includes `+`, `/`, and `=` padding, which would collide with the
key-value `=` delimiter in the fragment grammar.

Offsets are formatted as plain decimal integers, and base64url is never used in
persisted JSON files such as [RFD D52]'s `annotations.json`.

### Interaction with `select=`

The `select=` query parameter filters conversation attachments by role and turn.
When a `jp://` URL specifies both `select=` and a fragment, the two compose as
an intersection:

1. The fragment resolves to an ordered sequence of entries, with partial text
   spans on the boundary entries when anchored within a field.
2. If `select=` is present, it filters that sequence of entries.
3. If `select=` excludes an entry explicitly named by a fragment endpoint,
   resolution fails with a validation error distinct from an unresolved-address
   error (for example, `"select=a excludes k3m9x2a, where the range starts."`).

### Differences from the Web Annotation Data Model

[WADM] is an interchange standard for independent clients annotating third-party
documents, so many of its matching rules are advisory (`SHOULD` or
`RECOMMENDED`).
Because JP controls both storage and resolution, this RFD makes those behaviors
normative.
JP addresses form a strict subset of WADM selectors: any JP address can be
exported as valid WADM, but JP does not accept every selector shape WADM
permits.

| WADM                                                            | This RFD                                              |
| --------------------------------------------------------------- | ----------------------------------------------------- |
| Range runs to the *beginning* of the end selector, excluding it | Runs to the end of the end selector, inclusive        |
| Start and end selectors SHOULD be the same class                | Either endpoint may be a quote or an offset           |
| Multiple matches SHOULD select all of them                      | Selects the match nearest the offset, or fails        |
| Text MUST be normalized (markup stripped) before recording      | No normalization; matches decoded stored strings      |
| A `State` is RECOMMENDED alongside a position selector          | No `State`; the quote is the authoritative anchor     |
| Selections SHOULD NOT split a grapheme cluster                  | Grapheme-splitting addresses are rejected on creation |

## Drawbacks

- **Changes existing URI handling**: `jp_attachment_internal` currently ignores
  URI fragments (asserted by `uri_to_entry_ignores_path_and_fragment`).
  Parsing fragments as addresses is a behavioral change for any caller passing
  arbitrary fragments.
- **Cross-entry ranges depend on stream order**: Because intermediate entries in
  a range are not content-anchored, reordering entries in the stream file
  changes the contents of the range.
- **Two offset units across the CLI**: `jp conversation grep --json` reports
  byte offsets, whereas `jp://` fragments use Unicode code-point offsets.
- **Opaque URL fragments**: Base64url encoding prevents delimiter collisions at
  the cost of human readability in raw URLs.

## Alternatives

- **Character offsets only (no quotes)**: Offsets are compact, deterministic,
  and always resolve if in bounds.
  Rejected because any edit before the target offset silently shifts what the
  address points to without producing an error.
- **Quotes only (no offsets)**: Eliminates fragile positional addresses.
  Rejected because disambiguating repeated substrings without an offset requires
  expanding `prefix` and `suffix` (which still cannot guarantee uniqueness), and
  because short offset addresses remain useful for ephemeral queries.
- **Cryptographic hashes of the target span**: Detects any modification to the
  quoted text, but invalidates the address on any edit to the target span or its
  context, defeating manual conversation editing.
- **A separate URI scheme instead of a `jp://` fragment**: Rejected because
  `jp://` already identifies conversations and RFC 3986 designates the URI
  fragment for sub-resource addressing.

## Non-Goals

- **Semantic validation of addressed content**: Resolution locates the target
  entry, field, or substring without interpreting its meaning.
- **Byte-offset addressing (`DataPositionSelector` in [WADM])**: Offsets in this
  RFD count Unicode code points, not UTF-8 bytes.
- **Addressing rendered output**: Addresses target decoded JSON strings in the
  stored conversation stream, not terminal or web-view rendering.
- **Cross-conversation entry stability**: Per [RFD 097], entry IDs are scoped to
  a single conversation stream.
- **Automatic address migration on edit**: Stored addresses are not rewritten
  when conversation entries change; they either continue to resolve or fail.

## Risks and Open Questions

- **Fragment grammar syntax**: The data model and resolution rules are settled,
  but the concrete delimiter syntax in the ABNF grammar needs review.
- **No shorthand for same-field offset ranges**: Addressing a character span by
  offset within a single field requires repeating the entry ID and pointer
  (`#e@/p:o=10..e@/p:o=20`).
  A compact form such as `#e@/p:o=10..20` is not yet specified.
- **Single-point offset anchors**: An offset anchor outside a range identifies a
  zero-width boundary between characters.
  Whether a standalone offset should be valid or rejected outside a `..` range
  remains open.
- **JSON Pointer syntax inside fragments**: [RFC 6901] is a standard path
  format, but embedding `/` paths inside a URL fragment adds another delimiter
  convention.
- **Raw-entry API design**: [RFD 097] deferred the programmatic API for
  inspecting non-event stream entries; this RFD requires that accessor, but its
  Rust type signature is left to implementation.

## Implementation Plan

### Phase 1: Address Types

Define the address, endpoint, and anchor types in `jp_conversation`, including
`serde` serialization, grapheme-boundary validation at construction, and
byte-to-code-point offset conversion helpers.
This phase is self-contained and can merge independently.

### Phase 2: Fragment Parsing and Formatting

Implement conversion between the URL fragment grammar and the address types,
including unpadded base64url encoding and decoding for quote fields.
Add tests covering round-trip serialization, quotes containing `#`, `&`, and
newlines, and descriptive parse errors for malformed fragments.
Depends on Phase 1.

### Phase 3: Stream Resolution

Implement the resolution algorithm against a loaded conversation stream,
including the raw-entry view over all stream entries, JSON Pointer lookup on
decoded fields, and range boundary validation.
Add unit tests for each resolution rule, including ambiguous quotes without an
offset and ranges whose endpoints are out of order.
Depends on Phase 2.

### Phase 4: `jp://` Attachment Integration

Update `jp_attachment_internal` to parse and resolve URL fragments, intersect
resolved entries with `select=`, and return an error when `select=` excludes a
range endpoint.
Replace `uri_to_entry_ignores_path_and_fragment` with fragment resolution tests.
Depends on Phase 3.

### Phase 5: Documentation

Document 0-based Unicode code-point offsets and their distinction from `grep
--json` byte offsets in
`docs/wiki/00dxyba-indexing-and-counting-conventions.md`.
Update the `jp_attachment_internal` README with fragment addressing examples.
Depends on Phase 4.

## References

- [RFD 097]: Stable event identifiers, which this RFD extends.
- [RFD D52]: Event annotations, the first consumer of these addresses.
- [WADM]: The Web Annotation Data Model, which defines the selector vocabulary.
- [RFC 6901]: JSON Pointer.
- `docs/wiki/00dxyba-indexing-and-counting-conventions.md`: Indexing and
  counting conventions for turns and character offsets.

[RFC 6901]: https://www.rfc-editor.org/rfc/rfc6901
[RFD 097]: ../097-stable-event-identifiers.md
[RFD D52]: D67-event-annotations.md
[WADM]: https://www.w3.org/TR/annotation-model/
