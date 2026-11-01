# Make an RFD's category a label, and share one header layout

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-01
- **Label**: client=web
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: relevant-rfd=127
- **Label**: type=enhancement

RFDs are the oldest kind of document the site shows, and two things set them
apart from tickets and wiki pages.

## Category becomes a label

An RFD's `Category` is a fixed list (`design`, `decision`, `guide`, `process`)
with its own filter buttons on the RFD index.
Tickets express the same thing as a `type` label with exactly one value, and the
wiki dropped its categories for the page tree (RFD 127).

Turn `- **Category**: Design` into `- **Label**: type=design`, with a
`plugins.command.rfd.options.labels` vocabulary that requires exactly one
`type`.
That gives RFDs:

- other labels, such as `domain=…`, under the same vocabulary rules tickets and
  wiki pages follow;
- labels rendered by `DocLabels` and filtered with `?label=`, like the other two
  kinds, where today they render as plain text;
- an RFD index that filters by label the way the ticket and wiki tables do,
  replacing the category buttons.

Costs:

- `jp rfd draft` picks its template by category; it reads the `type` label
  instead.
- Every RFD file's `Category` line becomes a `Label` line.
  That is a format change to RFD 001, so it is an amendment to RFD 001 and RFD
  116, not only code.
- `jp rfd tools sync` mirrors the categories into the tool schemas; it would
  mirror the label vocabulary, as `jp ticket tools sync` does.
- The site settings snapshot carries `rfd.categories`; it would carry the RFD
  label vocabulary.

## One header layout

The site renders the metadata block three ways: a wiki page puts its status as a
letter after the title and the rest at the end of the page; RFDs and tickets
keep a status, category, date, and authors line under the title.
Once category is a label, one layout rule can cover all three.
The status badge carries more weight on an RFD than on a wiki page, so the rule
may keep it visible under the title for RFDs and tickets; the decision is which
fields each kind shows where, made once in `meta-header.mjs` rather than per
kind.

## Done when

- No RFD file carries `Category`; each carries exactly one `type` label.
- `jp rfd draft`, `jp rfd list`, and `jp rfd tools sync` work from the label.
- The RFD index filters by label; its category buttons are gone.
- RFD labels render through `DocLabels` and lead somewhere.
- RFD 001 and RFD 116 describe the new format.
