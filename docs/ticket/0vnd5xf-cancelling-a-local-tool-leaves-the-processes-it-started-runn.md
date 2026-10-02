# Cancelling a local tool leaves the processes it started running

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_mcp

A cancelled local tool call kills only the process JP spawned.
`run_tool_command` puts the child in its own process group
(`crates/jp_mcp/src/server.rs`, `cmd.process_group(0)`), then relies on
`kill_on_drop(true)`, which signals the child's pid, not the group.
A tool command that is a shell or a `just` recipe (every tool under
`.jp/mcp/tools/` runs `just serve-tools ...`) has its real work in a grandchild,
which survives the cancel and keeps running, writing files, or holding a port.

Tools served by command plugins (`source = "plugin.command.<plugin>"`,
T-0vm44ks) are spawned the same way in `crates/jp_cli/src/cmd/plugin/tool.rs`
and have the same gap.

`jp <plugin>` commands do not: on the plugin-routing branch (PR #1216) they are
stopped through `ProcessTree`, which kills the process group on Unix and the job
object on Windows.

Input that hits it: a local tool whose command is `sh -c 'sleep 600'`, cancelled
with Ctrl+C mid-call.
The `sh` exits; the `sleep` is still running afterwards.

Fix: kill the tool's whole tree on cancel, for every subprocess-backed source,
reusing `ProcessTree` once PR #1216 lands.
Test with a command that starts a background worker and writes its pid, cancel
the call, and assert the worker is gone, as
`process_tree_tests::terminating_the_tree_kills_what_the_plugin_started` does.

## Comments

-----

- **From**: jp
- **Date**: 2026-10-01T15:15:06Z

#1225 merged `jp_process` with group-aware stopping: a process spawned with
`own_process_group` is stopped by signalling `-pid` (SIGINT, then SIGKILL once
`Watch::grace` runs out), even after the leader itself has exited.
Command plugin tool calls (T-0vm44ks) run through `ProcessRunner` with
`own_process_group = true`, so on Unix they are covered once that branch is on
main's `jp_process`.
Local tools are covered when `run_tool_command` moves onto `ProcessRunner`
(#1228 in the #1175 split).
Windows still kills only the child: `jp_process` has no job object.
