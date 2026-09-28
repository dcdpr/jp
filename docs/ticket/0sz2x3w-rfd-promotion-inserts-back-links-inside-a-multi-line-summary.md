# RFD promotion inserts back-links inside a multi-line Summary field

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-28
- **Label**: domain=tooling
- **Label**: type=bug

Promoting RFD 114 added `Extended by` to RFD 072 and `Required by` to RFD 113 in
the middle of their metadata `Summary` field, which spans two lines:

```markdown
- **Summary**: Standalone command plugins communicate with JP via JSON-lines
- **Extended by**: [RFD 114](114-plugin-workspace-scope-and-addressing.md)
  protocol to extend subcommands across languages.
```

## Why it matters

- The Summary is split: the index and priority board show half a sentence, and
  the second half reads as a continuation of the back-link.
- `Summary` is no longer the last field, which RFD 001 requires.
- The link is written inline instead of in the reference style every RFD uses,
  with its definition at the bottom.

## Fix

The back-link writer (used by `rfd-promote`, and likely by `rfd-extend` and
`rfd-require`) has to treat a metadata field as the field line plus its indented
continuation lines, insert new fields before `Summary`, and write a
reference-style link with a matching definition.

Both headers were corrected by hand.

## Verifying

A fixture RFD whose `Summary` wraps onto a second line gains an `Extended by`
through promotion: the field lands above `Summary`, `Summary` is intact, and the
link definition is appended to the reference list.
