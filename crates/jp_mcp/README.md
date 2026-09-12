# Model Context Protocol (MCP)

The default `client` feature manages configured upstream stdio MCP servers.
The `server` feature also enables tool resolution, local command execution, and
the built-in tool registry.

`server::service::Service` manages individual calls against an immutable tool
catalog and working context.
Its private Host receiver carries admission, execution release, input, result
review, and recording requests.
The MCP Host must service those requests while calls run.
Questions finish an execution attempt; answers trigger a new attempt with
accumulated input.

Calls have independent cancellation tokens.
Dropping a result receiver does not cancel or retry a call.
`cancel_current` stops current work while allowing later calls; `shutdown` stops
admission, cancels calls, waits for cleanup, and closes owned upstream
connections.
Stderr progress uses a separate bounded channel.

This library service does not start an HTTP listener.
MCP transport integration and CLI adoption use this service in RFD 109's next
phase.
