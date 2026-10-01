# A plugin's background workers outlive a plugin that exits on its own

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-01
- **Implements**: 072
- **Label**: client=cli
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: type=bug

When a plugin exits by itself (it sends `exit`, or it closes stdout), JP reaps
the process it spawned and does nothing about the rest of its process tree.
A worker the plugin started in the background keeps running after `jp` returns,
and keeps any port it bound.

Tree-wide termination only happens on the kill path: `stop_plugin` calls
`ProcessTree::kill` once the shutdown grace period runs out (T-0sz2vpy).
A plugin that exits cleanly never reaches that path.

## Why

- `run_plugin` (`crates/jp_cli/src/cmd/plugin/dispatch.rs`) ends with
  `child.wait()` and a bounded join on the stderr reader.
  Neither one touches the plugin's process group or job object.
- The bounded join keeps `jp` from hanging on a surviving descendant, so the
  leak is silent: nothing is logged unless the stderr join times out.

RFD 072 says JP owns the plugin's process tree for the lifetime of the
invocation, not only the process it spawned.
That guarantee holds today only for a plugin that has to be killed.

## Scope

A shell-script plugin that runs `server &`, does its work, sends `exit`, and
returns.
The server is still running after `jp` exits.

## Fix

Terminate the tree on every exit path in `run_plugin`, not only after the grace
period.

- Unix: `kill(-pgid, SIGKILL)` once the plugin has exited.
  Do it before the leader is reaped (for example, wait with `waitid(..., WEXITED
  | WNOWAIT)` first, then signal the group, then reap).
  A zombie leader keeps the group ID reserved, so the signal cannot reach an
  unrelated group that reused the PID.
- Windows: terminate the job at cleanup, or create it with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` so closing the handle does the same.
- With the tree gone, the stderr join only times out for a descendant that
  deliberately left the group, which RFD 072 leaves unsupported.

## Verifying

A shell plugin that starts `sleep 600 &`, prints the worker's PID, sends `exit`
with code 0, and exits.
After `run_plugin` returns, the worker's PID no longer exists.
