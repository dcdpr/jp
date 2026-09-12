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

The MCP Host can set `ConfiguredTool.metadata` before starting the service.
It is advertised as each tool's `_meta` object, including opaque result-size
hints for external clients.
Incoming call metadata remains correlation data; it cannot change these
descriptions, execution context, options, or answers.

Upstream stdio calls carry trusted execution context and accumulated answers
under `_meta["computer.jp/tool"]` and `_meta["computer.jp/context"]`.
Single text results are recognized as legacy `Outcome` envelopes when their
shape matches.
Mixed native content and result metadata are retained in `jp_tool::ToolResult`.
Execution, Host review, and recording carry that ordered representation; the
HTTP handler converts it back to MCP content after recording is acknowledged.
The CLI explicitly projects it to the existing text/error conversation format.
An unchanged review retains resources, annotations, structured content, and
metadata; a text edit replaces the delivered content.

The ordinary CLI query runner submits calls through the HTTP endpoint.
Its executor adapter holds pending Host replies across preparation, release,
input, and result review.
After the conversation owner flushes the recorded response, the adapter
acknowledges final delivery and consumes the MCP response.

The Host connection disables environment proxies, redirects, and transparent
session reinitialization.
It does not resubmit a tool call on transport failure.
`http_client` implements rmcp's HTTP-client trait using the workspace Reqwest
version.
The rmcp worker owns MCP sessions and SSE resumption; the adapter does not
implement another request retry loop.
The HTTP endpoint has no authentication; its loopback binding and header checks
are not a claim that the caller is a particular local application.

## Conformance checks

The server tests include an independent JSON-RPC/SSE client, without using
`Endpoint::connect` for third-party calls.
A scripted MCP Host handles approval, input, result editing, and recording
through the private channel.
The tests exercise concurrent callers, scoped cancellation, failed recording,
large results, and resumption through `Last-Event-ID` after dropping an HTTP
response.
They also check that an upstream result's native content and metadata survive
the Host's text projection.

These tests do not launch Claude Code, consume subscription quota, or guarantee
exactly-once execution after a process crash.
Agent-specific correlation and configuration belong to the integration consuming
this service.
