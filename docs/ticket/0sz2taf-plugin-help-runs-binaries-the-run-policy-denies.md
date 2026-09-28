# Plugin help runs binaries the run policy denies

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-28
- **Implements**: 072
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=bug

`jp -h` and `jp <plugin> -h` execute plugin binaries without checking
`plugins.command.<name>.run`, a pinned checksum, or `$PATH` approval.

## Why

- `print_plugin_help_section` (`crates/jp_cli/src/lib.rs`) calls
  `describe_plugin` on every `jp-*` binary `discover_plugins` finds.
- `describe_plugin` (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) spawns the
  binary directly.
- `run_external` answers a bare `jp <plugin> --help` through `show_plugin_help`
  before `resolve_plugin_binary` runs, where the policy, checksum, and approval
  checks live.

RFD 077 says the `run` policy applies at every execution point, and RFD 072 now
states that admission precedes every spawn, `describe` included.

## Scope

Any `jp-*` executable on `$PATH` plus `jp -h`.
A binary set to `run = "deny"`, or an unapproved `ask` binary, executes, and the
user sees only a help listing: it can do anything before it reads its first
protocol message.

## Fix

- Root help: describe only admitted plugins; list any other by its file name
  without running it.
  No prompts from root help.
- `jp <plugin> -h`: go through `resolve_plugin_binary` first, prompting where
  the policy is `ask`.

## Verifying

- A `jp-*` test binary that writes a marker file when run, with `run = "deny"`:
  `jp -h` lists it and the marker is absent.
- The same binary under `ask` with no approval: `jp <plugin> -h`
  non-interactively fails with the approval error and the marker is absent.
