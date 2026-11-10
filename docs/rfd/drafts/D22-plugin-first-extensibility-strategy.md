# RFD D22: Plugin-First Extensibility Strategy

- **Status**: Draft
- **Category**: Guide
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-13
- **Summary**: Defines JP's plugin-first architecture: a small agent-loop and
  storage core extended by plugins over an internal event bus.

## Summary

This guide defines JP's extensibility strategy: keep the core limited to the
agent loop, storage API, and configuration pipeline, and build LLM providers,
renderers, attachment handlers, and tools as plugins.
An internal event bus connects the core pipeline to plugins that observe,
intercept, or extend execution.

## Why Plugins

JP is a tool for working with LLMs, and its plugin system is designed to be
built with them.
The core involves layered config merging, streaming state machines, and
cross-cutting coordination between tool execution and user interaction.
That work is hard to delegate to an LLM and needs careful human design.

Plugins are the opposite: self-contained binaries or Wasm modules with bounded,
protocol-driven contracts where AI pair programming works well.
This creates a natural division of labor, keeping human attention on the core
while enabling parallel, often assistant-guided experimentation in plugins.

Plugins also speed up iteration.
Contributors can build, test, and ship a plugin or prototype a new workflow
without touching the main binary, waiting for a core review cycle, or risking
regressions in unrelated code paths.

## The Core

The core owns three responsibilities:

- **The agent loop.** The state machine at the center of `jp query` that drives
  the turn lifecycle: building the thread, streaming from the LLM, handling tool
  calls, and persisting events.
  It coordinates plugins without holding domain-specific logic that belongs in a
  plugin.
- **The storage API.** Conversation state, event streams, metadata, workspace
  management, and locking.
  Plugins interact with conversations exclusively through this API and never
  access storage directly.
- **The config pipeline.** The layered merge (`global → workspace → local →
  conversation → env → CLI`) that produces `AppConfig`.
  Once resolved, `AppConfig` is **immutable** for the duration of the run: the
  configuration at startup governs the entire session, and no plugin can mutate
  it afterward.
  Plugins that need mutable session state (OAuth tokens, "don't ask again"
  preferences, cached data) store it through the storage API (workspace state or
  conversation metadata) or in their own side channels.

Everything else is an extension point.

## What Becomes a Plugin

Several subsystems compiled into the `jp` binary belong in plugins instead.
Extracting them shrinks the core and makes each subsystem independently
testable, replaceable, and shippable.

### LLM Providers

Every LLM provider (Anthropic, Google, OpenAI, Ollama, and others) is currently
a Rust module compiled into the binary, with the `Provider` trait and
`get_provider()` dispatch wired into `jp_llm`.
Adding a provider requires modifying core code and shipping a new release.

Moving providers into plugins removes that bottleneck.
When a user configures `anthropic/claude-sonnet-4-6`, JP resolves `anthropic` as
a provider plugin.
First-party provider plugins install silently, preserving the current UX, while
third-party providers prompt for approval under the same trust model as command
plugins.

The `Provider` trait (`model_details`, `models`, `chat_completion_stream`)
already defines the boundary, which a Wasm capability interface or a
protocol-based command plugin can implement.
The CLI stays the same: `jp query -m anthropic/foo` works identically whether
`anthropic` is compiled in or loaded as a plugin.
Extracting providers removes HTTP clients, provider-specific serialization, and
API key management (beyond what the plugin protocol provides) from the core
binary.

### Terminal Rendering

The `ChatRenderer` and `Printer` pipeline formats LLM response chunks for the
terminal and is tightly coupled to the agent loop.

Decoupling them turns the terminal renderer into the default, replaceable output
plugin.
The agent loop emits structured content events (message chunks, tool call
headers, reasoning blocks, status updates), and the rendering plugin consumes
them through the four-channel output model ([RFD 048]), which separates content,
chrome, tool calls, and errors into distinct streams.
Alternative frontends (web UI, TUI, IDE integration) subscribe to the same
events and render them in their own format.

### Attachment Handlers

Attachment handlers are already moving to plugins.
[RFD 016] defines the Wasm `attachment` capability interface, and [RFD 017]
designs the first Wasm attachment handlers.
Built-in handlers (file content, HTTP content, Bear notes) will migrate to Wasm
plugins over time.

### Tools

MCP tools already run as external server processes, and local tools in
`.jp/mcp/tools/` act as shell-script plugins.
For in-process tools, the Wasm plugin architecture ([RFD 016]) reserves a `tool`
capability interface.

## The Internal Event Bus

Rather than defining ad-hoc hook APIs for each stage of execution, JP uses a
single internal event bus for plugins that observe, intercept, or react to the
pipeline.

### How It Works

JP emits structured events at fixed points in the execution pipeline.
Each event carries a type, a timestamp, and a typed payload, and the bus
delivers it to every listener subscribed to that event type.

```
Agent Loop ──emit──▶ Event Bus ──deliver──▶ [Listener A]
                         │                  [Listener B]
                         │                  [Listener C]
                         ▼
                    (continues)
```

Each event type is either **informational** or **interceptable**:

- **Informational events** notify listeners without affecting execution (for
  example, "a turn completed" or "a tool call returned").
- **Interceptable events** let the listener modify the payload or abort the
  operation (for example, "a tool call is about to execute").

### Listener Registration

A listener can be a Wasm plugin, a command plugin connected over the protocol,
or an internal Rust component.
Listeners subscribe only to the event types they handle; a plugin subscribed to
tool-call events never receives rendering events.

How a listener registers depends on its runtime:

- **Command plugins** use the subscription model from [RFD D18].
- **Wasm plugins** receive events in-process through a `listener` capability
  interface.
- **Internal components** (such as the terminal renderer or diagnostics
  subsystem) register directly in Rust.

### Delivery Guarantees

The bus delivers interceptable events synchronously, pausing the pipeline until
the listener responds, so a slow interceptor adds latency to the pipeline.
It delivers informational events asynchronously (fire-and-forget), so
informational listeners cannot block the agent loop.

The event bus handles observation and interception, not primary data flow.
Critical content paths, such as streaming LLM chunks to the renderer, use
dedicated backpressured channels.
The bus broadcasts informational copies of those events to analytics and
diagnostics listeners, where occasional dropped events are acceptable.
Separating the two prevents a slow observer from stalling the agent loop and a
fast producer from causing unbounded memory growth.

Listeners that persist or forward events externally (for example, pushing to a
pub/sub server or writing to a file) manage their own buffering.
JP does not guarantee delivery if a listener crashes.

## Integration Points

Plugins can hook into the pipeline at six stages, from config resolution through
conversation persistence.

### Config Pipeline Hooks

**When:** During config resolution, before `AppConfig` is finalized.

**What a plugin can do:**

- Declare a config surface via JSON Schema so users can configure the plugin
  through the standard inheritance chain (`plugins.command.<name>.options` or a
  dedicated path), with full validation and visibility in `jp config show`.
- Contribute default config layers alongside files, environment variables, and
  CLI flags that the user's own settings can override.

**What a plugin cannot do:** Mutate `AppConfig` after resolution finishes.

**Mechanism:** During the pipeline build, JP validates user-provided values
against each plugin's JSON Schema and passes the validated config in the `init`
message (for command plugins) or as a typed import (for Wasm plugins).
Plugins that need custom merge logic or `KvAssignment` semantics call a
config-over-JSON API during the build phase.

**Bootstrap sequence:** Because the active plugin list is part of the config
being built, resolution runs in two phases, matching MCP server and tool config
resolution: a structural pass identifies enabled plugins and loads their
schemas, and a validation pass applies those schemas to the full config.

**Enables:** Typed, discoverable plugin configuration that replaces the
unstructured `options: Value` field without coupling `jp_config` to plugin
internals (see [RFD 077]).

### Pre-Query Events

**When:** Between `ConfigPipeline::build()` and `run_turn_loop()`, after config
resolution and before the agent loop starts.

**Event type:** Interceptable.

**What a plugin can do:**

- Attach context based on workspace state (current git diff, recent test
  failures, project-specific knowledge files).
- Augment the user's prompt (prepend a preamble or inject structured
  instructions).
- Validate or transform attachments before they enter the thread.

**Visibility constraint:** Pre-query hooks must not mutate the prompt silently,
since hidden modifications obscure what the LLM is responding to.
Injected attachments are recorded in the conversation stream, and prompt
modifications are logged to the chrome channel.

**Scope:** Deterministic workspace context (system instructions, static
attachment lists) belongs in a config pipeline hook.
Pre-query events are for context that depends on the specific query being sent.

### Tool Call Events

**When:** Around tool execution in `ToolCoordinator` (`prepare → permission →
execute → collect results`), which expands with event hooks to `prepare →
intercept → permission → execute → after → complete`.

**Event types:**

- `tool_call.intercept` (interceptable): Emitted after the executor is prepared
  and **before** the permission prompt.
  The listener receives the tool name, arguments, and executor metadata, and can
  modify arguments, substitute the executor, return a cached result, or abort
  the call.
  Running interception before the permission prompt ensures the user always
  reviews and approves the final tool call, including any listener
  modifications.
- `tool_call.after` (interceptable): Emitted after execution, before the result
  is sent to the LLM.
  The listener receives the request and result and can transform, filter, or
  redact the output.
- `tool_call.complete` (informational): Emitted after the result is committed to
  the conversation stream, for logging, analytics, and diagnostics.

**Security:** Because `tool_call.intercept` can rewrite tool calls, listeners
must declare the specific tool names they intercept, and the user approves that
scope at plugin install time (similar to browser extension permissions).
All modifications to tool arguments or executors are logged.

**Enables:**

- *Logging and analytics.* Record every tool call with timing and results.
- *Policy enforcement.* Apply dynamic rules beyond static `run_mode` config (for
  example, blocking filesystem writes during a review or restricting tools to a
  specific MCP server in a workspace).
- *Caching.* Return cached results for deterministic calls (such as `read_file`
  on an unchanged file).
- *Judges.* Observe tool calls and inject automated feedback to steer the main
  LLM (see [Judges](#judges)).

### LLM Response Events

**When:** As the LLM streams response chunks during a turn.

**Event types:**

- `response.chunk` (informational): A copy of each content or reasoning chunk as
  it arrives, broadcast for observation and analysis while primary renderers
  (terminal, web UI, TUI) read from the dedicated backpressured rendering
  channel.
- `response.complete` (informational): The full assembled response once
  streaming ends.

**Enables:**

- *Secondary stream consumers.* Logging, analytics, and live telemetry alongside
  the primary frontend.
- *Response monitoring.* Listeners that evaluate response quality and flag
  off-topic drift, hallucinated code references, or safety concerns.
  Aborting a generation mid-stream requires sending a cancellation signal to the
  agent loop through the interrupt handler stack ([RFD 045]), since
  `response.chunk` is informational.
- *Diagnostics.* Semantic analysis plugins that process completed responses to
  extract summaries, sentiment, named entities, or classifications (see
  [Diagnostics](#diagnostics)).

### Turn Lifecycle Events

**When:** At the boundaries of the turn lifecycle managed by `TurnCoordinator`.

**Event types:**

- `turn.started` (informational): A new turn has begun.
- `turn.cycle_complete` (informational): One LLM streaming cycle finished (tool
  calls may trigger follow-up cycles).
- `turn.complete` (informational): The turn has finished and its events are
  persisted.
- `turn.aborted` (informational): The turn was interrupted or failed.

**Enables:**

- *Post-turn automation.* Trigger workflows after a turn: run tests, commit
  staged changes, update a tracking issue, or notify a team channel.
- *Conversation analytics.* Track turn count, token usage, tool call frequency,
  and model performance over time.
- *Chaining.* Feed a turn's output into another conversation or system (such as
  a review pipeline or summary generator).
- *Judges.* Evaluate the completed response and inject automated follow-up
  messages to correct or refine the LLM's output.

### Conversation Events

**When:** Whenever events are persisted to a conversation stream from any source
(the agent loop, a plugin writing via `push_events`, or a concurrent `jp query`
session).

**Event type:** Informational.

**Enables:**

- *Sync and backup.* Mirror conversations to an external system.
- *Notifications.* Alert when specific patterns occur (such as repeated tool
  call failures or a conversation exceeding a token budget).
- *Indexing.* Build a search index over conversation content for retrieval.

**Mechanism:** Long-lived command plugins subscribe via [RFD D18], while
standard `jp query` sessions deliver events in-process to registered listeners.
A plugin can also run a background pub/sub server alongside `jp` to stream
events to external consumers, or append events to a file for offline processing.

## Event Bus Use Cases

Combining these integration points supports workflows that would otherwise
require changes to the core.

### Judges

A judge is an agent plugin that subscribes to `tool_call.after` or
`turn.complete` events, evaluates the output against criteria such as code
quality, style compliance, or security, and injects a system message or
automated reply to steer the main LLM.
Each judge runs as an independent plugin with its own model configuration and
system prompt, enabled per-workspace or per-conversation in config.

### Interceptors

An interceptor monitors `response.chunk` or `response.complete` events during
streaming and intervenes when the response goes off track.
When it detects hallucinated file paths, contradictory statements, or off-topic
drift, it flags the issue for the user or injects a correction into the next
turn.

### Diagnostics

Diagnostics plugins subscribe to `turn.complete` events, run NLP or lightweight
ML over the turn content, and write structured metadata back to the conversation
through the storage API:

- **Summarization.** Generate turn or conversation summaries.
- **Classification.** Zero-shot classification of conversation topics or intent.
- **Sentiment analysis.** Track the tone and trajectory of a conversation.
- **Named entity recognition.** Extract references to files, functions,
  concepts, and people.
- **Keyword extraction.** Identify themes for indexing and retrieval.

Downstream tools can use this metadata for conversation search, knowledge
graphs, review dashboards, and quality tracking.

### Notifications

Notification plugins subscribe to conversation and turn lifecycle events and
push alerts to external systems: a Slack message when a long-running query
completes, a desktop alert when a tool call needs approval, or a webhook when a
conversation reaches a token budget threshold.
Because notifications are informational and low-frequency, they are a natural
first use case for the event bus.

## Where New Functionality Belongs

Evaluate new features in this order:

1. **Command plugin.** New subcommands, servers, exporters, and standalone
   workflows belong in command plugins, the lowest-friction extension path.
2. **Wasm plugin.** New attachment handlers, tools, and LLM providers belong
   behind a Wasm capability interface.
3. **Event listener.** Features that observe or intercept existing behavior
   (tool calls, responses, turn lifecycle) belong on the event bus.
   If no existing event type covers the use case, design a new event type first.
4. **Configuration.** Features that control deterministic behavior
   (auto-attaching context, setting defaults, enabling tools) belong in a config
   pipeline hook or JSON Schema extension rather than a runtime hook.
5. **Core.** Only changes to the agent loop, storage format, or config pipeline
   itself belong in the core and require a core RFD.

Default to plugins and events.
Before adding self-contained logic to `jp_cli` or `jp_llm`, check whether an
event subscription, a capability interface, or a config extension can express it
instead.

## Plugin Maturity

As the set of third-party plugins grows, users need signals to evaluate a
plugin's maturity along four dimensions:

- **Authoring method.** Core-team authored, assistant-guided (human designs, AI
  implements), or vibe-coded (predominantly AI-generated), indicating how much
  deliberate human design went into the plugin.
- **Test coverage.** Whether the plugin tests its protocol handling and
  capability contracts.
- **Documentation.** Whether the plugin provides a README, structured help ([RFD
  D19]), and a config schema.
- **Integration depth.** From informational listener, to interceptor, to
  agent-integrated, to config-extending.
  Deeper integration carries more surface for breakage.

The plugin registry ([RFD 072]) already carries an `official` field and can add
informational `tested`, `documented`, and `authoring` fields when needed,
without gating publication.

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016]
- [RFD 017: Wasm Attachment Handlers][RFD 017]
- [RFD 048: Four-Channel Output Model][RFD 048]
- [RFD 072: Command Plugin System][RFD 072]
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18]
- [RFD D19: Structured Plugin Help Protocol][RFD D19]
- [RFD 077: Plugin Configuration and Trust Policy][RFD 077]
- [Vector RFC: Registered Internal Events][vector-events]: Prior art for
  internal event systems in pipeline architectures.

[RFD 016]: ../016-wasm-plugin-architecture.md
[RFD 017]: ../017-wasm-attachment-handlers.md
[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 048]: ../048-four-channel-output-model.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 077]: ../077-plugin-configuration-and-trust-policy.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
[RFD D19]: D19-structured-plugin-help-protocol.md
[vector-events]: https://github.com/vectordotdev/vector/blob/master/rfcs/2022-07-28-13691-registered-internal-events.md
