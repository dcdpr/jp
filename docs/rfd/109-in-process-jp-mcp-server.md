# RFD 109: In-Process JP MCP Server

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-12
- **Required by**: [RFD 110]

## Summary

JP moves per-call tool execution into `jp_mcp::server`, running inside the JP
CLI process.
JP and third-party MCP clients use the same MCP invocation path over loopback
Streamable HTTP; a private in-process channel connects the JP MCP Server to JP
for interactions and lifecycle control.
The coordinator, inquiry routing, and conversation storage remain outside the JP
MCP Server.

## Motivation

Tool execution is split between `jp_llm::tool` and `jp_cli::cmd::query::tool`.
Exposing it to Claude Code must not create another implementation of approvals,
questions, argument editing, or result delivery.
A wrapper that delegates the execution pipeline back into the CLI leaves that
ownership problem in place.

This RFD extracts the execution service, not the entire coordinator or agent
loop from [RFD 026].
It provides the MCP dependency needed by future RFDs without waiting for the
full typed-content and attachment migrations in [RFD 058] and [RFD 065].

## Design

### User-facing behavior

Users keep their existing `conversation.tools` and `providers.mcp`
configuration.
Local commands, built-in tools, and tools supplied by configured MCP servers
remain available through ordinary queries:

```sh
jp query --new "Run the configured checks."
```

JP starts its execution service and HTTP endpoint automatically.
Users do not start a daemon, select a port, copy tool definitions, or configure
another MCP server to run a normal query.
The service adds no external runtime dependency; Claude Code installation and
subscription setup belong to a separate RFD.

The initial deployment is exclusively in-process.
A future `jp mcp serve` command for long-running service deployment is outside
this RFD.
Third-party MCP servers retain their existing stdio configuration and
child-process lifecycle; no HTTP variant is added to `providers.mcp`.

### Roles and terminology

| Term                        | Meaning in this design                                                                                                                    |
| --------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| **MCP Host**                | JP's CLI process. It boots the JP MCP Server, supplies trusted configuration and context, and handles interaction and recording requests. |
| **JP MCP Server**           | `jp_mcp::server`, the in-process service responsible for per-call tool execution.                                                         |
| **JP MCP Client**           | The upstream MCP client component owned by the JP MCP Server. It connects to and invokes third-party MCP servers.                         |
| **Third-party MCP Servers** | The MCP servers configured under `providers.mcp`, started and managed by the JP MCP Server through the JP MCP Client.                     |
| **Third-party MCP Client**  | An external caller, initially Claude Code, requesting tools from the JP MCP Server.                                                       |

The MCP Host also makes ordinary MCP requests when JP drives the model/tool loop
itself.
That Host-side connection is distinct from the upstream JP MCP Client.

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

Both HTTP callers use the same MCP handlers, catalog, validation, preparation,
execution, and result processing.
There is no Host-only invocation shortcut into an executor.
An in-memory transport can be added later if justified, but it must carry the
same MCP messages through those handlers, not introduce a second execution API.

The private channel serves a different purpose: it supplies Host services while
an MCP call is being processed.
Third-party MCP clients cannot send Host commands over HTTP.
They can perform ordinary MCP initialization, discovery, calls, and cancellation
of their own requests.

### Ownership and crate boundaries

The JP MCP Server owns tool resolution, per-call requirements and state,
argument/answer validation, process invocation, accumulated answers, and the
final MCP response.
It executes configured custom argument formatters through the same controlled
command-launching path when presentation requests them; pure terminal formatting
stays with the MCP Host.

The MCP Host owns query phases, the execution plan, terminal/editor interaction,
turn-scoped remembered decisions, inquiry routing, and conversation writes.
`ToolCoordinator` remains in `jp_cli` and delegates per-call execution.
Moving that coordinator belongs to [RFD 026], not this extraction.

Extract the execution portions of `ToolDefinition::execute`, local command
handling, upstream dispatch, and the existing `BuiltinTool`/`BuiltinExecutors`
registry.
Tool descriptions and the minimum shared result/input contracts live in
`jp_tool`; configuration-dependent construction and execution do not move into
that lightweight SDK.
Provider-specific schema adaptation stays in `jp_llm`.

The JP MCP Server does not depend on `jp_llm`, `jp_workspace`, or
`jp_conversation`.
Its inputs are resolved configuration and owned context data, not a Workspace, a
provider, or a ConversationStream.
Conversation event wrapping and inquiry provenance conversion belong at the MCP
Host boundary; genuinely shared payload information belongs below both
consumers.

Introduce `client` and `server` features on `jp_mcp`.
The `server` feature enables the client machinery needed for upstream stdio
connections.
The MCP Host's HTTP connection uses the MCP transport library without
generalizing the upstream configuration surface.
Feature selection limits client-only dependencies; it is not a way to conceal
dependency cycles.

Remove the MCP-client parameter from the base attachment-handler interface and
its callers when retiring `jp_attachment_mcp_resources` resolution.
Plugin-based MCP attachments can be designed separately.
Existing stored handler data must remain loadable, with a clear
unsupported-resolution error when used.
This small cleanup removes `jp_attachment`'s MCP dependency without implementing
an attachment redesign; it does not remove MCP tools or their resource results.

### Host-only interaction

The MCP Host supplies `conversation.tools`, `providers.mcp`, the working root,
invocation identity, and existing access-approval data for the active context.
The JP MCP Server does not discover or choose another workspace.
Bind each server instance to its supplied context; concurrent queries must not
share mutable configuration, answers, or pending interactions by accident.

The private channel carries correlated requests and replies for:

- Admission, approval, and argument editing.
- Tool input requests, including supporting content and sensitivity constraints.
- Result review, editing, and delivery decisions.
- Conversation recording acknowledgements.
- Execution release, cancellation, and shutdown.

The JP MCP Server determines which per-call interaction is required and
validates its reply.
The MCP Host decides how to obtain that reply.
In particular, the JP MCP Server makes no distinction between user-targeted and
assistant-targeted inquiries.
The MCP Host applies configured answers, remembered answers, question targets,
and assistant overrides, presents prompts or calls an assistant, and records the
exchange.
Secret answers retain their existing routing and redaction rules and do not
enter ordinary progress events or logs.

Remembering a decision for a Turn remains a Host responsibility.
The JP MCP Server can request an interaction for each invocation and receive an
automatic Host reply; it does not interpret the lifetime of an MCP connection as
a JP Turn.

The JP MCP Server having no terminal does not authorize unattended execution.
The MCP Host applies existing non-interactive policy; changing that policy is
outside this RFD.

The private interface is created by the MCP Host when it starts the JP MCP
Server.
Client names, MCP session IDs, and request metadata do not grant access to it.
Nor can a tool request override configuration, access grants, or accumulated
answers by supplying its own context metadata.

### Preparation and release

A call being prepared is not yet authorized to execute.
The common call path resolves the tool, validates its arguments, obtains
required Host decisions, and waits for release.
Edited arguments are validated again; configured formatter ordering and
visibility remain part of the interaction contract.

For JP-driven model loops, the MCP Host can start preparation as a tool call
request finishes streaming while retaining the existing execution-phase barrier.
It releases calls according to the execution plan derived from the conversation,
not an independent list of queued HTTP requests.
For an external agent, the MCP Host can release an admitted call as soon as the
required preparation finishes.
The JP MCP Server uses the same path in both cases; the MCP Host controls
release timing.

The MCP Host must keep servicing its private channel and model stream while MCP
requests are outstanding.
Awaiting a final HTTP result in the only task capable of releasing the call or
answering its inquiry would deadlock.

### Inquiries re-run tools

`Outcome::NeedsInput` ends an execution attempt.
It does not suspend a tool process for later resumption:

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

Further questions repeat that sequence.
The enclosing MCP call can remain outstanding throughout; each tool execution
attempt has finished before its answer is obtained.
A persistent third-party MCP server can stay alive between attempts, but its
JP-aware tool is invoked again with the answers.
Built-ins are called again as well.

The tool author remains responsible for making this re-execution safe, as under
the existing protocol.
This RFD does not add suspended tool invocations, stateful task handles, or a
new upstream MCP elicitation implementation.

### Narrow content-model integration

Use [RFD 058]'s ordered content and input-request representation for the service
boundary.
Introduce the minimum shared `ContentBlock` and `InputRequest` data and
conversions needed for text, resource data, questions, and structured error
information.
Retain other native MCP content variants and metadata for forwarding without
requiring the MCP Host to render them or persist them as typed blocks.
Carry the MCP-standard resource fields as data; do not require [RFD 065]'s
attachment placement, refresh, or canonicalization work to use them.

This RFD does **not** require completion of RFD 058 or RFD 065.
The implemented shared definitions are the ones those migrations consume, not a
competing service-specific content model.

| Included here                                                              | Deferred                                                                              |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------- |
| Shared ordered result content and schema-described input requests          | Typed conversation-file migration and conversion of every provider/renderer           |
| Conversions from existing `Outcome` and MCP results                        | Mandatory migration of local tools to a new stdout protocol                           |
| Retention of native MCP content and metadata at the JP MCP Server boundary | New binary tool-result rendering in JP's existing provider flows                      |
| Existing text/error projection for the MCP Host                            | Resource deduplication, blob storage, attachment refresh, and stateful tool lifecycle |

Existing local and built-in tools keep working without changes.
New shared input requests retain the secrecy constraints of existing questions.
Malformed recognized envelopes are failures, not permission to silently discard
content.
The JP MCP Server must not lose mixed native MCP content while looking for a JP
result inside it.

The MCP Host adapts results to the existing `ToolCallResponse` representation
and rendering where needed.
Ordinary text/error results retain their existing serialized shape.
This compatibility projection is explicit and shared across invocation paths;
full typed persistence remains RFD 058 work.
A result edited by the MCP Host replaces the delivered content, rather than
allowing the JP MCP Server to return an earlier unedited value.

JP question blocks are not standard MCP result blocks.
Resolve them through the private Host interface before returning the final
standard MCP result.
Do not assume Claude Code interprets a raw `NeedsInput` envelope or a JP
question block as a request to the JP user.

Retaining `Outcome` as an input decoder is a deliberate difference from RFD
058's coordinated removal of that decoder.
It avoids making tool migration a prerequisite for this service; a later removal
requires its own compatibility decision.

### JP-aware upstream MCP tools

Adopt the narrowly scoped interoperability from [RFD 108], within the new
execution path rather than as another implementation in `jp_llm`.

For a result containing exactly one MCP text block, attempt to parse the entire
text as `jp_tool::Outcome`.
A successful parse uses Outcome semantics and is converted to the shared
representation.
No opt-in or protocol advertisement is required.
Do not concatenate mixed content to make it parseable or recursively unwrap
strings within a successful result.
An MCP error flag conflicting with `Outcome::Success` wins, as specified by 108.

A literal JSON document can therefore be interpreted as an Outcome when its
shape matches.
This collision risk is explicitly accepted.
`Outcome` remains a tool-result envelope; it does not authorize execution or
change Host policy.
Otherwise, preserve the native MCP result.
Local stdout retains the existing Outcome and raw-text paths, with typed content
decoding added at the boundary.
A transient-error hint does not authorize blind replay after a lost connection.

A shared context builder supplies local command templates and, for upstream MCP
calls, `_meta["computer.jp/tool"]` and `_meta["computer.jp/context"]`.
It includes the appropriate invoked name, validated execution arguments,
accumulated answers, options, and trusted invocation context.
Any duplicated arguments derive from the same post-edit value.
Build this metadata from Host configuration and replies; never trust incoming
metadata as those values.

This reuses 108's request plumbing and Outcome compatibility without requiring
its separate implementation or broadening the first delivery into new MCP
features.

### Recording, correlation, and lifetime

The JP MCP Server assigns invocation identity independently of transport request
IDs and carries caller correlation metadata to the MCP Host.
The MCP Host associates it with the appropriate tool call and ensures each event
is recorded once.
Host communication distinguishes requested arguments from edited execution
arguments and raw results from approved or edited delivery content.
Neither matching arguments nor arrival order identifies a call: simultaneous
identical invocations are valid.
Interpreting Claude Code's `claudecode/toolUseId` metadata belongs to a future
RFD integration, not the execution policy.
Host-supplied tool-description metadata can likewise carry its result-size hints
without introducing an Anthropic dependency into the JP MCP Server.

Execution release and final delivery respect Host recording acknowledgements.
The MCP Host applies its configured persistence policy; a non-persisting
invocation is not forced to write a conversation to disk.
The JP MCP Server owns no conversation lock.
Historical event replay does not submit new calls, and observed ACP tool events
must not enqueue a second execution of an MCP call.

Use the existing cancellation pattern: the MCP Host sends a scoped stop command
or cancellation token and waits for cleanup.
Stopping current calls and shutting down the JP MCP Server are distinct
operations.
Shutdown stops admission, cancels pending interactions and calls, and closes
owned upstream clients.
The JP MCP Server's HTTP listener dies with the JP process; child-process
cleanup still follows JP's existing execution mechanisms.

The JP MCP Server continues handling control messages while individual calls
wait on answers.
Bound progress buffering separately from required interactions so a slow display
does not block draining a tool's stderr.
A transient HTTP disconnection is not itself cancellation or authority to
execute again.
A crash after a side effect but before recording leaves an uncertain outcome,
not an exactly-once guarantee.

### HTTP and initial security scope

Use MCP [Streamable HTTP], not the legacy HTTP+SSE transport.
Bind to loopback on an OS-assigned port and supply the endpoint to callers
programmatically.
JP's stdin/stdout retain their CLI purpose.
The separate ACP connection used by a future RFD is outside the MCP transport.

The initial endpoint has no authentication token or login flow.
This is an explicit local-access trade-off: loopback does not establish caller
identity, and another local process can submit requests under the bound tool
policies.
Validate Host and supplied Origin headers using the controls provided by
`rmcp`'s `StreamableHttpServerConfig`, with tests for rejected requests.

Sandboxing stays at the current level.
Preserve access-policy compilation and cooperative enforcement; moving code into
a server does not create an OS sandbox.
Future confinement work can use these execution boundaries, but is not part of
this delivery.

## Drawbacks

Using HTTP for JP's own calls adds serialization and lifecycle work.
It buys one invocation path and avoids a second private execution API.
There is no latency benchmark gate; investigate an in-memory MCP transport only
if it solves a measured problem.

The local endpoint accepts unauthenticated callers, and speculative Outcome
recognition can reinterpret text.
Both are explicit initial trade-offs, not claims of stronger isolation.
The compatibility result projection also does not deliver RFD 058's complete
typed-persistence benefits.

## Alternatives

**execution-host wrapper.** Exposes tools while leaving more execution ownership
in the existing CLI arrangement.
This RFD replaces that execution machinery and makes the MCP Host a caller of
the same service as external clients.

**A separate runtime crate.** Unnecessary for the narrowed execution service.
`jp_mcp::server` and feature separation are sufficient without importing the
coordinator, LLM inference, or workspace storage.

**A direct Host execution API plus MCP for external clients.** Creates another
invocation path.
Rejected; transport may vary later, execution semantics may not.

**Require all of RFD 058 first.** Expands the prerequisite into storage,
provider, renderer, and attachment migrations.
The shared types and legacy conversion supply the required interface without
delaying a future RFD for that work.

## Non-Goals

- Extracting `ToolCoordinator` or implementing RFD 026.
- Implementing a future RFD's ACP provider flow, native transcript conversion,
  or subscription authentication.
- Separate-process deployment, controller IPC, a long-running daemon, or `jp mcp
  serve`.
- HTTP transport for configured third-party MCP servers.
- OS sandboxing, a new built-in plugin framework, suspended tool execution, MCP
  tasks, sampling, or new upstream elicitation support.
- Completing RFD 058/065 or changing existing tool and conversation formats as a
  prerequisite.

## Implementation Plan

1. **Shared contracts and dependency cleanup.** Introduce the minimum shared
   result/input types and compatibility conversions.
   Move tool descriptions and tool-domain errors out of their accidental LLM
   ownership.
   Remove the attachment-handler MCP coupling without a plugin redesign.
   Keep existing execution working during these mechanical changes.
2. **Execution service and Host interaction.** Implement `jp_mcp::server` behind
   feature flags.
   Reuse command execution and the built-in registry; add the private Host
   channel, preparation/release, Outcome re-execution, and scoped cancellation.
   Tests use real executor creation and controlled tool fixtures.
3. **One HTTP path and CLI adoption.** Add the Streamable HTTP endpoint and make
   JP's ordinary query path its MCP caller.
   Keep the coordinator and event ownership in JP.
   Add 108 metadata/Outcome handling to upstream stdio calls; do not maintain a
   parallel production execution pipeline.
4. **future RFD readiness.** Exercise a third-party MCP client against the same
   service while the MCP Host handles interactions.
   Verify correlation metadata, result-size metadata, edited results, and
   recording before final response.
   No Anthropic credential or transcript implementation is needed to test this
   contract; future RFD consumes the completed service afterward.

Acceptance tests cover both MCP callers through the same handlers.
Force denied calls, malformed arguments, stale/duplicate interaction replies,
and cancelled prompts, and prove forbidden execution did not occur.
An inquiry fixture must prove separate executions with the accumulated answer,
one logical final result, and no claim of process resumption.
Pin exact CLI output, request/result pairing, and stored text/error results.
Exercise simultaneous identical calls, Host loss, shutdown, failed recording,
and HTTP disconnect without duplicate side effects.
Test client-only and server feature builds.

This RFD can be implemented and used by future RFD without completing the
broader RFD 058, RFD 065, or RFD 026 migrations.
Their documents remain separate references; this delivery establishes only the
shared contracts and execution behavior specified here.

[RFD 026]: 026-agent-loop-extraction.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 108]: 108-transitional-jp-protocol-bridge-for-mcp-tools.md
[RFD 110]: 110-anthropic-subscription-queries-via-acp.md
[Streamable HTTP]: https://modelcontextprotocol.io/specification/2025-11-25/basic/transports#streamable-http
