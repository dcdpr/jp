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

`server::http::Endpoint` exposes the service through MCP Streamable HTTP on an
OS-assigned loopback port.
It validates Host and supplied Origin headers.
Its `connect` method creates an ordinary MCP client connection through that HTTP
endpoint; the private Host channel remains separate.

Upstream stdio calls carry trusted execution context and accumulated answers
under `_meta["computer.jp/tool"]` and `_meta["computer.jp/context"]`.
Single text results are recognized as legacy `Outcome` envelopes when their
shape matches.
Mixed native content and result metadata are retained for forwarding.

The ordinary CLI query runner submits calls through the HTTP endpoint.
Its executor adapter holds pending Host replies across preparation, release,
input, and result review.
After the conversation owner flushes the recorded response, the adapter
acknowledges final delivery and consumes the MCP response.

The Host connection disables environment proxies, redirects, and transparent
session reinitialization.
It does not resubmit a tool call on transport failure.
The HTTP endpoint has no authentication; its loopback binding and header checks
are not a claim that the caller is a particular local application.
