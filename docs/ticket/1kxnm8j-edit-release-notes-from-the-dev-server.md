# Edit release notes from the dev server

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-16
- **Label**: client=web
- **Label**: domain=tooling
- **Label**: relevant-rfd=116
- **Label**: relevant-rfd=122
- **Label**: type=feature

Preparing a release's notes means reviewing them on the site: editing an entry's
wording, ordering the highlights and entries, fixing credits, and dropping
entries that don't belong.
Under `jp vite serve`, the release notes pages should allow all of that in
place, the way wiki pages can be edited from the dev server.

Writes go through `jp release`, as the wiki editor's go through `jp wiki edit`,
so `jp release` keeps the only implementation of the fragment format, and checks
each change before writing it.
The editing parts of the page (inline text editing, reordering a list) are
generic, for RFD, ticket, and wiki pages to use later; this ticket only wires
them to release notes.

- [ ] `jp release` commands that replace an entry's text, add or remove one of
      its credits, set a release's order, and delete an entry, each checked
      before it writes.
- [ ] A dev server endpoint that runs them, refusing writes the way the wiki
      endpoint does.
- [ ] The rendered page knows which fragment each entry and highlight came from,
      so the editor can address it.
- [ ] Inline editing of an entry's text.
- [ ] Drag-and-drop ordering of highlights and entries, written to the release's
      `.order.json`.
- [ ] Adding and removing credits; removing an entry's last credit is refused,
      since the entry would then be credited to the commit that added it.
- [ ] Deleting an entry.
- [ ] The inline editing and list ordering usable by other page kinds.

The release notes pages and `jp release` exist only on `release-2` until the
release backfill's pull request merges: the generic parts can branch off `main`,
and the release notes part off `release-2`.
