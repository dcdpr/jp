# Upgrade rmcp to 3.x and drop the RUSTSEC-2026-0462 ignore

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-10
- **Label**: domain=mcp
- **Label**: package=bookworm
- **Label**: package=grizzly
- **Label**: package=jp_cli
- **Label**: package=jp_mcp
- **Label**: type=task

`deny.toml` ignores [RUSTSEC-2026-0462]: rmcp's built-in Streamable HTTP clients
buffer response bodies with no size limit, so a malicious MCP server can exhaust
memory.
It is fixed in rmcp 3.9.0, with no backport to 1.x or 2.x.
JP is on 1.8.

The ignore holds only while JP enables neither affected client
(`transport-streamable-http-client-reqwest`,
`transport-streamable-http-client-unix-socket`).
JP's one HTTP client, `LoopbackClient` in
`crates/jp_mcp/src/server/http_client.rs`, connects only to JP's own in-process
loopback endpoint.
External MCP servers connect over stdio.

`LoopbackClient::post_message` calls `response.json()` with no body limit, the
same pattern as the advisory.
That is safe while its only peer is JP.
Once JP connects to remote MCP servers over HTTP, the ignore no longer holds and
that client needs a limit, or should be replaced by rmcp's built-in client at >=
3.9.

The upgrade crosses two major versions (1.8 to 3.9), and 3.0 tracks the
2026-07-28 MCP protocol revision. rmcp 3.x requires Rust 1.88.

- [ ] Bump `rmcp` to `>= 3.9` in the root `Cargo.toml` and fix `jp_mcp`,
      `jp_cli`, `grizzly`, and `bookworm`
- [ ] Bound response bodies in `LoopbackClient`, or replace it with rmcp's
      client if that keeps the no-proxy, no-redirect guarantees
- [ ] Remove the `RUSTSEC-2026-0462` entry from `deny.toml`

[RUSTSEC-2026-0462]: https://rustsec.org/advisories/RUSTSEC-2026-0462
