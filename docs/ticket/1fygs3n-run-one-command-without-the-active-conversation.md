# Run one command without the active conversation

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: relevant-rfd=112
- **Label**: type=feature

There is no target that runs a single command without the session's active
conversation while leaving the session's active conversation intact.

This matters for commands that can run without a conversation.
`jp decide` (proposed in RFD 135) targets the active conversation by default and
runs standalone when none is active.
Passing `-!` (`--no-persist`) suppresses persistence, but still loads the active
conversation's configuration (including its decision model).
Without a target keyword, the only per-invocation opt-out is `-c
decision.conversation=none`, which is specific to `jp decide`.

Proposal: add a `none` conversation target representing the absence of a
conversation:

- `--id=none` (`-i none`) runs a single command without a conversation.
- `jp c use none` clears the session's active conversation for subsequent
  commands.

RFD 112 (Discussion) originally proposed `jp c use --clear` and rejected a
`none` keyword on the grounds that a target slot should name a target.
Once `--id=none` is needed for `-i`, accepting `none` in `jp c use none` uses
the same keyword across both positions.
RFD 112 is updated accordingly: `jp c use --clear` becomes `jp c use none`, and
the rejected "`none` or `NONE` keyword" alternative becomes the design.

Open questions:

- **Workspace axis.** RFD 112 also proposed `jp w use --clear`; replacing it
  with `jp w use none` keeps both axes consistent, and `-w none` runs a single
  command without a workspace.
- **Commands that require a conversation.** Only commands that can run without a
  conversation (such as `jp decide`) accept `--id=none`; commands that require a
  conversation (such as `jp query`) reject `none`.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T13:18:23Z

Incorporated into RFD 112: `none` is a target on both the workspace and
conversation axes.
`jp w use none` and `jp c use none` replace `--clear`, and `-w none` and
`--id=none` run a single command without a workspace or conversation.
RFD 135 uses `-i none` to run a single `jp decide` invocation without the active
conversation.
Implementation will be tracked under RFD 112's tracking ticket once accepted, so
this ticket is closed.
