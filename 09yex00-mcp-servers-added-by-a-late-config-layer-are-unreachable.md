# MCP servers added by a late config layer are unreachable

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-28

`Ctx::new` builds the MCP client once, from the config it is handed:

```rust
let mcp_client = jp_mcp::Client::new(config.providers.mcp.clone());
```

`jp_mcp::Client` keeps that map for its lifetime, and `Client::get_tool` returns
`Error::UnknownServer` for anything absent from it. Tool *definitions*, by
contrast, are resolved per query against whatever config that query was layered
with: `jp_cli::cmd::query` passes `cfg.conversation.tools` alongside
`ctx.mcp_client`, and those two come from different snapshots.

They agree for a one-shot CLI invocation, because `lib.rs` runs `resolve_config`
— conversation layer and `--cfg` included — before `Ctx::new`. They diverge for
any host that builds a `Ctx` once and then serves turns whose config is
re-layered per turn. A `[providers.mcp.*]` entry introduced by that later layer
never reaches the client, while the tools that name it do.

## What it looks like

A tool declared `source = "mcp.<server>.<tool>"` fails the turn with
`Unknown MCP server: <server>`, while the tool config itself resolves fine. It
reads as a typo in the tool declaration rather than as a client built from an
older config.

`optional = true` does not soften it. That flag covers a server that is known
and fails to *start*; here the server is not known at all, so the fail-soft path
in `tool_definitions` — which drops tools whose backing server is not running —
is never reached.

## Affected sites

- `crates/jp_cli/src/ctx.rs` — `Ctx::new` builds the client from its own
  config and holds it for the lifetime of the `Ctx`.
- `crates/jp_mcp/src/client.rs` — `Client::get_tool` and `run_services` look
  the server up in the map fixed at construction.
- `crates/jp_cli/src/cmd/query.rs` — `tool_definitions` is handed the
  per-query config and the `Ctx`-lifetime client together.

## Suggested resolution

Either rebuild the server map when the config a turn runs with differs from the
one the `Ctx` was built with, or make the mismatch legible: a server configured
for this turn but absent from the client is a different failure from a server
nobody configured, and the message should say which it is.

Reproducing it needs a long-running host rather than a CLI invocation, so a test
wants a `Ctx` built from one config and a turn resolved from another.

## Related

D36 (Live Workspace View for Long-Running Plugin Hosts) is the same class of
staleness, on the workspace rather than on config.
