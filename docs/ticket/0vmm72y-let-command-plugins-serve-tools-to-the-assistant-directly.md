# Let command plugins serve tools to the assistant directly

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=mcp
- **Label**: domain=plugins
- **Label**: package=jp_mcp
- **Label**: package=jp_plugin
- **Label**: type=pre-rfd

A command plugin that wants to offer tools has to be wrapped as a local tool
whose command starts a second `jp`: `command = "jp ticket tool {{context}}
{{tool}}"`.
Tool sources are `local`, `mcp`, and `builtin`
(`crates/jp_mcp/src/server.rs:738-748`); there is no plugin source.

The wrapper costs RFD 116 four workarounds:

- **Configuration is lost.** The nested `jp` resolves configuration from the
  workspace root, so the query's `--cfg`, its conversation config, and
  `.jp.toml` files below the root never reach the tool (T-0vm44ks).
- **Definitions are hand-maintained.** Each project writes TOML for every tool,
  so the plugins ship `tools init PATH` to seed it and `tools sync PATH` to
  mirror label enums and categories into it.
- **Trust is configured twice.** The plugin needs `run = "unattended"`, or the
  nested `jp` refuses for lack of a terminal
  (`crates/jp_cli/src/cmd/plugin/dispatch.rs:2447-2460`).
- **Every call starts a process** that loads the whole workspace.

## Direction

A tool source that names a command plugin, e.g. `source = "plugin.ticket"`.
The host asks the plugin for its tools and their schemas, runs calls over the
plugin protocol, and hands the plugin the query's own resolved configuration.
That removes all four workarounds at once and subsumes T-0vm44ks.

Draft D06 (Self-Describing Local Tools) covers the schema half for local tool
binaries.
This is the larger version: the plugin also lists which tools exist, and the
call runs inside the host's protocol rather than as a new process.

## Open questions

- Is the plugin spawned per call or kept alive for the query?
- How do per-tool `options` and a project's renames
  (`[conversation.tools.track]` with `source = "plugin.ticket.ticket_create"`)
  map onto it?
- Does the plugin's `run` policy replace the tool's own `run`, or stack with it?
