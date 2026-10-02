# Pass a plugin's effective options to the local tools it serves

- **Status**: Done
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-01
- **Label**: domain=plugins
- **Label**: package=jp_mcp

A local tool whose command runs a command plugin (`jp ticket tool {{context}}
{{tool}}`) gets a child `jp` that resolves configuration on its own, from the
workspace root.
The query's own configuration does not reach it.

The tool context carries the per-tool options and invocation ids, not
`plugins.command.*` (`crates/jp_mcp/src/server.rs:53-67`).
The command starts in the workspace root (`server.rs:497`).
External commands load no conversation config
(`crates/jp_cli/src/cmd.rs:104-115`).

Input that hits it: a monorepo where `packages/foo/.jp.toml` sets
`plugins.command.ticket.options.dir`.
A query run from `packages/foo` sees that setting; the `ticket_create` call it
makes writes to the ticket directory configured at the root.
Query `--cfg` overrides and conversation config are lost the same way.

The tool result names the path it wrote, so the mistake is visible, but the file
stays where it landed.

RFD 116 documents this limit for v1.
The fix is the host passing the effective options of the plugin a tool runs
through, so the child uses them instead of resolving its own.
That needs a decision on how a tool declares which plugin it runs through.

## Comments

-----

- **From**: jp
- **Date**: 2026-10-02T14:26:08Z

Fixed on the host side: a tool declares the plugin it runs through with `source
= "command.<plugin>[.<tool>]"`, and JP runs the plugin itself with the turn's
resolved `plugins.command.<plugin>.options` in `init.options`.
No child `jp` resolves config any more, so nested `.jp.toml` files, conversation
config, and `--cfg` all reach the plugin.

The ticket tools still need to adopt it on the RFD 116 branch:

- `jp-ticket` answers an `init` carrying `tool`: run `tool.name` with
  `tool.arguments`, reply with `tool_outcome`, then `exit 0`.
  Stdin is closed after `init`, so the tool path must not `compose` or
  `read_config`.
- On `tool.action == "format_arguments"`, return the preview as a `success`
  outcome and write nothing.
- Read the ticket directory from `init.options["dir"]` when `--dir` is absent.
- Enforce `tool.access` when present.
- `REQUIRED_PROTOCOL = 12`.
- `.jp/mcp/tools/ticket/*.toml`: `source = "command.ticket.<tool>"`, drop
  `command`, and `style.parameters = "tool"`.
- Admit the binary: `plugins.command.ticket.run = "allow"` in `.jp/config.toml`,
  or `jp plugin approve`.
  Otherwise the turn drops the ticket tools and says why.
