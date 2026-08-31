# `jp config fmt --all` is accepted and then ignored

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-27

`jp config fmt --all` parses, exits zero, and does exactly what `jp config fmt`
does.

The flag is declared on `Fmt` (`crates/jp_cli/src/cmd/config/fmt.rs:17-19`) and
never read.
`Fmt::run` branches on one thing: whether `self.target` differs from
`Target::default()`.
With no target flag it formats all four targets (workspace, user-workspace,
user-global, cwd); with one it formats that one.
`self.all` appears nowhere in the body.

## Options

1. **Remove the flag.** A bare `jp config fmt` already formats every
   configuration file that applies to the workspace, which is what `--all`
   promises.
   Subtractive, no behavior change.
2. **Make the default narrow and `--all` broad.** Format only the workspace file
   by default, matching `jp config set`, and require `--all` for the other
   three.
   This is what the flag's name implies, and it would make `fmt`'s default
   consistent with `set`'s.

Option 1 is the safer call.
Option 2 silently shrinks what an existing `jp config fmt --check` in CI
verifies, and the broad default is the more useful one for a formatter.

## Context

Found while rewriting `docs/configuration.md` against the code.
The flag was never documented, so nothing user-facing depends on it yet.
That makes this the cheapest moment to delete it.
