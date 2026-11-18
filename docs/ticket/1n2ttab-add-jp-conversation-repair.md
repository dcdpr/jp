# Add jp conversation repair

- **Status**: Todo
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
