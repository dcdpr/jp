# Move dynamic data out of tracing messages into fields

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-13
- **Label**: domain=tooling
- **Label**: type=task

Tracing events across the workspace interpolate their data into the message
(`warn!("Failed to persist before abort: {err}")`) instead of carrying it in
fields.
JP's logs are structured (D15, D32): a message names the event and is the same
for every occurrence, and everything that varies goes in a field, so the log can
be filtered and grouped by field.

The convention:

- The message is a short, static event name: `"Pull failed."`, `"Turn ended."`.
- Varying data goes in fields: `error = %error`, `chat = %chat`, durations as
  integers with the unit in the name (`run_ms`).
- Fields hold data, not prose (the inverse mistake, `PROSE_IN_TRACE_FIELD` in
  `exodus_lints`).

`jp-serve-chat` follows it: its `Host::log_with` takes the message as `&'static
str`, so a formatted message does not compile.

## Sites

A single-line grep for an interpolated message literal,
`(trace|debug|info|warn|error)!(… "…{…}…")`, finds at least 29 sites:

- `jp_cli` (10): `access/compile.rs`, `cmd/plugin/dispatch.rs` (2),
  `cmd/plugin/registry.rs`, `cmd/query.rs`, `cmd/query/stream/retry.rs` (2),
  `cmd/query/turn_loop.rs` (2), `render/chat.rs`
- `jp_llm` (7): `event_builder.rs` (2), `provider/anthropic.rs` (2),
  `provider/anthropic/resolve.rs`, `provider/openai.rs`,
  `provider/openai/resolve.rs`
- `jp_mcp` (2): `server.rs`, `server/command.rs`, both logging a tool's stderr
  line as the message
- `jp_conversation` (1): `thread.rs`
- `jp-serve-web` (4): `client.rs`
- `crates/contrib` (5): `async-anthropic/src/client.rs` (3), `xct2cli` (2)

The grep misses invocations whose message sits on its own line; more than a
hundred macro calls are written that way and need a pass of their own.
`serve-web`'s `"POST /conversations/{{id}}/turn"` messages are static (escaped
braces) and already carry `id` as a field.

## Related gaps

- `jp-serve-web`'s `ProtocolLogLayer` forwards only an event's `message`
  (`MessageVisitor` records no other field), so its fields never reach the host.
- The host's `emit_log` forwards a plugin's `fields` as one JSON-encoded
  `fields` value, because `tracing` fixes field names at compile time.

## Enforcement

Converting the sites once does not keep them converted.
A dylint rule can enforce this by flagging any tracing macro whose message is
not a plain string literal or contains format arguments.
It belongs with the other dylint candidates in T-1dhk8ac (step 2), next to
`PROSE_IN_TRACE_FIELD`.
Until then, document the convention where contributors and assistants read it.

- [ ] Convert the sites the grep found
- [ ] Convert the multi-line invocations
- [ ] Forward event fields in `ProtocolLogLayer`
- [ ] Write the convention down for contributors and assistants
