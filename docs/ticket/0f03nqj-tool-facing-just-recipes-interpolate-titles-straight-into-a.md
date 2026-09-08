# Tool-facing just recipes interpolate titles straight into a shell script

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-07
- **Label**: domain=tooling
- **Label**: type=bug

`just rfd-draft`, `just rfd-rename`, and `just ticket-add` all take a free-text
title and reach it through `{{TITLE}}` inside a `#!/usr/bin/env sh` recipe body.
Just substitutes the text before the shell parses the script, so a title
carrying `$(...)`, a backtick, or a `"` is read as shell syntax rather than as a
title.

`rfd-draft` and `rfd-rename` are the commands behind the `rfd_draft` and
`rfd_rename` tools, so the title on those calls is model-generated, repeating
text that came from the conversation.
`ticket-add` is human-facing: the `ticket_create` tool goes through `just
serve-tools` and never touches this recipe.

Nothing here is exploitable by accident: a title with a stray backtick fails the
recipe rather than doing something quiet.
The concern is a title that was crafted upstream, in an issue body or a pasted
diff, and then handed to `rfd_draft` verbatim.

The fix is to stop interpolating into the script and pass the arguments through
`argv` instead: mark the recipes `[positional-arguments]` and read `$1`, `$2`,
... rather than `{{NNN}}` and `{{TITLE}}`.
`_shape-args` in the justfile already does this, so the pattern is in the tree.

Worth auditing every recipe that takes free text at the same time, not just
these three.
