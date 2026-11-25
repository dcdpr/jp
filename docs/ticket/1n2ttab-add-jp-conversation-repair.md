# Add jp conversation repair

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=feature
- **Parent**: T-1n2tnwv

Inspect a conversation that fails to load and offer repair actions for its
state, applying an action only when the user selects one.
Examples:

- `events.json` present, `metadata.json` missing: rebuild metadata from the
  events and the directory name

- only a query draft: open it to send, or discard it

- corrupt JSON: show the parse error and location; offer to trash

- [ ] Accepts conversations that fail to load, resolved from the directory
      listing

- [ ] Lists the actions it can take before taking any

- [ ] Non-interactive mode requires the action to be named

## Comments

-----

- **From**: jp
- **Date**: 2026-11-23T15:28:49Z

Withdrawn.
`jp doctor` is a global command (installation, config, workspace,
conversations), so fixes belong at its level rather than in the `jp
conversation` namespace, which could never fix a workspace or config problem.
The one real action this ticket had, rebuilding a missing `metadata.json` from
the events and the directory name, becomes `jp doctor`'s first named fix (`jp
doctor --fix rebuild-metadata`) under T-1n2ttac.
The other examples are gone or out of scope: draft-only directories no longer
exist (the conversation is stored before its draft), trashing was removed, and
truncated JSON is not recovered (RFD 052 non-goal).
