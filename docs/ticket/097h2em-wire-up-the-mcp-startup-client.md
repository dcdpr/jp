# Wire up the MCP startup client

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091

Adds a tagged line channel to `spawn_stderr_forwarder` and a receiver to
`StartupSet`; `await_mcp_servers` owns one sink per pending server and drops it
when that server's join completes, seeding only from the channel's backlog.
Also adds a persistent, always-emitted line reporting optional-server startup
failures and the tools they take out, pointing to `-v` for full stderr.
