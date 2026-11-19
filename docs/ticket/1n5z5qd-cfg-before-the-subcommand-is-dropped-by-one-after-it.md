# --cfg before the subcommand is dropped by one after it

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

A `--cfg` given before the subcommand is silently discarded when another `--cfg`
(or `-c`) follows the subcommand:

```sh
jp --cfg='conversation.tools.git_status.style.background=196' query -cdev 'Call git_status.'
```

runs with only `dev` applied.
Nothing reports that the first value was ignored.

## Cause

`--cfg` is a clap `global` argument with `ArgAction::Append`.
Clap propagates a global argument's values from the deepest command that set it
to every level, replacing rather than appending, so the values given before the
subcommand are overwritten by the ones given after it.
`Globals::from_matches` reads the propagated list and sees only the latter.

`-v` (`ArgAction::Count`) has the same cause: `jp -v query -v` counts one, not
two.

## Fix

Declare the accumulating global options on every declared command rather than
through clap's propagation, so each level keeps its own values, and read them
back by walking the matched subcommands from the root down, which is the order
they appear on the command line.

- [x] `--cfg` values on both sides of the subcommand are all applied, in
      command-line order
- [x] `-v` counts across both sides of the subcommand
- [x] Both still appear under Global Options in every command's help
