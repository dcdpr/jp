# Structured Output Architecture

This document describes the target architecture for structured output in JP.
It replaces the current `Provider::structured_completion` approach with a
unified flow through `chat_completion_stream`, using native provider APIs
and existing conversation event types.

## Table of Contents

- [Overview](#overview)
- [Motivation](#motivation)
- [Design Goals](#design-goals)
- [Core Concepts](#core-concepts)
  - [Schema on `ChatRequest`](#schema-on-chatrequest)
  - [Structured Variant on `ChatResponse`](#structured-variant-on-chatresponse)
  - [Serialization](#serialization)
  - [Event Lifecycle](#event-lifecycle)
- [Architecture Overview](#architecture-overview)
- [Provider Changes](#provider-changes)
  - [Schema Detection](#schema-detection)
  - [Native Structured Output Mapping](#native-structured-output-mapping)
  - [Event Conversion](#event-conversion)
  - [Streaming Structured Parts](#streaming-structured-parts)
  - [Removing `structured_completion` and `chat_completion`](#removing-structured_completion-and-chat_completion)
- [Event Builder Changes](#event-builder-changes)
  - [New `IndexBuffer` Variant](#new-indexbuffer-variant)
  - [Flush Behavior](#flush-behavior)
- [Turn Loop Integration](#turn-loop-integration)
  - [Streaming Phase](#streaming-phase)
  - [Rendering](#rendering)
  - [Post-Turn Extraction](#post-turn-extraction)
- [Background Callers](#background-callers)
  - [Title Generator](#title-generator)
  - [Conversation Edit](#conversation-edit)
- [Data Flow](#data-flow)
  - [Interactive Query Flow](#interactive-query-flow)
  - [Background Task Flow](#background-task-flow)
  - [Persisted Event Stream](#persisted-event-stream)
  - [Multi-Turn Conversation](#multi-turn-conversation)
- [Error Handling](#error-handling)
- [Testing Strategy](#testing-strategy)
- [Migration Path](#migration-path)

---

## Overview

Structured output allows the user to request a JSON response conforming to a
schema. The current implementation uses a separate code path
(`Provider::structured_completion`) that fakes a tool call to coerce JSON
from the model. This path bypasses the streaming pipeline, retry logic,
persistence, and signal handling.

The new architecture eliminates this separate path. Instead of introducing
new event types, it extends the existing `ChatRequest` and `ChatResponse`
types: the schema becomes an optional field on `ChatRequest`, and the
structured JSON data becomes a new `ChatResponse` variant. Providers use
their native structured output APIs, and everything flows through the same
`chat_completion_stream` → `run_turn_loop` pipeline as normal queries.

---

## Motivation

The current `handle_structured_output` function:

1. **Duplicates provider/model resolution** with `handle_turn`
2. **Has no transport-level retries** — rate limits and timeouts fail
   immediately
3. **Does not persist** — the response is added to a local `Thread` clone
   but never synced to the workspace
4. **Has no signal handling** — Ctrl+C during the call is unhandled
5. **Uses a tool-call workaround** instead of native structured output APIs
   that all major providers now support

Every major provider supports native structured output:

| Provider    | Mechanism                                                |
|-------------|----------------------------------------------------------|
| Anthropic   | `output_config.format = { type: "json_schema", schema }` |
| OpenAI      | `response_format = { type: "json_schema", ... }`         |
| Google      | `response_schema` + `response_mime_type`                 |
| Ollama      | `format: <schema>`                                       |
| OpenRouter  | Passes through to underlying provider                    |
| Llamacpp    | `response_format` (OpenAI-compatible)                    |

With native support, the provider **guarantees** schema compliance. The
tool-call workaround's validation-retry loop becomes unnecessary.

---

## Design Goals

| Goal                     | Description                                        |
|--------------------------|----------------------------------------------------|
| **Single code path**     | Structured and normal queries flow through the     |
|                          | same streaming pipeline                            |
| **Native provider APIs** | Use each provider's structured output mechanism    |
| **No new event types**   | Extend `ChatRequest` and `ChatResponse` instead    |
|                          | of adding new `EventKind` variants                 |
| **Eliminate dead code**  | Remove `structured_completion`, `chat_completion`, |
|                          | `StructuredQuery`, `SCHEMA_TOOL_NAME`              |
| **Incremental rendering**| Stream JSON tokens to terminal in a fenced code    |
|                          | block                                              |

---

## Core Concepts

### Schema on `ChatRequest`

A structured request is fundamentally a chat request with an output format
constraint. The schema is an optional field on `ChatRequest`:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The user's query or message content.
    pub content: String,

    /// Optional JSON schema constraining the assistant's response format.
    ///
    /// When present, providers set their native structured output
    /// configuration (e.g. Anthropic's `output_config.format`) and the
    /// assistant's response is emitted as `ChatResponse::Structured`
    /// instead of `ChatResponse::Message`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<Map<String, Value>>,
}
```

This models the domain accurately: a structured request is a user message
("extract contacts from this text") combined with a format constraint
("respond as JSON matching this schema"). The Anthropic API enforces this
relationship — `output_config.format` requires at least one user message.
By placing the schema on `ChatRequest`, this pairing is structural.

**Backward compatibility:** Existing serialized events have no `schema`
field. With `#[serde(default, skip_serializing_if = "Option::is_none")]`,
deserialization of old events produces `schema: None`, and serialization
of non-structured requests omits the field entirely. The wire format is
unchanged for normal requests.

### Structured Variant on `ChatResponse`

The assistant's structured JSON data is a new variant on `ChatResponse`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged, rename_all = "snake_case")]
pub enum ChatResponse {
    /// A standard message response.
    Message {
        message: String,
    },

    /// Reasoning/thinking response.
    Reasoning {
        reasoning: String,
    },

    /// Structured JSON response conforming to the schema from the
    /// preceding `ChatRequest`.
    Structured {
        /// The structured JSON value.
        ///
        /// After flush, this is the parsed JSON (object, array, etc.).
        /// During streaming, individual parts carry `Value::String`
        /// chunks that are concatenated by the `EventBuilder`.
        data: Value,
    },
}
```

`ChatResponse` already uses `#[serde(untagged)]` — variants are
distinguished by their field name (`message`, `reasoning`, `data`). Adding
`Structured` is a third variant with a unique field name. Existing events
deserialize exactly as before.

### Serialization

The existing serialized format (inside `EventKind` with `#[serde(tag =
"type")]`):

```json
{
  "type": "chat_request",
  "content": "hello"
}
{
  "type": "chat_response",
  "message": "world"
}
{
  "type": "chat_response",
  "reasoning": "let me think..."
}
```

New structured events:

```json
{"type": "chat_request", "content": "Extract contacts", "schema": {"type": "object", ...}}
{"type": "chat_response", "data": {"name": "Alice"}}
```

Serde's `untagged` deserialization tries variants in order:

1. `Message` — does the JSON have a `message` field? → match
2. `Reasoning` — does the JSON have a `reasoning` field? → match
3. `Structured` — does the JSON have a `data` field? → match

The field names are distinct, so there is no ambiguity.

### Event Lifecycle

```
User runs: jp query --schema '{"type":"object",...}' "Extract contacts"

1. ConversationStream receives:
   [ChatRequest { content: "Extract contacts", schema: Some({...}) }]

2. Provider reads schema from the last ChatRequest:
   → Sets native output format config (e.g. output_config.format)
   → Converts ChatRequest.content to a user message (schema is not
     included in the message text)

3. Provider streams JSON tokens as ChatResponse::Structured parts:
   Part { index: 0, ChatResponse::Structured { data: String("{\"name") } }
   Part { index: 0, ChatResponse::Structured { data: String("\": \"Al") } }
   Part { index: 0, ChatResponse::Structured { data: String("ice\"}") } }
   Flush { index: 0 }

4. EventBuilder accumulates String chunks, parses on flush:
   → Persists: ChatResponse::Structured { data: {"name": "Alice"} }

5. Turn completes. Caller extracts structured data from stream.
```

---

## Architecture Overview

```
Before (two separate paths):
─────────────────────────────

  handle_turn ──► run_turn_loop ──► ResilientRequest
                                    TurnCoordinator
                                    EventBuilder
                                    Persistence

  handle_structured_output ──► provider.structured_completion
                               (tool-call hack, own retry, no persist)

After (unified):
────────────────

                    ┌─────────────────────┐
                    │    Query::run       │
                    │                     │
                    │  if --schema:       │
                    │    add ChatRequest  │
                    │    with schema      │
                    │  else:              │
                    │    add ChatRequest  │
                    │    without schema   │
                    │                     │
                    │  call handle_turn   │
                    └──────────┬──────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │    run_turn_loop    │
                    │                     │
                    │  ResilientRequest     │
                    │  TurnCoordinator    │
                    │  EventBuilder       │
                    │  Persistence        │
                    │  Signal handling    │
                    │  Waiting indicator  │
                    └──────────┬──────────┘
                               │
                               ▼
                    ┌─────────────────────┐
                    │  Provider reads     │
                    │  ChatRequest.schema │
                    │                     │
                    │  Sets native output │
                    │  format config      │
                    │                     │
                    │  Streams JSON tokens│
                    │  as Structured parts│
                    └─────────────────────┘
```

---

## Provider Changes

### Schema Detection

Each provider's `create_request` function reads the schema from the last
`ChatRequest` in the conversation stream:

```rust
fn create_request(model: &ModelDetails, query: ChatQuery) -> Result<Request> {
    let ChatQuery { thread, tools, tool_choice, .. } = query;
    let Thread { events, .. } = thread;

    // Read schema from the last ChatRequest (the current request).
    let structured_schema = events
        .last()
        .and_then(|req| req.schema.as_ref());

    // ... build messages ...

    if let Some(schema) = structured_schema {
        // Set provider-specific output format config
    }

    // ...
}
```

**Important:** Only the LAST `ChatRequest` determines the output format.
Historical `ChatRequest`s may have schemas from previous structured turns —
these are ignored during request building. The schema on historical requests
is preserved for replay and auditing only.

### Native Structured Output Mapping

Each provider maps the schema to its native API:

**Anthropic:**

```rust
if let Some(schema) = structured_schema {
    builder.output_config(OutputConfig {
        effort: existing_effort,
        format: Some(JsonOutputFormat::JsonSchema { schema }),
    });
}
```

Note: `OutputConfig` in `async_anthropic::types` has been extended with a
`format` field:

```rust
pub struct OutputConfig {
    pub effort: Option<Effort>,
    pub format: Option<JsonOutputFormat>,
}

pub enum JsonOutputFormat {
    JsonSchema { schema: Map<String, Value> },
}
```

When both `effort` (from reasoning config) and `format` (from structured
request) are needed, they coexist on the same `OutputConfig`. The current
code that sets `effort` must be updated to merge with `format` rather than
replacing the entire `OutputConfig`.

**OpenAI:**

```rust
if let Some(schema) = structured_schema {
    request.response_format = Some(ResponseFormat::JsonSchema {
        json_schema: JsonSchemaFormat {
            name: "structured_output".to_owned(),
            schema: Value::Object(schema),
            strict: Some(true),
        },
    });
}
```

**Google:**

```rust
if let Some(schema) = structured_schema {
    request.generation_config.response_mime_type =
        Some("application/json".to_owned());
    request.generation_config.response_schema =
        Some(Value::Object(schema));
}
```

**Ollama:**

```rust
if let Some(schema) = structured_schema {
    request.format = Some(Value::Object(schema));
}
```

**OpenRouter:**

OpenRouter passes through to the underlying provider. Set the OpenAI-style
`response_format` field, which OpenRouter forwards.

**Llamacpp:**

Uses the OpenAI-compatible `response_format` field.

### Event Conversion

When converting `ConversationStream` events to provider-specific messages
(e.g. `convert_events` in the Anthropic provider), the existing match arms
handle the new cases naturally:

**`ChatRequest` with schema:**

The `content` field is converted to a user message, as before. The `schema`
field is NOT included in the message text — it's read separately by
`create_request` to set the output format config.

```rust
EventKind::ChatRequest(request) if !request.content.is_empty() => Some((
    Role::User,
    Content::Text(request.content),
    // request.schema is intentionally NOT included here.
    // It's read by create_request, not sent as message text.
))
```

**`ChatResponse::Structured`:**

On subsequent turns, the LLM should see the JSON it previously produced.
This provides useful context (e.g. "change the email field in the previous
response"). The structured data is converted to an assistant text message:

```rust
EventKind::ChatResponse(resp) => {
    let (role, content) = match resp {
        ChatResponse::Message { message } => {
            (Role::Assistant, message)
        }
        ChatResponse::Reasoning { reasoning } => {
            // ... existing reasoning handling ...
        }
        ChatResponse::Structured { data } => {
            (Role::Assistant, data.to_string())
        }
    };

    Some((role, Content::Text(content)))
}
```

### Streaming Structured Parts

When the provider detects a schema on the current `ChatRequest`, it emits
streamed text tokens as `ChatResponse::Structured` parts instead of
`ChatResponse::Message` parts:

```rust
// In the provider's event mapping function:

if is_structured_request {
    Event::Part {
        index,
        event: ConversationEvent::now(ChatResponse::Structured {
            data: Value::String(text_chunk),
        }),
    }
} else {
    Event::Part {
        index,
        event: ConversationEvent::now(ChatResponse::message(text_chunk)),
    }
}
```

The provider carries an `is_structured` flag set during `create_request`
and threaded through to the event mapping logic. This flag is derived from
whether the last `ChatRequest` had a schema.

### Removing `structured_completion` and `chat_completion`

The following are removed from the `Provider` trait:

```rust
// REMOVED from Provider trait:
async fn structured_completion(&self, model, query) -> Result<Value>;
async fn chat_completion(&self, model, query) -> Result<Vec<Event>>;
```

The trait retains only:

```rust
#[async_trait]
pub trait Provider: Debug + Send + Sync {
    async fn model_details(&self, name: &Name) -> Result<ModelDetails>;
    async fn models(&self) -> Result<Vec<ModelDetails>>;
    async fn chat_completion_stream(&self, model, query) -> Result<EventStream>;
}
```

`chat_completion` was a convenience that collected the stream. Callers that
need collected results use the stream directly:

```rust
let stream = provider.chat_completion_stream(&model, query).await?;
let events: Vec<Event> = stream.try_collect().await?;
```

The following modules and types are also removed:

| Item                           | Location                         |
|--------------------------------|----------------------------------|
| `StructuredQuery`              | `jp_llm/src/query/structured.rs` |
| `structured::completion()`     | `jp_llm/src/structured.rs`       |
| `structured::titles::titles()` | `jp_llm/src/structured/titles.rs`|
| `SCHEMA_TOOL_NAME`             | `jp_llm/src/structured.rs`       |
| `handle_structured_output()`   | `jp_cli/src/cmd/query.rs`        |

---

## Event Builder Changes

### New `IndexBuffer` Variant

`EventBuilder` gets a new buffer variant for structured response parts:

```rust
enum IndexBuffer {
    Reasoning { content: String },
    Message { content: String },
    ToolCall { request: ToolCallRequest },
    Structured { content: String }, // NEW
}
```

When `handle_part` receives a `ChatResponse::Structured` event, it extracts
the `Value::String` content and appends to the buffer:

```rust
fn handle_part(&mut self, index: usize, event: &ConversationEvent) {
    match &event.kind {
        // ... existing variants ...

        EventKind::ChatResponse(ChatResponse::Structured { data }) => {
            let chunk = match data {
                Value::String(s) => s.as_str(),
                _ => {
                    warn!("Structured part with non-string value");
                    return;
                }
            };

            match self.buffers.entry(index) {
                Entry::Occupied(mut e) => e.get_mut().append(&event.kind),
                Entry::Vacant(e) => {
                    e.insert(IndexBuffer::Structured {
                        content: chunk.to_owned(),
                    });
                }
            }
        }
    }
}
```

### Flush Behavior

On flush, the accumulated string is parsed into a `Value`:

```rust
fn handle_flush(&mut self, index: usize, metadata: IndexMap<String, Value>,
                stream: &mut ConversationStream) {
    let Some(buffer) = self.buffers.remove(&index) else { return };

    let event = match buffer {
        // ... existing variants ...

        IndexBuffer::Structured { content } => {
            let data = serde_json::from_str::<Value>(&content)
                .unwrap_or_else(|e| {
                    warn!("Failed to parse structured response: {e}");
                    Value::String(content)
                });

            ConversationEvent::now(ChatResponse::Structured { data })
        }
    };

    let event = event.with_metadata(metadata);
    stream.push(event);
}
```

If JSON parsing fails, the raw string is preserved as `Value::String`. This
ensures no data loss even if the provider returns malformed JSON.

---

## Turn Loop Integration

### Streaming Phase

The `TurnCoordinator` receives `ChatResponse::Structured` parts during
streaming. It must NOT route them through `ChatResponseRenderer` (which
applies markdown formatting to `Message` and `Reasoning` variants).

Instead, `TurnCoordinator::handle_streaming_event` matches on the
`ChatResponse` variant and delegates to the appropriate renderer:

```rust
fn handle_streaming_event(
    &mut self,
    event: &Event,
    stream: &mut ConversationStream,
) -> Action {
    match event {
        Event::Part { index, event } => {
            match &event.kind {
                EventKind::ChatResponse(
                    resp @ (ChatResponse::Message { .. }
                           | ChatResponse::Reasoning { .. })
                ) => {
                    self.chat_renderer.render(resp);
                }
                EventKind::ChatResponse(
                    ChatResponse::Structured { .. }
                ) => {
                    self.structured_renderer.render_chunk(event);
                }
                EventKind::ToolCallRequest(_) => {
                    // ... existing tool call handling ...
                }
                _ => {}
            }

            self.event_builder.handle_part(*index, event);
            Action::Continue
        }
        // ... Flush, Finished handling unchanged ...
    }
}
```

### Rendering

Structured output is rendered as a fenced JSON code block. A minimal
`StructuredRenderer` handles this:

```rust
struct StructuredRenderer {
    printer: Arc<Printer>,
    started: bool,
}

impl StructuredRenderer {
    fn render_chunk(&mut self, event: &ConversationEvent) {
        let ChatResponse::Structured { data } = &event.kind else {
            return;
        };
        let Value::String(chunk) = data else { return };

        if !self.started {
            self.printer.print("```json\n");
            self.started = true;
        }

        self.printer.print(chunk);
    }

    fn flush(&mut self) {
        if self.started {
            self.printer.print("\n```\n");
            self.started = false;
        }
    }
}
```

The renderer:

1. On first chunk: prints ` ```json\n `
2. On each chunk: prints the raw JSON text
3. On flush/finish: prints ` \n``` `

No markdown parsing. No typewriter effect. Just raw JSON in a code fence.

### Post-Turn Extraction

After `run_turn_loop` completes, the caller extracts the structured result
from the persisted conversation events:

```rust
// In Query::run, after handle_turn returns:

if self.schema.is_some() {
    let events = workspace
        .get_events(&conversation_id)
        .expect("conversation must exist");

    let data = events
        .iter()
        .rev()
        .find_map(|e| e.as_chat_response())
        .and_then(|resp| resp.as_structured_data())
        .cloned()
        .ok_or(Error::MissingStructuredData)?;

    result = Ok(Success::Json(data));
}
```

For non-TTY output (piped), `Success::Json(data)` is formatted by the CLI
output layer — either pretty-printed (text format) or compact (JSON format).

---

## Helper Methods on `ChatResponse`

The existing `content()`, `content_mut()`, and `into_content()` methods on
`ChatResponse` must return `Option` variants now that `Structured` cannot return
`&str`.

New helper methods for structured data:

```rust
impl ChatResponse {
    pub const fn is_structured(&self) -> bool {
        matches!(self, Self::Structured { .. })
    }

    pub fn as_structured_data(&self) -> Option<&Value> {
        match self {
            Self::Structured { data } => Some(data),
            _ => None,
        }
    }

    pub fn into_structured_data(self) -> Option<Value> {
        match self {
            Self::Structured { data } => Some(data),
            _ => None,
        }
    }

    pub fn structured(data: impl Into<Value>) -> Self {
        Self::Structured {
            data: data.into(),
        }
    }
}
```

---

## Background Callers

Background tasks that need structured output do not go through
`run_turn_loop`. They use `ResilientRequest` + `chat_completion_stream`
directly.

### Title Generator

Currently uses `structured::completion()` →
`provider.structured_completion()`.

**New approach:**

```rust
// In TitleGeneratorTask::update_title

let provider = provider::get_provider(
    self.model_id.provider, &self.providers
)?;
let model = provider.model_details(&self.model_id.name).await?;

// Build the thread with title generation instructions.
let thread = ThreadBuilder::default()
    .with_events(self.events.clone())
    .with_instructions(title_instructions(count, &rejected))
    .build()?;

// Add a ChatRequest with schema as the last event.
let mut events = thread.events.clone();
events.add_chat_request(ChatRequest {
    content: "Generate titles for this conversation.".into(),
    schema: Some(title_schema(count)),
});

let query = ChatQuery {
    thread: Thread { events, ..thread },
    tools: vec![],
    tool_choice: ToolChoice::default(),
    tool_call_strict_mode: false,
};

// Use ResilientRequest for transport retries.
let resilient = ResilientRequest::new(provider.as_ref(), &request_config);
let stream = resilient
    .run(&model, query, &mut TurnState::default())
    .await?;
let events: Vec<Event> = stream.try_collect().await?;

// Extract the structured response.
let data = events
    .into_iter()
    .filter_map(Event::into_conversation_event)
    .find_map(|e| {
        e.into_chat_response()
            .and_then(ChatResponse::into_structured_data)
    })
    .ok_or("No structured response")?;

let titles: Vec<String> = serde_json::from_value(data)?;
```

The `titles()` helper function is replaced with simpler functions that
return the schema `Map<String, Value>` and instructions separately, instead
of building a `StructuredQuery`.

### Conversation Edit

Same pattern as title generator. The `generate_titles` function in
`conversation/edit.rs` builds a thread, adds a `ChatRequest` with schema,
uses `ResilientRequest` + `chat_completion_stream`, and extracts the result.

---

## Data Flow

### Interactive Query Flow

````
User: jp query --schema '{"type":"object","properties":{"name":{"type":"string"}}}' \
               "Extract the contact name from: Alice called Bob"

     │
     ▼
Query::run
     │
     │ stream.add_chat_request(ChatRequest {
     │     content: "Extract the contact name from: ...",
     │     schema: Some({"type": "object", ...}),
     │ })
     │
     ▼
handle_turn → run_turn_loop
     │
     │ build ChatQuery from thread
     │
     ▼
ResilientRequest::run
     │
     ▼
Provider::chat_completion_stream
     │
     │ create_request reads schema from last ChatRequest
     │ → sets output_config.format = json_schema (Anthropic)
     │ → converts ChatRequest.content to user message
     │ → sends request to LLM API
     │
     │ LLM streams JSON tokens:
     │
     ▼
Event stream:
     │
     │ Part { 0, ChatResponse::Structured { data: String("{\"name") } }
     │ Part { 0, ChatResponse::Structured { data: String("\": \"") } }
     │ Part { 0, ChatResponse::Structured { data: String("Alice\"}") } }
     │ Flush { 0 }
     │ Finished(Completed)
     │
     ▼
TurnCoordinator
     │
     ├──► StructuredRenderer: prints ```json\n{"name": "Alice"}\n```
     │
     └──► EventBuilder: accumulates "{\"name\": \"Alice\"}"
                │
                │ on flush: parse → Structured { data: {"name":"Alice"} }
                │ push to ConversationStream
                │
                ▼
          workspace.persist_active_conversation()
     │
     ▼
Query::run (post-turn)
     │
     │ extract structured data from events
     │ return Success::Json({"name": "Alice"})
     │
     ▼
CLI output layer
     │
     │ TTY:  (already rendered by StructuredRenderer)
     │ Pipe: {"name": "Alice"}
````

### Background Task Flow

```
TitleGeneratorTask::update_title
     │
     │ build Thread with events + instructions
     │ add ChatRequest { content: "...", schema: Some(title_schema) }
     │
     ▼
ResilientRequest::run
     │
     ▼
Provider::chat_completion_stream
     │
     │ detect schema on last ChatRequest → set output format
     │ stream JSON tokens
     │
     ▼
stream.try_collect::<Vec<Event>>()
     │
     ▼
find ChatResponse::Structured in collected events
     │
     │ serde_json::from_value::<Vec<String>>(data)
     │
     ▼
titles = ["Extracted Contact Names"]
```

### Persisted Event Stream

After a structured query, the conversation's `events.json` contains:

```json
[
  {
    "type": "chat_request",
    "content": "Extract the contact name from: Alice called Bob",
    "schema": {
      "type": "object",
      "properties": {
        "name": {
          "type": "string"
        }
      },
      "required": [
        "name"
      ]
    }
  },
  {
    "type": "chat_response",
    "data": {
      "name": "Alice"
    }
  }
]
```

### Multi-Turn Conversation

When a structured turn is followed by a normal turn:

```json
[
  {
    "type": "chat_request",
    "content": "Extract contacts",
    "schema": {
      "type": "object",
      "properties": {
        "name": {
          "type": "string"
        }
      }
    }
  },
  {
    "type": "chat_response",
    "data": {
      "name": "Alice"
    }
  },
  {
    "type": "chat_request",
    "content": "Now explain what you found"
  },
  {
    "type": "chat_response",
    "message": "I found one contact named Alice."
  }
]
```

When building the LLM request for the last turn, the provider:

1. Converts all `ChatRequest`s to user messages (content only, schema
   ignored for message text)
2. Converts `ChatResponse::Structured` to an assistant text message
   containing the JSON string — the LLM sees what it previously produced
3. Reads the schema from ONLY the last `ChatRequest` — since it has no
   schema, this is a normal (non-structured) request
4. Converts `ChatResponse::Message` to an assistant text message as usual

The LLM's message history for this request:

```
User: "Extract contacts"
Assistant: "{\"name\": \"Alice\"}"
User: "Now explain what you found"
```

---

## Error Handling

### Transport Errors

Handled by `ResilientRequest`, same as normal queries. Rate limits, timeouts,
and transient errors are retried automatically.

### Schema Compliance

With native structured output APIs, the provider **guarantees** the response
conforms to the schema. No client-side validation or retry is needed for
providers with native support.

If a provider does not support native structured output (detected by absence
of the feature in `ModelDetails`), the request should fail with a clear
error rather than silently falling back to the tool-call workaround:

```rust
if structured_schema.is_some()
    && !model.features.contains(&"structured-outputs")
{
    return Err(Error::StructuredOutputNotSupported {
        model: model.id.to_string(),
    });
}
```

This is a deliberate design choice. The tool-call fallback is removed
entirely. Providers that lack native support can add it over time.

### JSON Parse Failure

If the `EventBuilder` fails to parse the accumulated JSON on flush (e.g.
due to a truncated response from `FinishReason::MaxTokens`), it falls back
to storing the raw string as `Value::String`:

```rust
let data = serde_json::from_str::<Value>(&content)
    .unwrap_or_else(|_| Value::String(content));
```

The caller can detect this by checking if the resulting `Value` is a
`String` when it expected an `Object`. This preserves the raw response for
debugging.

---

## Testing Strategy

### Unit Tests

**Serialization round-trip:**

```rust
#[test]
fn test_chat_request_with_schema_roundtrip() {
    let request = ChatRequest {
        content: "Extract contacts".into(),
        schema: Some(Map::from_iter([
            ("type".into(), json!("object")),
        ])),
    };

    let json = serde_json::to_value(&request).unwrap();
    assert_eq!(json["content"], "Extract contacts");
    assert_eq!(json["schema"]["type"], "object");

    let deserialized: ChatRequest = serde_json::from_value(json).unwrap();
    assert_eq!(deserialized, request);
}

#[test]
fn test_chat_request_without_schema_omits_field() {
    let request = ChatRequest {
        content: "hello".into(),
        schema: None,
    };

    let json = serde_json::to_value(&request).unwrap();
    assert!(json.get("schema").is_none());
}

#[test]
fn test_structured_response_roundtrip() {
    let event = ConversationEvent::now(
        ChatResponse::structured(json!({"name": "Alice"}))
    );

    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["data"]["name"], "Alice");

    let deserialized: ConversationEvent =
        serde_json::from_value(json).unwrap();
    let resp = deserialized.as_chat_response().unwrap();
    assert!(resp.is_structured());
    assert_eq!(
        resp.as_structured_data(),
        Some(&json!({"name": "Alice"}))
    );
}
```

**EventBuilder — structured response accumulation:**

```rust
#[test]
fn test_structured_response_accumulation() {
    let mut stream = ConversationStream::default();
    let mut builder = EventBuilder::new();

    builder.handle_part(0, &ConversationEvent::now(
        ChatResponse::Structured {
            data: Value::String("{\"name".into()),
        }
    ));
    builder.handle_part(0, &ConversationEvent::now(
        ChatResponse::Structured {
            data: Value::String("\": \"Alice\"}".into()),
        }
    ));
    builder.handle_flush(0, IndexMap::new(), &mut stream);

    let event = stream.last().unwrap();
    let resp = event.as_chat_response().unwrap();
    assert_eq!(
        resp.as_structured_data(),
        Some(&json!({"name": "Alice"})),
    );
}
```

**EventBuilder — malformed JSON fallback:**

```rust
#[test]
fn test_structured_response_malformed_json() {
    let mut stream = ConversationStream::default();
    let mut builder = EventBuilder::new();

    builder.handle_part(0, &ConversationEvent::now(
        ChatResponse::Structured {
            data: Value::String("{\"truncated".into()),
        }
    ));
    builder.handle_flush(0, IndexMap::new(), &mut stream);

    let event = stream.last().unwrap();
    let resp = event.as_chat_response().unwrap();
    // Falls back to raw string
    assert_eq!(
        resp.as_structured_data(),
        Some(&Value::String("{\"truncated".into())),
    );
}
```

**Provider — schema detection from last ChatRequest:**

```rust
#[test]
fn test_schema_read_from_last_chat_request() {
    let mut events = ConversationStream::default();

    // Historical request WITH schema
    events.add_chat_request(ChatRequest {
        content: "old query".into(),
        schema: Some(Map::from_iter([
            ("type".into(), json!("array")),
        ])),
    });

    // Current request WITHOUT schema
    events.add_chat_request(ChatRequest {
        content: "new query".into(),
        schema: None,
    });

    let schema = events
        .iter()
        .rev()
        .find_map(|e| e.as_chat_request())
        .and_then(|req| req.schema.clone());

    // Only the last ChatRequest is checked
    assert!(schema.is_none());
}
```

**Provider — structured response in history converts to assistant message:**

```rust
#[test]
fn test_structured_response_converts_to_assistant_message() {
    let mut events = ConversationStream::default();
    events.add_chat_request("Extract contacts");
    events.push(ConversationEvent::now(
        ChatResponse::structured(json!({"name": "Alice"}))
    ));
    events.add_chat_request("follow-up");

    let messages = convert_events(events);
    assert_eq!(messages.len(), 3);
    // messages[0]: User "Extract contacts"
    // messages[1]: Assistant "{\"name\":\"Alice\"}"
    // messages[2]: User "follow-up"
}
```

### Integration Tests

**Full turn with structured output (mock provider):**

```rust
#[tokio::test]
async fn test_structured_output_through_turn_loop() {
    // Mock provider that returns ChatResponse::Structured parts
    let provider = MockProvider::with_structured_response(
        json!({"name": "Alice", "email": "alice@example.com"})
    );

    // ... set up workspace, conversation, etc. ...

    // Add ChatRequest with schema
    workspace.get_events_mut(&conv_id).unwrap()
        .add_chat_request(ChatRequest {
            content: "Extract contacts".into(),
            schema: Some(contact_schema()),
        });

    run_turn_loop(/* ... */).await.unwrap();

    // Verify persistence
    let events = workspace.get_events(&conv_id).unwrap();
    let resp = events.iter().rev()
        .find_map(|e| e.as_chat_response())
        .unwrap();

    assert_eq!(
        resp.as_structured_data(),
        Some(&json!({"name": "Alice", "email": "alice@example.com"})),
    );
}
```

---

## Migration Path

### Phase 1: Extend `ChatRequest` and `ChatResponse`

1. Add `schema: Option<Map<String, Value>>` to `ChatRequest` with
   `#[serde(default, skip_serializing_if = "Option::is_none")]`
2. Add `Structured { data: Value }` variant to `ChatResponse`
3. Add helper methods: `ChatResponse::is_structured()`,
   `as_structured_data()`, `into_structured_data()`, `structured()`
4. Update `content()` / `content_mut()` / `into_content()` to handle
   the `Structured` variant (return empty string)
5. Add serialization round-trip tests
6. Update snapshot tests

### Phase 2: Event Builder

1. Add `IndexBuffer::Structured` variant
2. Implement `handle_part` for `ChatResponse::Structured` events
3. Implement flush behavior with JSON parsing + fallback
4. Add unit tests

### Phase 3: Provider — Schema Detection and Event Conversion

1. Add schema detection in each provider's `create_request` (read from
   last `ChatRequest.schema`)
2. Set native structured output config for each provider
3. Add `is_structured` flag to provider streaming logic
4. Emit `ChatResponse::Structured` parts instead of
   `ChatResponse::Message` when structured
5. Update `convert_events` to handle `ChatResponse::Structured` as an
   assistant text message
6. Add unit tests for each provider

Provider order (by usage priority):
1. Anthropic
2. Google
3. OpenAI
4. Ollama
5. Llamacpp
6. OpenRouter

### Phase 4: Turn Loop Integration

1. Add `StructuredRenderer` to `jp_cli`
2. Update `TurnCoordinator::handle_streaming_event` to detect
   `ChatResponse::Structured` and delegate to `StructuredRenderer`
3. Flush the `StructuredRenderer` on stream finish
4. Add post-turn extraction logic to `Query::run`
5. Add integration tests

### Phase 5: Remove Old Code

1. Remove `handle_structured_output` from `query.rs`
2. Remove `Provider::structured_completion` from the trait
3. Remove `Provider::chat_completion` from the trait
4. Remove `StructuredQuery` (`jp_llm/src/query/structured.rs`)
5. Remove `structured::completion()` (`jp_llm/src/structured.rs`)
6. Remove `structured::titles::titles()` and the `titles` module
7. Remove `SCHEMA_TOOL_NAME`
8. Update `Query::run` to set `schema` on `ChatRequest` and call
   `handle_turn` for both paths

### Phase 6: Background Callers

1. Update `TitleGeneratorTask` to use `ResilientRequest` +
   `chat_completion_stream` + `ChatRequest` with schema
2. Update `conversation/edit.rs` `generate_titles` similarly
3. Extract shared title schema + instructions into a helper (replacing
   the `titles()` function)
4. Add unit tests

### Phase 7: Cleanup

1. Remove `tool_call_strict_mode` from `ChatQuery` (no longer needed —
   providers should always use strict tool calls if supported)
2. Update architecture documentation (`index.md`)
3. Run full test suite, fix any regressions
4. Remove any remaining dead code (`#[allow(dead_code)]` markers)
