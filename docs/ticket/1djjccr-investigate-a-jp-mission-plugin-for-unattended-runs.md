# Investigate a jp-mission plugin for unattended runs

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-04
- **Label**: domain=plugins
- **Label**: domain=tooling
- **Label**: type=pre-rfd

The loop harness from T-0yh5hqj (`.config/loop/`) runs `jp` unattended for a
mission, but it lives in this repository as a bash script.
Other JP workspaces need the same workflow.
Investigate shipping it as a command plugin alongside `jp-ticket`, `jp-rfd`,
`jp-wiki`, `jp-vite`, and `jp-serve-web`.

Example workflow:

```sh
cd /path/to/workspace
jp mission init
jp mission new <name>      # interactive, manual, or assistant-generated
jp mission run <name>
```

Start this investigation after running the bash harness on `bug-hunt` and the
`rustqual` mission (T-0yh5sy9), so the design reflects actual usage.
The outcome is an RFD draft.

## Questions for the RFD

- **Mission format.** Whether missions are defined as sourced bash scripts (like
  `.config/loop/`) or as declarative TOML files (settings and prompt templates)
  paired with `gate` and `next-target` executables.
  A declarative format is easier for `jp mission new` to generate and validate.
- **State location.** The bash harness stores state in the mission's `state/`
  directory for a maintainer to commit.
  Other workspaces may prefer user-local or gitignored state.
- **Session and conversation handling.** The bash harness sets `JP_SESSION` per
  run so unattended turns do not change the caller terminal's active
  conversation.
  Determine how a command plugin should isolate its session directly.
- **Relation to existing RFDs.** RFD 051 (sub-agent workflows), RFD 107
  (background task infrastructure), and RFD 019 and RFD 049 (non-interactive
  mode) address related problems.
  Determine whether `jp-mission` builds on those designs or operates
  independently.
- **What belongs in core JP versus the plugin.** Rate-limit probing,
  stream-stall detection, and conversation rotation in `run.sh` work around CLI
  exit statuses and log output.
  Determine which of these belong in core `jp` rather than in the plugin.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-05T11:23:23Z

Gaps in the bash harness for the plugin to close, from the review of PR #1286:

- **Supervised child processes.** `run.sh` runs the gate, `next-target` and the
  capacity probe in the foreground with no timeout.
  A `cargo check` blocked on a build lock (for example one held by a `cargo` a
  killed turn left behind) keeps the run alive past its deadline, and a `kill`
  of the harness waits until the child exits.
  Run each as a supervised process, with a timeout capped at the remaining
  budget and process-group cleanup on interrupt.
- **Tests for the shutdown and discard paths.** A stub `jp` and a gate that
  blocks would cover interrupt handling, the exit status, and discarding an
  iteration.
  The bash harness has none.
