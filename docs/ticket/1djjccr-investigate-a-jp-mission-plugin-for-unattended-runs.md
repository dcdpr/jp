# Investigate a jp-mission plugin for unattended runs

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-04
- **Label**: domain=plugins
- **Label**: domain=tooling
- **Label**: type=pre-rfd

The loop harness from T-0yh5hqj (`.config/loop/`) runs `jp` unattended on behalf
of a mission, but it lives in this repository as a bash script.
The same idea is useful in other JP workspaces.
Investigate shipping it as a command plugin, alongside `jp-ticket`, `jp-rfd`,
`jp-wiki`, `jp-vite` and `jp-serve-web`.

An illustrative workflow:

```sh
cd /path/to/workspace
jp mission init
jp mission new <name>      # interactive, manual, or assistant-generated
jp mission run <name>
```

Start this after the bash harness has run for real with bug-hunt and the
rustqual mission (T-0yh5sy9), so the design follows what those runs needed.
The outcome is an RFD draft.

## Questions for the RFD

- **Mission format.** Sourced bash, as the harness uses today, or declarative
  TOML (settings, prompt templates) with executables for `gate` and
  `next-target`.
  A declarative format is easier for `jp mission new` to generate and validate.
- **State location.** The harness keeps state in the mission's own `state/`
  directory, committed by a maintainer.
  Other workspaces may want it user-local or ignored instead.
- **Session and conversation handling.** The harness sets `JP_SESSION` per run
  so its conversations don't take over the terminal's session.
  A plugin can do this through JP directly instead of an environment variable.
- **Relation to existing RFDs.** RFD 051 (sub-agent workflows), RFD 107
  (background task infrastructure), and RFD 019 and 049 (non-interactive mode)
  cover nearby ground.
  Decide whether a mission builds on them or sits beside them.
- **What carries over from bash.** Rate-limit probing, stall detection and
  rotation exist because of how `jp` reports failures to a shell script.
  Some may be better solved inside JP than reimplemented in the plugin.
