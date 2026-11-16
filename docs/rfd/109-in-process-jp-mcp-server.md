# RFD 109: In-Process JP MCP Server

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-12
- **Extended by**: [RFD 115]
- **Summary**: Moves tool execution into an in-process JP MCP Server called over
  loopback Streamable HTTP by JP and external clients.

## Summary

JP moves per-call tool execution into `jp_mcp::server`, an in-process MCP server
running inside the JP CLI process.
Both JP and third-party MCP clients invoke tools over loopback Streamable HTTP
through the same execution path, while a private in-process channel connects the
JP MCP Server back to JP for user interaction, event recording, and lifecycle
control.
The tool coordinator, inquiry routing, and conversation storage remain in the
CLI outside the JP MCP Server.

## Motivation

Tool execution is currently split between `jp_llm::tool` and
`jp_cli::cmd::query::tool`.
Exposing JP's tools to external MCP clients such as Claude Code must not
duplicate approvals, questions, argument editing, or result delivery, and
wrapping the CLI in an adapter that delegates execution back into `jp_cli` would
leave that split ownership in place.

This RFD extracts per-call tool execution into a dedicated service without
waiting for the full coordinator and agent-loop extraction in [RFD 026], or the
typed-content and attachment migrations in [RFD 058] and [RFD 065].

## Design

### User-facing behavior

Users keep their existing `conversation.tools` and `providers.mcp`
configuration.
Local commands, built-in tools, and tools supplied by configured MCP servers
remain available through ordinary queries:

```sh
jp query --new "Run the configured checks."
```

JP starts the execution service and its HTTP endpoint automatically for each
query.
Users do not start a daemon, select a port, copy tool definitions, or configure
another MCP server to run a query.
The service adds no external runtime dependencies; Claude Code installation and
subscription setup belong to a separate RFD.

The server runs exclusively in-process; a standalone `jp mcp serve` command for
long-running deployment is out of scope.
Third-party MCP servers under `providers.mcp` keep their existing stdio
configuration and child-process lifecycle, with no HTTP variant added.

### Roles and terminology

| Term                        | Role                                                                                                                                      |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| **MCP Host**                | JP's CLI process. It boots the JP MCP Server, supplies trusted configuration and context, and handles interaction and recording requests. |
| **JP MCP Server**           | `jp_mcp::server`, the in-process service responsible for per-call tool execution.                                                         |
| **JP MCP Client**           | The upstream MCP client component owned by the JP MCP Server. It connects to and invokes third-party MCP servers.                         |
| **Third-party MCP Servers** | The MCP servers configured under `providers.mcp`, started and managed by the JP MCP Server through the JP MCP Client.                     |
| **Third-party MCP Client**  | An external caller, initially Claude Code, requesting tools from the JP MCP Server.                                                       |

When JP drives the model and tool loop itself, the MCP Host also calls the JP
MCP Server as an MCP client.
This Host-side connection is distinct from the upstream JP MCP Client that
connects to third-party MCP servers.

### One invocation path

```text
MCP Host -- HTTP --> JP MCP Server <-- HTTP -- Third-party MCP Client
                          |
             +------------+-------------+
             |            |             |
        local command  built-in    JP MCP Client
                                        |
                                      stdio
                                        |
                              third-party MCP server

MCP Host <-------- private typed channels --------> JP MCP Server
```

Both HTTP callers (the MCP Host and any third-party MCP client) go through the
same MCP handlers for tool discovery, argument validation, call preparation,
execution, and result processing.
The MCP Host has no direct shortcut into an executor.
If an in-memory transport is added later, it must route the same MCP messages
through these handlers rather than introducing a second execution API.

The private typed channel is separate from the MCP transport: the JP MCP Server
uses it to request Host services while processing a call, and the MCP Host uses
it for lifecycle control.
Third-party MCP clients cannot send Host commands over HTTP; they can only
initialize sessions, discover and call tools, and cancel their own requests.

### Ownership and crate boundaries

The JP MCP Server owns tool resolution, per-call requirements and state,
argument and answer validation, process invocation, accumulated inquiry answers,
and the final MCP response.
When presentation requires a configured custom argument formatter, the server
runs that formatter command through the same controlled process-execution path;
pure terminal formatting stays with the MCP Host.

The MCP Host owns query phases, the execution plan, terminal and editor
interaction, turn-scoped remembered decisions, inquiry routing, and conversation
writes.
`ToolCoordinator` stays in `jp_cli` and delegates per-call execution to the
server; moving the coordinator is deferred to [RFD 026].

`jp_mcp::server` takes over the execution logic from `ToolDefinition::execute`,
including local command handling, upstream MCP dispatch, and the
`BuiltinTool`/`BuiltinExecutors` registry.
Tool descriptions and the minimum shared result and input-request contracts live
in `jp_tool`, which remains a lightweight SDK without configuration-dependent
construction or execution logic.
Provider-specific schema adaptation stays in `jp_llm`.

`jp_mcp` does not depend on `jp_llm`, `jp_workspace`, or `jp_conversation`.
The server accepts resolved configuration and owned context data rather than a
`Workspace`, model provider, or `ConversationStream`.
Shared payload types live below both `jp_mcp` and `jp_cli`, while conversation
event wrapping and inquiry provenance conversion stay at the MCP Host boundary.

`jp_mcp` splits into `client` and `server` Cargo features.
The `server` feature enables the upstream client machinery needed for stdio
connections to third-party MCP servers, while the MCP Host uses the MCP
transport library directly for its loopback HTTP connection without expanding
the upstream `providers.mcp` configuration surface.
These features limit client-only dependencies rather than concealing dependency
cycles.

Retiring `jp_attachment_mcp_resources` resolution removes the MCP-client
parameter from the base attachment-handler interface and its callers, dropping
`jp_attachment`'s MCP dependency without redesigning attachments or removing MCP
tools and their resource results.
Existing stored handler data remains loadable and returns a clear
unsupported-resolution error when used.
Plugin-based MCP attachments can be designed separately.

### Host-only interaction

When the MCP Host starts a JP MCP Server instance, it binds the server to the
active context: `conversation.tools`, `providers.mcp`, the working root,
invocation identity, and existing access-approval data.
The server never discovers or selects a workspace on its own, and concurrent
queries do not share mutable configuration, accumulated answers, or pending
interactions.

The MCP Host creates the private channel when spawning the server.
Client names, MCP session IDs, and HTTP request metadata do not grant access to
this channel, nor can a tool call override configuration, access grants, or
accumulated answers by supplying its own context metadata.
Over the private channel, the server and Host exchange correlated requests and
replies for:

- Admission, approval, and argument editing.
- Tool input requests, including supporting content and sensitivity constraints.
- Result review, editing, and delivery decisions.
- Conversation recording acknowledgements.
- Execution release, cancellation, and shutdown.

The JP MCP Server determines which per-call interaction is required and
validates the reply, while the MCP Host decides how to obtain that reply.
In particular, the server does not distinguish between user-targeted and
assistant-targeted inquiries: the Host applies configured answers, remembered
answers, question targets, and assistant overrides, prompts the user or queries
an assistant, and records the exchange.
Secret answers retain their existing routing and redaction rules and are never
written to ordinary progress events or logs.

Turn-scoped decisions and non-interactive policy also stay with the MCP Host.
The server does not interpret an MCP connection lifetime as a JP Turn; it
requests an interaction for each invocation and receives an automatic reply when
the Host has remembered a decision for the active Turn.
Similarly, running inside a server with no terminal does not authorize
unattended execution: the Host enforces JP's existing non-interactive policy,
and changing that policy is outside this RFD.

### Preparation and release

Preparing a call does not authorize it to execute.
During preparation, the common call path resolves the tool, validates its
arguments, obtains required Host decisions, and waits at a release gate.
If the Host returns edited arguments, the server validates them again while
preserving the configured formatter ordering and visibility rules.

The JP MCP Server uses this same two-phase path for all callers, while the MCP
Host controls release timing:

- For JP-driven model loops, the Host can start preparing a call as soon as its
  request finishes streaming from the model, while keeping the existing
  execution-phase barrier.
  It releases calls according to the execution plan derived from the
  conversation rather than HTTP arrival order.
- For an external agent, the Host can release an admitted call as soon as
  preparation completes.

Because an in-flight `tools/call` HTTP request waits on execution release and
any subsequent inquiries, the MCP Host must keep servicing the private channel
and model stream while MCP requests are outstanding.
Awaiting a final HTTP response on the only task capable of releasing the call or
answering its inquiry would deadlock.

### Inquiries re-run tools

Returning `Outcome::NeedsInput` ends that execution attempt rather than
suspending a tool process for later resumption:

```text
MCP tools/call
  -> JP MCP Server executes tool
  -> attempt finishes with NeedsInput
  -> JP MCP Server requests an answer from MCP Host
  -> MCP Host obtains and returns the answer
  -> JP MCP Server executes tool again with accumulated answers
  -> attempt finishes with the result
  -> MCP Host handles result delivery and recording
  -> final MCP response
```

Further questions repeat this loop while the enclosing MCP `tools/call` request
remains open.
Each execution attempt exits before the Host collects its answer, and the server
invokes the tool again with the accumulated answers, whether it is a local
command, a built-in tool, or a JP-aware tool on a persistent third-party MCP
server.
As under the existing protocol, the tool author is responsible for making
re-execution safe.
This RFD does not add suspended tool invocations, stateful task handles, or
upstream MCP elicitation.

### Narrow content-model integration

The service boundary uses the ordered content and input-request representation
from [RFD 058] without requiring completion of [RFD 058] or [RFD 065].
`jp_tool` introduces the minimum shared `ContentBlock` and `InputRequest` types
and conversions that those migrations will consume, covering text, MCP-standard
resource fields, questions (with their existing secrecy constraints), and
structured error information.
Other native MCP content variants and metadata pass through the JP MCP Server
for forwarding without requiring the MCP Host to render them or persist them as
typed blocks, and without requiring [RFD 065]'s attachment placement, refresh,
or canonicalization work.

| Included here                                                              | Deferred                                                                              |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Shared ordered result content and schema-described input requests          | Typed conversation-file migration and conversion of every provider/renderer           |
| Conversions from existing `Outcome` and MCP results                        | Mandatory migration of local tools to a new stdout protocol                           |
| Retention of native MCP content and metadata at the JP MCP Server boundary | New binary tool-result rendering in JP's existing provider flows                      |
| Existing text/error projection for the MCP Host                            | Resource deduplication, blob storage, attachment refresh, and stateful tool lifecycle |

Unlike [RFD 058], which removes the `Outcome` input decoder in a coordinated
migration, this RFD keeps `Outcome` decoding alongside typed content blocks so
existing local and built-in tools keep working without changes; removing
`Outcome` later requires its own compatibility decision.
Malformed recognized envelopes fail the call rather than silently discarding
content, and the JP MCP Server never drops sibling native MCP content blocks
when looking for a JP result inside a mixed response.

On the Host side, an explicit compatibility projection adapts results to the
existing `ToolCallResponse` representation and rendering across invocation
paths, preserving the serialized shape of ordinary text and error results until
[RFD 058] migrates conversation storage.
If the MCP Host edits a result, the edited content replaces the delivered
response rather than allowing the server to return the earlier unedited value.
Likewise, because external clients such as Claude Code do not interpret a raw
`NeedsInput` envelope or a JP question block as a user prompt, the server always
resolves JP question blocks through the private Host interface before returning
a standard MCP result.

### JP-aware upstream MCP tools

Upstream stdio calls adopt the narrowly scoped interoperability from [RFD 111]
directly inside `jp_mcp::server`, reusing its request plumbing and `Outcome`
compatibility without a separate implementation in `jp_llm` or new MCP protocol
features.

When an upstream MCP result contains a single text block, the server attempts to
parse the entire text as `jp_tool::Outcome` without requiring opt-in or protocol
advertisement.
If parsing succeeds, the server applies `Outcome` semantics and converts the
result to the shared representation; otherwise, it preserves the native MCP
result.
The server never concatenates multiple content blocks to make them parseable or
recursively unwraps strings inside a successful result, and an MCP error flag
conflicting with `Outcome::Success` takes precedence as specified in [RFD 111].

Because detection is structural, a literal JSON document whose shape matches
`Outcome` will be interpreted as one.
This collision risk is explicitly accepted: `Outcome` is only a tool-result
envelope and cannot authorize execution or alter Host policy, and a
transient-error hint never authorizes blind replay after a lost connection.
Local command stdout retains its existing `Outcome` and raw-text paths, with
typed content decoding added at the boundary.

For outbound calls, a shared context builder populates local command templates
and, on upstream MCP calls, `_meta["computer.jp/tool"]` and
`_meta["computer.jp/context"]`.
It supplies the invoked tool name, validated execution arguments, accumulated
answers, options, and trusted invocation context, deriving any duplicated
argument fields from the same post-edit value.
The server builds this metadata exclusively from Host configuration and replies
and never trusts incoming caller metadata for those values.

### Recording, correlation, and lifetime

The JP MCP Server assigns each call an invocation identity independent of
transport request IDs and forwards caller correlation metadata to the MCP Host.
Because simultaneous identical invocations are valid, the Host correlates calls
by invocation identity rather than matching arguments or arrival order, ensuring
each event is recorded once.
Host messages distinguish requested arguments from edited execution arguments,
and raw results from approved or edited delivery content.
Interpreting Claude Code's `claudecode/toolUseId` metadata belongs to a future
RFD integration rather than the execution policy, and Host-supplied
tool-description metadata can carry result-size hints without introducing an
Anthropic dependency into the JP MCP Server.

The server owns no conversation lock and waits for Host recording
acknowledgements before releasing execution or delivering a final response.
The MCP Host applies its configured persistence policy, so a non-persisting
invocation is not forced to write a conversation to disk.
Replaying historical events does not submit new calls, and observing ACP tool
events must not enqueue a second execution of an MCP call.

Stopping current calls and shutting down the JP MCP Server are distinct
operations, both using JP's existing scoped stop commands or cancellation
tokens:

- Stopping cancels the targeted in-flight calls and waits for cleanup.
- Shutting down stops admitting new calls and cancels all pending interactions
  and running calls.

Upstream MCP connections belong to the MCP Host so they can be shared across
concurrent Turns; shutting down one JP MCP Server leaves those upstream
connections running.
The server's HTTP listener exits with the JP process, and local child-process
cleanup follows JP's existing execution mechanisms.

While individual calls wait on inquiry answers, the JP MCP Server continues
handling control messages.
Progress buffering is bounded separately from required interactions so a slow
terminal display cannot block draining a tool's stderr.
A transient HTTP disconnect neither cancels a call nor authorizes executing it
again, and a crash after a tool side effect but before recording leaves an
uncertain outcome rather than an exactly-once guarantee.

### HTTP and initial security scope

The server uses MCP [Streamable HTTP] rather than the legacy HTTP+SSE transport,
binding to loopback on an OS-assigned port and supplying the endpoint to callers
programmatically.
JP's stdin/stdout retain their CLI purpose, and the ACP connection used by a
future RFD runs outside the MCP transport.

The initial endpoint has no authentication token or login flow.
This is an explicit local-access trade-off: loopback does not establish caller
identity, so another local process can submit requests under the bound tool
policies.
To reject cross-origin requests, the server validates `Host` and supplied
`Origin` headers using `rmcp`'s `StreamableHttpServerConfig`, with tests for
rejected requests.

Moving tool execution into a server does not create an OS sandbox.
The server preserves JP's existing access-policy compilation and cooperative
enforcement; future OS confinement work can build on these execution boundaries.

## Drawbacks

Routing JP's own tool calls over HTTP adds serialization and connection
lifecycle work in exchange for a single invocation path without a second private
execution API.
This RFD sets no latency benchmark gate; an in-memory MCP transport should be
investigated only if measurements show HTTP overhead is a problem.

The loopback endpoint accepts unauthenticated local callers, and structural
`Outcome` detection can reinterpret literal JSON text whose shape matches
`Outcome`.
Both are explicit initial trade-offs rather than claims of stronger isolation.
In addition, projecting results to `ToolCallResponse` defers the full
typed-persistence benefits of [RFD 058].

## Alternatives

**An `execution-host` wrapper around the CLI.** A wrapper could expose tools
over MCP while leaving execution ownership split across the existing CLI
modules.
This RFD instead moves execution into `jp_mcp::server` and makes the MCP Host a
caller of the same service as external clients.

**A separate runtime crate.** A dedicated crate is unnecessary for per-call
execution.
`jp_mcp::server` and Cargo feature separation isolate dependencies without
importing the coordinator, LLM inference, or workspace storage.

**A direct Host execution API plus MCP for external clients.** Giving the Host a
direct execution API while serving external clients over MCP would create two
invocation paths.
Transport may vary later, but execution semantics must stay unified.

**Requiring all of RFD 058 first.** Making the full [RFD 058] migration a
prerequisite would expand this work into storage, provider, renderer, and
attachment migrations.
Introducing the shared boundary types and legacy `ToolCallResponse` conversion
provides the required interface without delaying downstream RFDs.

## Non-Goals

- Extracting `ToolCoordinator` or implementing [RFD 026].
- Implementing a future RFD's ACP provider flow, native transcript conversion,
  or subscription authentication.
- Separate-process deployment, controller IPC, a long-running daemon, or `jp mcp
  serve`.
- HTTP transport for configured third-party MCP servers.
- OS sandboxing, a new built-in plugin framework, suspended tool execution, MCP
  tasks, sampling, or new upstream elicitation support.
- Completing [RFD 058] or [RFD 065], or changing existing tool and conversation
  formats as a prerequisite.

## Implementation Plan

1. **Shared contracts and dependency cleanup.** Introduce the minimum shared
   result and input-request types and compatibility conversions in `jp_tool`,
   and move tool descriptions and tool-domain errors out of `jp_llm`.
   Remove the attachment-handler MCP coupling without redesigning plugins, while
   keeping existing tool execution working.
2. **Execution service and Host interaction.** Implement `jp_mcp::server` behind
   Cargo feature flags, reusing local command execution and the built-in tool
   registry.
   Add the private Host channel, call preparation and release gating, `Outcome`
   re-execution, and scoped cancellation, tested with real executors and
   controlled tool fixtures.
3. **Single HTTP path and CLI adoption.** Add the loopback Streamable HTTP
   endpoint and switch JP's ordinary query path to call it over MCP while
   keeping `ToolCoordinator` and event ownership in `jp_cli`.
   Add [RFD 111] metadata and `Outcome` handling to upstream stdio calls without
   maintaining a parallel execution pipeline.
4. **External MCP client readiness.** Exercise a third-party MCP client against
   the same service while the MCP Host handles interactions.
   Verify correlation metadata, result-size metadata, edited results, and
   conversation recording before the final response returns.
   Testing this contract requires no Anthropic credentials or transcript
   implementation, leaving the completed service ready for downstream RFDs
   without waiting for the broader [RFD 026], [RFD 058], or [RFD 065]
   migrations.

Acceptance tests cover both MCP callers through the same server handlers across
`client`-only and `server` feature builds:

- Force denied calls, malformed arguments, stale or duplicate interaction
  replies, and cancelled prompts, and verify that forbidden execution never
  occurs.
- Use an inquiry fixture to verify separate tool executions with accumulated
  answers, a single final result, and no process suspension.
- Pin exact CLI output, request/result pairing, and stored text and error
  results.
- Exercise simultaneous identical calls, Host loss, shutdown, failed recording,
  and HTTP disconnect without duplicate side effects.

[RFD 026]: 026-agent-loop-extraction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 111]: 111-transitional-jp-protocol-bridge-for-mcp-tools.md
[RFD 115]: 115-bounded-tool-output.md
[Streamable HTTP]: https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#streamable-http
