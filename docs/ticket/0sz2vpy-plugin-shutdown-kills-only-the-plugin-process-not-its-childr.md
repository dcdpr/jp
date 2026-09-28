# Plugin shutdown kills only the plugin process, not its children

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-28
- **Implements**: 072
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=bug

When a plugin does not exit within the shutdown grace period, JP kills the
process it spawned and nothing else.
A subprocess the plugin started survives, keeps any port it bound, and can keep
JP from exiting.

## Why

- The child is spawned with `process_group(0)`, so its subprocesses share its
  group and are also shielded from the terminal's Ctrl-C.
- `kill_child` (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) sends `SIGKILL` to
  the positive PID on Unix, and `TerminateProcess` to that one process on
  Windows.
- Cleanup then joins the stderr reader thread, which blocks for as long as a
  surviving descendant holds the inherited stderr pipe open.

## Scope

A shell-script plugin that runs a worker in the foreground (a server, a watcher)
and then Ctrl-C.
The worker outlives `jp`, and `jp` itself can hang on exit.

## Fix

- Unix: kill the group, `kill(-pid, SIGKILL)`; the child already leads it.
- Windows: assign the child to a Job Object at spawn and terminate the job.
- Bound the stderr join once the group is gone.

RFD 072 states the guarantee: JP owns the plugin's process tree for the
invocation, and deliberately detached descendants are unsupported.

## Verifying

A shell plugin that starts `sleep 600 &`-style foreground worker and ignores
`shutdown`: after the grace period, assert the worker's PID no longer exists and
`jp` has exited.
