# RFD D22: Plugin-First Extensibility Strategy

- **Status**: Draft
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-13

## Summary

This guide describes JP's extensibility strategy. The core of JP is the agent
loop and a stable storage API. Everything else — LLM providers, terminal
rendering, attachment handlers, tools — is a plugin or is on a path to becoming
one. An internal event bus provides the connective tissue: JP emits structured
events at well-defined points in its execution pipeline, and plugins subscribe
to observe, intercept, or extend behavior.

This document explains the reasoning, maps the integration points where plugins
participate, and provides a framework for deciding where new functionality
belongs.

## Why Plugins

JP is a tool for working with LLMs, and its plugin system is designed to be
built with them. The core has the properties that make AI-assisted development
hard: layered config merging, streaming state machines, cross-cutting
coordination between tool execution and user interaction. These need careful
human design.

Plugins have the opposite properties. They are self-contained binaries or Wasm
modules with well-defined contracts. Input goes in, output comes out. This is
the kind of bounded, protocol-driven work where AI pair programming works well.
The result is a natural division of labor: human attention on the core, parallel
(and often assistant-guided) experimentation on plugins.

This also means faster iteration. A plugin can be built, tested, and shipped
without touching the main binary. A contributor can prototype a new workflow
without waiting for a core review cycle and without risking regressions in
unrelated code paths.

## The Core

The core is deliberately small. It owns three things:

**The agent loop.** The turn lifecycle: build thread, stream from LLM, handle
tool calls, persist. This is the state machine at the center of `jp query`.
It coordinates plugins but does not contain domain-specific logic that could
live in a plugin instead.

**The storage API.** Conversation state, event streams, metadata, workspace
management, locking. All plugins interact with conversations through this API
— they never access storage directly.

**The config pipeline.** The layered merge that produces `AppConfig` (global →
workspace → local → conversation → env → CLI). A core invariant: once the
pipeline completes, the resulting config is **immutable** for the duration of
the run. No plugin can mutate config after resolution. This invariant
simplifies reasoning about behavior — the config you see at startup is the
config that governs the entire session. Plugins that need mutable session
state (OAuth tokens, "don't ask again" preferences, cached data) store it
through the storage API (workspace state, conversation metadata) or their
own side channels — never by mutating `AppConfig`.

Everything else is an extension point.

## What Becomes a Plugin

Several subsystems that currently live inside the `jp` binary are candidates for
extraction into plugins. The goal is not to move code for the sake of it, but to
reduce the core's surface area and make each subsystem independently testable,
replaceable, and shippable.

### LLM Providers

Today, every LLM provider (Anthropic, Google, OpenAI, Ollama, etc.) is a Rust
module compiled into the binary. The `Provider` trait and `get_provider()`
dispatch are wired into `jp_llm`. Adding a provider means modifying core code
and shipping a new release.

The direction: providers become plugins. When a user configures
`anthropic/claude-sonnet-4-6`, JP resolves `anthropic` as a provider plugin.
First-party provider plugins install silently (the same UX as today).
Third-party providers prompt for approval, following the same trust model as
command plugins.

The `Provider` trait already defines the right boundary: `model_details`,
`models`, `chat_completion_stream`. A Wasm capability interface or a
protocol-based command plugin could implement this trait. The user experience
does not change — `jp query -m anthropic/foo` works the same whether
`anthropic` is compiled in or loaded as a plugin.

This reduces the core binary to the agent loop, storage, and config — no
HTTP clients, no provider-specific serialization, no API key management
beyond what the plugin protocol provides.

### Terminal Rendering

Today, the `ChatRenderer` and `Printer` pipeline process LLM response chunks
into formatted terminal output. This is tightly coupled to the agent loop.

The direction: the agent loop produces structured content events (message
chunks, tool call headers, reasoning blocks, status updates). A rendering plugin
consumes those events and writes to the terminal. The terminal renderer becomes
the default output plugin — always present, but replaceable.

This makes alternative frontends (web UI, TUI, IDE integration) equal
citizens. They subscribe to the same content events and render in their own
way. The four-channel output model ([RFD 048]) already separates content,
chrome, tool calls, and errors into distinct streams — the rendering plugin
consumes these channels.

### Attachment Handlers

Already on this path. [RFD 016] defines Wasm plugins with an `attachment`
capability interface. [RFD 017] designs the first Wasm attachment handlers.
Built-in handlers (file content, HTTP content, Bear notes) will migrate to
Wasm plugins over time.

### Tools

The Wasm plugin architecture ([RFD 016]) reserves a `tool` capability
interface for future work. MCP tools are already external (they run as
separate server processes). Local tools defined in `.jp/mcp/tools/` are
effectively shell-script plugins. The pattern is established.

## The Internal Event Bus

The integration points described in the next section share a common need:
plugins that observe, intercept, or react to things happening inside JP's
execution pipeline. Rather than designing ad-hoc hook APIs for each case, JP
uses a single mechanism: an internal event bus.

### How It Works

JP emits structured events at well-defined points in its execution pipeline.
Each event has a type, a timestamp, and a typed payload. Plugins register as
listeners for specific event types. The bus delivers matching events to each
registered listener.

```
Agent Loop ──emit──▶ Event Bus ──deliver──▶ [Listener A]
                         │                  [Listener B]
                         │                  [Listener C]
                         ▼
                    (continues)
```

Some events are **informational**: the listener observes but does not
influence the outcome (e.g., "a turn completed," "a tool call returned").
Some events are **interceptable**: the listener can modify the payload or
abort the operation (e.g., "a tool call is about to execute"). The event
type determines which category it belongs to.

### Listener Registration

A listener is anything that can receive events: a Wasm plugin, a command
plugin connected over the protocol, or an internal Rust component. Listeners
declare which event types they care about. The bus only delivers matching
events — plugins that subscribe to tool-call events don't see rendering
events.

For command plugins, event delivery uses the subscription model from
[RFD D18]. For Wasm plugins, a new `listener` capability interface receives
events in-process. For internal components (like the terminal renderer or
future diagnostics subsystem), registration is direct.

### Delivery Guarantees

Events are delivered synchronously for interceptable events (the pipeline
waits for the listener's response) and asynchronously for informational
events (fire-and-forget). This means interceptable hooks add latency — a
slow listener slows the pipeline. Informational listeners cannot block the
agent loop.

The event bus is for observation and interception, not for primary data
flow. Critical content paths — such as streaming LLM chunks to the
renderer — use dedicated backpressured channels. The event bus broadcasts
informational copies of those events to analytics and diagnostics
listeners, where occasional dropped events are acceptable. This
distinction prevents a slow observer from blocking the agent loop or a
fast producer from causing unbounded memory growth.

A listener that needs to persist or forward events externally (e.g., push
to a pub/sub server, write to a file) is responsible for its own buffering.
JP does not guarantee delivery if the listener crashes.

## Integration Points

These are the events the bus emits and the points in the pipeline where
plugins can participate. Each integration point describes what it is, what
plugins can do with it, and what use cases it enables.

### Config Pipeline Hooks

**When:** During config resolution, before the config is finalized.

**What a plugin can do:**

- Contribute additional config layers. A plugin declares its config surface
  via JSON Schema, and users set values through the standard config
  inheritance chain (`plugins.command.<name>.options` or a dedicated path).
  The plugin's schema participates in validation and `jp config show`.
- Provide config defaults that the user can override, rather than the other
  way around.

**What a plugin cannot do:** Mutate config after the pipeline completes.
The `AppConfig` immutability invariant holds. Config pipeline hooks run
during the build phase, contributing layers alongside files, env vars, and
CLI flags. Once the pipeline produces the final `AppConfig`, it is frozen.

**Mechanism:** Plugins declare a JSON Schema for their config surface.
JP validates user-provided values against this schema during the pipeline
build. The validated config is passed to the plugin in the `init` message
(for command plugins) or as a typed import (for Wasm plugins). Complex
config needs (custom merge logic, `KvAssignment` semantics) are served by a
config-over-JSON API that plugins call during the build phase.

**Bootstrap sequence:** The config pipeline must know which plugins are
active before it can load their schemas, but the plugin list is itself part
of the config being built. This is resolved with a two-phase approach:
first, a structural parse identifies enabled plugins and loads their
schemas; then, a validating pass applies those schemas to the full config.
This is the same pattern JP already uses for MCP server and tool config
resolution.

**Enables:** Typed, validated plugin config without coupling `jp_config` to
plugin internals. Replaces the current unstructured `options: Value` field
with something discoverable and self-documenting. See [RFD 077] for the
initial design.

### Pre-Query Events

**When:** After config resolution, before the agent loop starts. This is the
gap between `ConfigPipeline::build()` and `run_turn_loop()`.

**Event type:** Interceptable.

**What a plugin can do:**

- Auto-attach context based on workspace state (current git diff, recent
  test failures, project-specific knowledge files).
- Augment the user's prompt (prepend a preamble, inject structured
  instructions).
- Validate or transform attachments before they enter the thread.

**Visibility constraint:** Any context injected via pre-query events must
be surfaced to the user. Injected attachments appear in the conversation
record. Prompt modifications are logged to the chrome channel. Silent
prompt mutation breaks the user's mental model of what the LLM is
responding to — pre-query hooks must not operate invisibly.

**Note:** Many auto-context use cases are better expressed as config
contributions (system instructions, attachment lists) rather than runtime
hooks. If the context is deterministic and based on workspace state, a
config pipeline hook is the right tool. Pre-query events are for cases
where the context depends on the specific query being sent.

### Tool Call Events

**When:** Before and after tool execution. The `ToolCoordinator` manages
the pipeline: `prepare → permission → execute → collect results`.

**Event types:**

- `tool_call.intercept` (interceptable): Emitted after the executor is
  prepared but **before** the permission prompt. The listener receives the
  tool name, arguments, and executor metadata. It can modify arguments,
  substitute the executor, return a cached result, or abort the call. The
  modified request is what the user sees in the permission prompt — the
  interception is visible, not hidden.
- `tool_call.after` (interceptable): Emitted after execution, before the
  result is sent to the LLM. The listener receives the request and result.
  It can transform, filter, or redact the result.
- `tool_call.complete` (informational): Emitted after the result is
  committed to the conversation stream. For logging, analytics, and
  diagnostics.

The pipeline ordering is: `prepare → intercept → permission → execute →
after → complete`. This ordering is deliberate: because interception
happens before permission, the user always approves the final form of the
tool call, including any modifications made by listeners.

**Security consideration:** An interceptable `tool_call.intercept` event
gives a listener the power to modify tool calls. Listeners must declare
which tool names they intercept. The user explicitly approves this at
plugin install time. Modifications to tool arguments or executors are
logged. The permission model is analogous to browser extension permissions
— you grant access to specific capabilities, not blanket access.

**Enables:**

- *Logging and analytics.* Record every tool call with timing and results.
- *Policy enforcement.* Dynamic rules beyond static `run_mode` config (e.g.,
  "no filesystem writes during review," "only allow tools from this MCP
  server in this workspace").
- *Caching.* Return cached results for deterministic calls (e.g., a
  `read_file` for a file unchanged since the last call).
- *Judges.* An agent that observes tool calls and provides automated
  feedback to steer the main LLM (see [Judges](#judges) below).

### LLM Response Events

**When:** As the LLM streams response chunks during a turn.

**Event types:**

- `response.chunk` (informational): A copy of each content or reasoning
  chunk as it arrives. Note: the primary renderer (terminal, web UI) does
  **not** consume chunks through the event bus. It uses a dedicated
  backpressured channel to guarantee no data loss. The event bus broadcasts
  copies for observation and analysis.
- `response.complete` (informational): The full assembled response after
  streaming ends.

**Enables:**

- *Alternative frontends.* A web UI or TUI subscribes to the dedicated
  rendering channel (not the event bus) for reliable content delivery.
  The event bus serves secondary consumers like logging and analytics.
- *Interception analysis.* A listener that monitors response quality and
  flags issues (off-topic responses, hallucinated code references, safety
  concerns). Real-time stream interception (aborting a generation
  mid-stream) is not served by the informational `response.chunk` event.
  That requires a cancellation signal to the agent loop, likely through
  the interrupt handler stack ([RFD 045]).
- *Diagnostics.* Semantic analysis plugins that process complete responses
  to extract summaries, sentiment, named entities, or classifications
  (see [Diagnostics](#diagnostics) below).

### Turn Lifecycle Events

**When:** At the boundaries of the turn lifecycle managed by
`TurnCoordinator`.

**Event types:**

- `turn.started` (informational): A new turn has begun.
- `turn.cycle_complete` (informational): One LLM streaming cycle finished
  (there may be more if tool calls trigger follow-up cycles).
- `turn.complete` (informational): The entire turn has finished and events
  are persisted.
- `turn.aborted` (informational): The turn was interrupted or failed.

**Enables:**

- *Post-turn automation.* Trigger workflows after a turn: run tests, commit
  staged changes, update a tracking issue, notify a team channel.
- *Conversation analytics.* Track turn count, token usage, tool call
  frequency, and model performance over time.
- *Chaining.* Feed a turn's output into another conversation or system
  (e.g., a review pipeline, a summary generator).
- *Judges.* An agent that evaluates the completed response and injects
  automated follow-up messages to correct or refine the LLM's output.

### Conversation Events

**When:** Whenever events are persisted to a conversation stream, from any
source (the agent loop, a plugin writing via `push_events`, a concurrent
`jp query` session).

**Event type:** Informational.

**Enables:**

- *Sync and backup.* Mirror conversations to an external system.
- *Notifications.* Alert when specific patterns occur (e.g., a tool call
  fails repeatedly, a conversation exceeds a token budget).
- *Indexing.* Build a search index over conversation content for retrieval.

**Mechanism:** For command plugins, this is the subscription model from
[RFD D18]. For regular `jp query` runs (no long-lived command plugin), a
lightweight registration mechanism lets listeners receive events in-process
during the session. A plugin could also start a background pub/sub server
alongside `jp` that streams events to external listeners, or simply append
events to a file for offline processing.

## Use Cases Enabled by the Event Bus

The integration points above combine to support several patterns that would
otherwise require core changes.

### Judges

Configurable agents that observe specific events and provide automated
feedback to steer the main LLM. A judge subscribes to `tool_call.after`
and/or `turn.complete` events, evaluates the content against criteria
(code quality, adherence to style guides, correctness), and injects a
system message or automated reply into the conversation.

Different judges can cover different concerns: one for code review, one for
security analysis, one for style compliance. They run as independent
plugins, each with its own model configuration and system prompt. The user
enables judges per-workspace or per-conversation through config.

### Interceptors

Automated systems that monitor LLM responses in real time and intervene
when the output goes off track. An interceptor subscribes to
`response.chunk` or `response.complete` events and evaluates quality
signals. When it detects a problem (hallucinated file paths, contradictory
statements, off-topic drift), it can flag the issue for the user or inject
a correction into the next turn.

This is similar to judges but operates during streaming rather than after
the turn completes.

### Diagnostics and Semantic Metadata

Plugins that apply NLP or lightweight ML to conversation content to extract
structured metadata:

- **Summarization.** Generate turn or conversation summaries.
- **Classification.** Zero-shot classification of conversation topics or
  intent.
- **Sentiment analysis.** Track the tone and trajectory of a conversation.
- **Named entity recognition.** Extract references to files, functions,
  concepts, and people.
- **Keyword extraction.** Identify key themes for indexing and retrieval.

A diagnostics plugin subscribes to `turn.complete` events, processes the
content, and writes structured metadata back to the conversation (via the
storage API). This metadata enables downstream features: conversation
search, knowledge graphs, review dashboards, and quality tracking.

### Notifications

A plugin that subscribes to conversation and turn lifecycle events and
pushes notifications to external systems. Examples: a Slack notification
when a long-running query completes, a desktop notification when a tool
call needs approval, or a webhook that fires when a conversation reaches
a token budget threshold.

This is a natural first use case for the event bus because it is
informational (no interception), low-frequency, and immediately useful.

## Where Does New Functionality Belong?

When considering a new feature, apply this sequence:

**1. Can it be a command plugin?** If the feature is a new subcommand, a
server, an exporter, or a standalone workflow — build it as a command
plugin. This is the lowest-friction path.

**2. Can it be a Wasm plugin?** If the feature is a new attachment handler,
tool, or LLM provider — it belongs behind a Wasm capability interface.

**3. Can it be an event listener?** If the feature needs to observe or
intercept existing behavior (tool calls, responses, turn lifecycle), check
whether one of the event types above covers the use case. If the event type
doesn't exist yet, that's a signal to design it.

**4. Can it be expressed as config?** If the feature is about controlling
existing behavior (auto-attaching context, setting defaults, enabling
tools), a config pipeline hook or a JSON Schema extension is likely the
right path. Prefer config over runtime hooks when the behavior is
deterministic.

**5. Does it need to change the core?** If the feature requires modifying
the agent loop, the storage format, or the config pipeline itself — it
belongs in the core. Write an RFD.

The bias is toward plugins and events. If you find yourself reaching into
`jp_cli` or `jp_llm` to add something that could be self-contained, step
back and ask whether an event subscription, a capability interface, or a
config extension would serve the same purpose.

## Plugin Maturity

As the plugin ecosystem grows, users need signals to assess plugin quality.
This section sketches a vocabulary for plugin maturity — not a scoring
system to implement today, but a framework for when the ecosystem warrants
it.

**Dimensions:**

- **Authoring method.** Core-team authored, assistant-guided (human designs,
  AI implements), or vibe-coded (predominantly AI-generated). Not a quality
  judgment — signals the level of deliberate design.
- **Test coverage.** Does the plugin test protocol handling or capability
  contracts?
- **Documentation.** README, structured help ([RFD D19]), config schema.
- **Integration depth.** Informational listener → interceptor →
  agent-integrated → config-extending. Deeper integration means more
  surface for breakage.

The plugin registry ([RFD 072]) already carries an `official` field. Future
registry metadata could include `tested`, `documented`, and `authoring`
signals — informational, not gatekeeping.

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 017: Wasm Attachment Handlers][RFD 017]
- [RFD 048: Four-Channel Output Model][RFD 048]
- [RFD 072: Command Plugin System][RFD 072]
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18]
- [RFD D19: Structured Plugin Help Protocol][RFD D19]
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077]
- [Vector RFC: Registered Internal Events][vector-events] — prior art for
  internal event systems in pipeline architectures.

[RFD 016]: 016-wasm-plugin-architecture.md
[RFD 017]: 017-wasm-attachment-handlers.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 072]: 072-command-plugin-system.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
[RFD D19]: D19-structured-plugin-help-protocol.md
[RFD 077]: 077-plugin-configuration-and-trust-policy.md
[vector-events]: https://github.com/vectordotdev/vector/blob/master/rfcs/2022-07-28-13691-registered-internal-events.md
