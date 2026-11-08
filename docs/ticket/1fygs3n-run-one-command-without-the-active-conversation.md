# Run one command without the active conversation

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: relevant-rfd=112
- **Label**: type=feature

Nothing runs a single command without the session's active conversation while
the session keeps it.

This matters more once commands can run without a conversation.
`jp decide`, proposed in the draft RFD *jp decide* (currently D74), targets the
active conversation by default, and runs without one when none is active.
`-!` stops it writing, but it still applies the conversation's config, including
its decision model.
The per-run way out in that draft is `-c decision.conversation=none`, which only
`jp decide` has.

Proposed: a `none` target, meaning no conversation.

- `--id=none` runs one command without a conversation.
- `jp c use none` drops the session's active conversation for every later
  command.

RFD 112 (Discussion) adds `jp c use --clear` for the second, and rejects a
`none` keyword on the grounds that a word in the target slot should name a
target.
Once `--id=none` exists, the target grammar has a word for no conversation, and
`jp c use none` is the same word in the same slot.
RFD 112 changes with this ticket: `jp c use --clear` becomes `jp c use none`,
and its rejected alternative "A `none` or `NONE` keyword instead of `--clear`"
is reversed.

Open questions:

- The workspace axis.
  RFD 112 gives `jp w use` a `--clear` too; `jp w use none` keeps the two axes
  alike, but `-w none` would then have to mean running without a workspace.
- Which commands accept `--id=none`: those that can run without a conversation,
  which is `jp decide` alone.
  `jp query` always has one, and under T-1fygs3m it starts a temporary one where
  it has none, so `--id=none` there would either mean a new temporary
  conversation or be refused.
