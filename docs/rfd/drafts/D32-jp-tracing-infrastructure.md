# RFD D32: JP Tracing Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Extends**: [RFD D15]
- **Summary**: Introduces a `jp_trace` crate with typed tracing events,
  execution spans, test capture, and content-addressed payload storage.

## Summary

This RFD introduces a typed tracing system for JP, built on `tracing`.
It replaces ad-hoc `tracing::info!(...)` calls with typed event structs, adds
span-based execution context, and establishes a two-channel output model:
structured tracing for developers (always written to the log file) and chrome
for users (controlled by `-v`).
A new `jp_trace` crate owns subscriber configuration, the `emit!` macro, a test
capture API, and content-addressed blob storage for large payloads.

## Motivation

JP's tracing is ad-hoc.
Every crate calls `tracing::info!("some message", field = value)` with free-form
strings and inconsistent field names.
The codebase has no spans: not a single `#[instrument]` or `tracing::span!()`,
so errors propagate through `?` without recording which span they occurred in.
Large payloads (LLM request bodies) are dumped to temp files via a one-off
`trace_to_tmpfile()` helper, and a 150-line function in `jp_cli::lib` configures
the subscriber while mixing verbosity semantics, file writing, and stderr
formatting.

This creates five problems:

1. **No structure.** Tracing output is only useful to someone who already knows
   the codebase.
   A `warn!("retrying")` in one provider looks different from `warn!("retry")`
   in another, so there is no way to query all retry events or all events from
   the Anthropic provider without grepping source code.

2. **No hierarchy.** Events form a flat stream with no parent-child
   relationships.
   Without spans, there is no way to see that an error occurred during the third
   retry of the second turn of a query against `claude-sonnet-4`, and
   post-mortem debugging requires reconstructing the call chain by hand.

3. **No test observability.** Tests cannot assert that a code path emitted a
   retry event without capturing stderr output and matching strings.
   Typed events and a test capture API let tests assert directly on traced
   behavior.

4. **Mixed audiences.** The `-v` flag controls both user-facing status ("what is
   JP doing?") and developer tracing (internal state and protocol details).
   Users running `jp -vvv` are flooded with implementation details they cannot
   act on, while developers who keep `JP_DEBUG=1` set get tracing noise mixed
   into their terminal.

5. **Fragile large-payload handling.** `trace_to_tmpfile()` writes to `/tmp`
   with no cleanup, no connection to the log directory, and no deduplication,
   and each provider re-implements the pattern.

As JP adds agentic workflows, server integrations, and plugins, these gaps grow
harder to retrofit.
A typed tracing system establishes structure and audience separation before the
surface area expands further.

## Design

### Two-channel output model

JP separates output into two channels with distinct audiences:

| Channel | Audience   | Controls              | Content                              |
| ------- | ---------- | --------------------- | ------------------------------------ |
| Chrome  | Users      | `-v` / `-q`           | Status lines, progress, tool headers |
| Tracing | Developers | `JP_LOG` / `JP_DEBUG` | Typed events, spans, structured data |

**Chrome** is user-facing status written to stderr via `Printer`.
CLI command code authors each line explicitly, controlling its wording and
format, and the `-v` / `-vv` / `-vvv` flags control its verbosity.

**Tracing** is structured diagnostic data written to a JSON log file.
Every typed event and span is recorded at TRACE level regardless of CLI flags,
and tracing output never appears on stderr unless a developer opts in via
`JP_LOG`.

When an event matters to both audiences (such as a rate-limit retry), the call
site emits both independently:

```rust
printer.chrome_v(format!("⟳ Retrying ({attempt}/{max})…"));
emit!(events::Retrying { attempt, max, backoff, kind });
```

Chrome receives a formatted status line while tracing records a structured event
with all fields.

### Verbosity and environment variables

All tracing controls use environment variables rather than CLI flags, keeping
`jp --help` free of developer-only options.

| Variable                  | Purpose                                   | Default                              |
| ------------------------- | ----------------------------------------- | ------------------------------------ |
| `JP_DEBUG=1`              | Prints the log path before execution and  | off                                  |
|                           | repeats it after five minutes.            |                                      |
| `JP_LOG=<filter>`         | Enables mirroring tracing to stderr,      | off                                  |
|                           | filtered by the `EnvFilter` expression.   |                                      |
| `JP_LOG_FILE=<path>`      | Overrides the default log file location.  | `~/.local/share/jp/logs/…`           |
| `JP_LOG_FORMAT=text/json` | Controls the format of the stderr mirror. | `auto` (text on TTY, JSON otherwise) |

CLI flags control chrome verbosity only:

| Flag                  | Effect                                |
| --------------------- | ------------------------------------- |
| `-v` / `-vv` / `-vvv` | Increases chrome verbosity. Does not  |
|                       | affect tracing.                       |
| `-q`                  | Suppresses chrome. Leaves tracing,    |
|                       | the log file, and `JP_DEBUG`'s report |
|                       | alone.                                |

The log file always records at TRACE level; no flag or variable reduces its
verbosity, so users always have a complete log to attach when filing issues.

For example:

```sh
# Regular user: chrome only, complete log in background
jp query "fix the bug"

# User with JP_DEBUG always set: prints the log path before execution
# and repeats it when the run lasts at least five minutes
JP_DEBUG=1 jp query "fix the bug"

# Developer wants live tracing for a specific run
JP_LOG=info,jp_llm=trace jp query "fix the bug"

# Verbose chrome + live tracing (orthogonal, combinable)
JP_LOG=warn jp -vv query "fix the bug"
```

This two-channel model replaces the `-v` through `-vvvvv` tracing verbosity
levels in [RFD D15], while keeping [RFD D15]'s log file plumbing (persistent
directory, deferred path resolution, and in-memory buffering) unchanged.

### The `jp_trace` crate

A new `jp_trace` workspace crate owns JP's tracing infrastructure:

```txt
crates/jp_trace/
├── src/
│   ├── lib.rs          // `Emit` trait, `emit!` macro re-export
│   ├── blob.rs         // Content-addressed large-payload storage
│   ├── configure.rs    // Subscriber construction (moved from `jp_cli::lib`)
│   ├── testing.rs      // Task-local test capture API
│   └── events/
│       └── common.rs   // Cross-cutting event types (HTTP, process, I/O)
```

`jp_trace` depends on `tracing` and `tracing-subscriber`, and on no `jp_*`
domain crate.
Domain crates depend on `jp_trace` for the `Emit` trait and `emit!` macro.

### Typed events

Each crate that emits trace events defines them in `src/trace.rs` inside a
`pub(crate) mod events` module, giving the crate a single place that lists its
observable behavior.
Each event is a `pub(crate)` struct with typed fields and an `Emit`
implementation, so only the owning crate can emit its events:

```rust
// crates/jp_llm/src/trace.rs

pub(crate) mod events {
    use std::time::Duration;
    use jp_config::model::id::ProviderId;
    use jp_llm::error::StreamErrorKind;
    use jp_trace::{Emit, Blob};

    pub(crate) struct RequestSent {
        pub payload: Blob,
        pub tokens_in: Option<usize>,
    }

    pub(crate) struct Retrying {
        pub attempt: u32,
        pub max: u32,
        pub backoff: Duration,
        pub kind: StreamErrorKind,
    }

    pub(crate) struct StreamErrorOccurred {
        pub kind: StreamErrorKind,
        pub message: String,
        pub retryable: bool,
    }

    impl Emit for RequestSent {
        fn emit(self, file: &'static str, line: u32) {
            let Self { payload, tokens_in } = self;
            tracing::debug!(
                target: "jp_llm::request_sent",
                caller.file = file,
                caller.line = line,
                payload = %payload,
                ?tokens_in,
                "LLM request sent"
            );
        }
    }

    impl Emit for Retrying {
        fn emit(self, file: &'static str, line: u32) {
            let Self { attempt, max, backoff, kind } = self;
            tracing::warn!(
                target: "jp_llm::retrying",
                caller.file = file,
                caller.line = line,
                attempt,
                max,
                backoff_ms = backoff.as_millis() as u64,
                kind = %kind,
                "LLM retry"
            );
        }
    }

    // ... further Emit impls
}
```

Event structs require no traits beyond `Sized`, so they can carry any domain
data, including error types and non-copyable values.

A small set of `pub` events in `jp_trace::events::common` cover shared
primitives that no single domain crate owns: HTTP requests and responses,
process lifecycle, and file I/O operations.

### The `Emit` trait and `emit!` macro

The `Emit` trait is the interface between event structs and the `tracing`
subscriber:

```rust
// crates/jp_trace/src/lib.rs

pub trait Emit: Sized {
    fn emit(self, file: &'static str, line: u32);
}
```

Call sites never invoke `Emit::emit` directly.
The `emit!` macro captures the caller's source location and invokes the test
recorder:

```rust
#[macro_export]
macro_rules! emit {
    ($event:expr) => {{
        let event = $event;
        #[cfg(test)]
        $crate::testing::maybe_record(&event);
        $crate::Emit::emit(event, ::core::file!(), ::core::line!())
    }};
}
```

The macro serves two purposes: it evaluates `file!()` and `line!()` at the call
site rather than inside the `Emit` implementation (at zero runtime cost), and it
invokes the test recorder in `#[cfg(test)]` builds.

The default JSON log always records `caller.file` and `caller.line` as
structured event fields rather than `tracing` callsite metadata, though output
formatters can strip them.

### Typed spans

Spans carry shared context that events emitted inside them inherit automatically
from the `tracing` subscriber.
Like events, each span is a `pub(crate)` function in the crate's `trace` module
returning a `tracing::Span`, defined and entered by the crate that owns the
execution boundary:

```rust
// crates/jp_cli/src/trace.rs

use jp_conversation::ConversationId;
use jp_config::model::id::ProviderId;
use jp_llm::model::ModelId;

pub(crate) fn cmd_span(name: &str, invocation_id: &ulid::Ulid) -> tracing::Span {
    tracing::info_span!("cmd", name, invocation_id = %invocation_id)
}

pub(crate) fn query_span(conversation_id: &ConversationId) -> tracing::Span {
    tracing::info_span!("query", conversation_id = %conversation_id)
}

pub(crate) fn turn_span(number: usize) -> tracing::Span {
    tracing::info_span!("turn", number)
}
```

```rust
// crates/jp_llm/src/trace.rs

pub(crate) fn stream_span(provider: ProviderId, model: &str) -> tracing::Span {
    tracing::info_span!("llm_stream", %provider, model)
}

pub(crate) fn tool_execution_span(tool: &str) -> tracing::Span {
    tracing::info_span!("tool_execution", tool)
}
```

Call sites enter spans synchronously or attach them to futures:

```rust
let _guard = trace::cmd_span(&cmd_path, &invocation_id).entered();

// For async code:
let stream = provider.chat_completion_stream(model, query)
    .instrument(trace::stream_span(provider_id, &model.name))
    .await?;
```

The initial span set covers the critical path:

| Span                   | Crate          | Fields                  | Scope                             |
| ---------------------- | -------------- | ----------------------- | --------------------------------- |
| `cmd`                  | `jp_cli`       | `name`, `invocation_id` | One per JP invocation. Root span. |
| `query`                | `jp_cli`       | `conversation_id`       | Duration of the query command.    |
| `turn`                 | `jp_cli`       | `number`                | One turn within a query.          |
| `llm_stream`           | `jp_llm`       | `provider`, `model`     | One LLM provider call.            |
| `tool_execution`       | `jp_llm`       | `tool`                  | One tool invocation.              |
| `conversation_persist` | `jp_workspace` | `conversation_id`       | Conversation save to disk.        |

The `cmd` span carries a ULID correlation ID (`invocation_id`) that uniquely
identifies a single `jp` invocation, allowing a log file to be filtered to one
run (`jq 'select(.spans[] | .invocation_id == "01HXZ...")'`).
Because the subscriber attaches active span fields automatically, events emitted
inside an `llm_stream` span do not repeat `provider` or `model`.

### Blob storage for large payloads

The `Blob` type replaces `trace_to_tmpfile()`.
At construction time, it either keeps small values inline or writes larger
payloads to a sidecar file:

```rust
// crates/jp_trace/src/blob.rs

pub struct Blob {
    repr: BlobRepr,
}

enum BlobRepr {
    Inline(String),
    Stored(Utf8PathBuf),
    Failed,
}

impl Blob {
    /// Serialize `value` as JSON. If the result exceeds `INLINE_THRESHOLD`
    /// bytes, write it to a content-addressed sidecar file under the log
    /// directory. Otherwise, keep it inline.
    pub fn json(label: &'static str, value: &impl Serialize) -> Self {
        let bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
        if bytes.len() < INLINE_THRESHOLD {
            return Self { repr: BlobRepr::Inline(String::from_utf8_lossy(&bytes).into_owned()) };
        }

        let hash = sha256_hex(&bytes);
        let dir = blob_dir();
        let path = dir.join(format!("{label}-{hash}.json"));

        if path.exists() {
            // Content-addressed: identical payloads reuse the same file.
            return Self { repr: BlobRepr::Stored(path) };
        }

        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes)) {
            Ok(()) => Self { repr: BlobRepr::Stored(path) },
            Err(_) => Self { repr: BlobRepr::Failed },
        }
    }
}

impl fmt::Display for Blob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            BlobRepr::Inline(s) => f.write_str(s),
            BlobRepr::Stored(path) => write!(f, "blob:{path}"),
            BlobRepr::Failed => f.write_str("<blob write failed>"),
        }
    }
}
```

Payloads at or above `INLINE_THRESHOLD` (4 KB) are written to
`~/.local/share/jp/logs/blobs/`, colocated with the log files from [RFD D15].
Content-addressed filenames (keyed by the payload's SHA-256 digest) deduplicate
identical request bodies across retries.
Event structs store `Blob` fields and format them via `Display`, writing either
the inline JSON or a `blob:<path>` reference.

### Plugin tracing

Plugin processes produce two kinds of trace data, both represented as typed
events: raw stderr lines and structured log messages from the JSON-RPC protocol:

```rust
// crates/jp_plugin/src/trace.rs

pub(crate) fn plugin_span(id: &str) -> tracing::Span {
    tracing::info_span!("plugin", id)
}

pub(crate) mod events {
    pub(crate) struct StderrLine {
        pub line: String,
    }

    pub(crate) struct LogMessage {
        pub level: tracing::Level,
        pub message: String,
        pub fields: serde_json::Value,
    }

    pub(crate) struct Started {
        pub pid: u32,
    }

    pub(crate) struct Exited {
        pub status_code: Option<i32>,
        pub duration: std::time::Duration,
    }

    pub(crate) struct ProtocolError {
        pub error: String,
    }
}
```

Events inside `plugin_span` inherit the plugin `id`, so `jq 'select(.spans[] |
.id == "jp-path")'` filters a log file to a single plugin.
`LogMessage` dispatches to the matching `tracing` level in its `Emit`
implementation.

### Test capture API

The test capture API uses `tokio::task_local!` to scope a recorder to a test
body, storing event type identity rather than field values:

```rust
// crates/jp_trace/src/testing.rs

use std::any::TypeId;
use std::sync::Mutex;

struct RecordedEvent {
    type_id: TypeId,
    type_name: &'static str,
}

tokio::task_local! {
    static RECORDER: Mutex<Vec<RecordedEvent>>;
}

/// Capture all events emitted during `f`.
pub async fn capture<F, R>(f: F) -> (R, Captured)
where
    F: std::future::Future<Output = R>,
{
    let recorder = Mutex::new(Vec::new());
    let result = RECORDER.scope(recorder, f).await;
    // After scope completes, the task-local is consumed.
    // Events are extracted from the recorder.
    todo!("extract events from task-local")
}

/// Called by the `emit!` macro. Noop when no recorder is installed.
pub fn maybe_record<E: 'static>(_event: &E) {
    let _ = RECORDER.try_with(|r| {
        if let Ok(mut vec) = r.lock() {
            vec.push(RecordedEvent {
                type_id: TypeId::of::<E>(),
                type_name: std::any::type_name::<E>(),
            });
        }
    });
}
```

The `Captured` type provides type-level assertion helpers:

```rust
pub struct Captured {
    events: Vec<RecordedEvent>,
}

impl Captured {
    /// Returns the number of captured events of type `E`.
    pub fn count<E: 'static>(&self) -> usize { /* ... */ }

    /// Returns true if any event of type `E` was captured.
    pub fn contains<E: 'static>(&self) -> bool { /* ... */ }

    /// Returns the total number of captured events.
    pub fn len(&self) -> usize { /* ... */ }
}
```

Tests wrap the future under test in `jp_trace::testing::capture`:

```rust
#[tokio::test]
async fn retries_on_rate_limit() {
    let (result, events) = jp_trace::testing::capture(async {
        // ... set up mock provider, run the streaming call
    }).await;

    assert!(result.is_ok());
    assert_eq!(events.count::<trace::events::Retrying>(), 2);
    assert!(events.contains::<trace::events::StreamErrorOccurred>());
}
```

Following Vector's approach, tests assert on event types and counts while
verifying field values through return values and observable side effects.
Task-local storage isolates parallel tests without a global mutex.
Tasks spawned within the same `tokio::task::LocalSet` share the recorder,
whereas tasks spawned via `tokio::spawn` do not; tests that need cross-task
capture can use `LocalSet` or assert on events from the coordinating task.

### Subscriber construction

`configure_logging` and `TracingGuard` move from `jp_cli::lib` to
`jp_trace::configure`.
`jp_trace::configure` reads the environment variables in [Verbosity and
environment variables](#verbosity-and-environment-variables) and builds the
subscriber stack:

```txt
┌────────────────────────────────────┐
│         tracing-subscriber         │
│            registry                │
│                                    │
│  ┌──────────────────────────────┐  │
│  │  File layer (JSON, TRACE)    │  │
│  │  Always active.              │  │
│  └──────────────────────────────┘  │
│                                    │
│  ┌──────────────────────────────┐  │
│  │  Stderr layer (optional)     │  │
│  │  Enabled by JP_LOG.          │  │
│  │  Filtered by JP_LOG expr.    │  │
│  │  Format from JP_LOG_FORMAT.  │  │
│  └──────────────────────────────┘  │
└────────────────────────────────────┘
```

The file layer writes JSON to the log directory from [RFD D15], keeping its
in-memory buffering layer for deferred file path resolution unchanged.
The stderr layer is installed only when `JP_LOG` is set.
`jp_cli::lib` calls `jp_trace::configure(...)` at startup and receives a
`TracingGuard`, whose flushing and persistence behavior on drop is unchanged
from [RFD D15].

## Drawbacks

**Boilerplate per event.** Every typed event requires a struct definition and an
`Emit` implementation with a hand-written `tracing::event!()` call (roughly 200
lines for a crate with 15 events).
Avoiding a derive macro keeps the code simple and easy to debug; if a crate
grows past ~50 events, a derive macro can be added later without changing the
external API.

**Two representations for dual-audience events.** When an event matters to both
users and developers, the call site makes both a chrome call and an `emit!()`
call.
Because chrome is curated UX and tracing is structured data, the two can drift:
a retry path might update its chrome message without updating the event struct's
fields, or vice versa.
Code review mitigates this risk.

**Migration cost.** Converting existing `tracing::info!(...)` calls across ~30
crate-level call sites requires defining a struct, implementing `Emit`, and
updating each call site.
The work is mechanical but touches many files.

**No field-level test assertions on events.** Because the test capture API
records event type identity rather than field values, tests cannot assert on
specific event fields (such as `attempt == 2`) through `Captured`.
Field values are tested through return values and side effects instead; if
needed, per-event opt-in value recording can be added later.

**CLI flag removal.** Removing `--log-file`, `--log-filter`, and `--log-format`
in favor of environment variables breaks shell aliases or scripts that pass
these flags.
The flags targeted developers and were not widely advertised, so a changelog
note is sufficient.

## Alternatives

### Keep ad-hoc tracing calls

Continuing to call `tracing::info!("message", field = value)` across the
codebase avoids migration cost and boilerplate, but leaves all five problems in
[Motivation](#motivation) unresolved: no test observability, inconsistent field
names, and mixed user and developer output.

### Derive macro for `Emit`

An `Emit` implementation could be generated from struct attributes:

```rust
#[derive(TraceEvent)]
#[trace(level = "warn", target = "jp_llm::retrying")]
pub(crate) struct Retrying {
    pub attempt: u32,
    // ...
}
```

A derive macro reduces boilerplate but adds a proc-macro dependency, increases
compile times, and obscures which fields are emitted.
Hand-written implementations are verbose but transparent, and a derive macro can
be introduced later if event counts grow past ~50 per crate.

### Enum-based event hierarchy

Events could be grouped into namespace structs with a `kind` enum:

```rust
pub struct LlmEvent {
    pub provider: ProviderId,
    pub model: ModelId,
    pub kind: LlmEventKind,
}
```

This reduces the number of top-level types, but duplicates context fields
(`provider`, `model`) that active spans already attach automatically.
Flat event structs combined with spans avoid this duplication.

### `#[instrument]` instead of typed spans

`tracing`'s `#[instrument]` attribute generates spans from function signatures,
but requires noisy `skip(...)` annotations, leaks internal parameter names into
trace output, and ties spans to functions rather than architectural boundaries.
Explicit span functions give direct control over span fields and lifetimes.

### Centralized event definitions

Defining all events in `jp_trace::events::*` (like Vector's
`src/internal_events/`) puts every event in one crate, but either turns
`jp_trace` into a god-crate depending on every domain crate or forces domain
types into `jp_trace`.
Per-crate `trace::events` modules avoid both problems.

### `RUST_LOG` instead of `JP_LOG`

`RUST_LOG` is the standard environment variable for `EnvFilter` expressions, but
JP already uses the `JP_*` namespace (`JP_DEBUG`), and `RUST_LOG` would also
enable noisy logging from third-party crates (`reqwest`, `hyper`, `tokio`).
`JP_LOG` gives JP control over the default filter baseline while preserving
standard `EnvFilter` syntax.

## Non-Goals

- **Chrome verbosity API.** The two-channel model commits to chrome as the
  user-facing channel, but `Printer` API additions (such as `chrome_v()` and
  `chrome_vv()`) and the assignment of status lines to verbosity levels belong
  to a separate RFD.
- **Error emission convention.** This RFD provides `emit!` and typed events; a
  separate RFD defines when and where to emit trace events as errors are
  constructed.
- **Log rotation and cleanup.** Deferred by [RFD D15]; the log and blob
  directories accumulate files until rotation is implemented.
- **Pretty-printing and `jp-log` command plugin.** A command plugin for
  rendering log files as hierarchical trees is a separate effort built on [RFD
  072].
- **OpenTelemetry SDK and network export.** Depending on `opentelemetry`,
  `opentelemetry_sdk`, or `tracing-opentelemetry` to export telemetry over
  OTLP/gRPC or OTLP/HTTP is out of scope: a single-user CLI has no collector to
  ship to, so the SDK's exporters, batching processors, and runtime add cost
  without benefit.
  File encoding is a separate question (see [Log file
  encoding](#log-file-encoding)).
- **Metrics.** Unlike Vector's `InternalEvent` system, this RFD does not add
  metric counters (request counts, error rates, token usage).
  If metrics are needed later, `Emit` can be extended to record them alongside
  trace events.

> [!TIP]
> [RFD D34] versions JP's machine-readable output behind a `{format, version,
> data}` envelope and excludes tracing because JP does not currently commit to a
> stable trace event schema.
> Once trace events are JP-owned structs rather than `tracing-subscriber`'s
> default serialization, this RFD (or a follow-up) can decide whether they
> become versioned machine records with format addresses; [RFD D34] neither
> requires nor precludes that.

## Risks and Open Questions

### Task-local recorder and `tokio::spawn`

Because `tokio::task_local!` does not propagate across `tokio::spawn`
boundaries, events emitted on independently spawned tasks (such as background
plugin process monitors) are not captured by `jp_trace::testing::capture`.
Tests that need cross-task capture can run on a `LocalSet` or assert on events
emitted by the coordinating task; if that proves too limiting, the recorder can
fall back to a global map keyed by test ID (similar to `tracing-test`).

### `Blob` writes on the hot path

`Blob::json()` hashes and writes payloads synchronously, which could block the
Tokio runtime if called frequently on an async hot path.
In practice, blob writes occur once per LLM provider request for payloads of a
few hundred kilobytes, and can be moved to `spawn_blocking` if they ever show up
in profiles.

### Migration ordering

Converting a crate's tracing calls to typed events requires `jp_trace` to exist,
while moving `configure_logging` into `jp_trace::configure` must not break
subscriber initialization in `jp_cli`.
The implementation plan separates these steps: Phase 1 extracts the crate and
subscriber setup before Phase 3 and Phase 4 migrate events.

### Span overhead

The subscriber records active span context on every event.
With six spans on the critical path, this overhead is negligible; if the span
count grows substantially, profiling should confirm that per-event cost stays
low.

### Log file encoding

The file layer currently uses `tracing-subscriber`'s bespoke JSON format, which
requires custom tooling to read.

An alternative is [OTLP JSON encoding][OTLP JSON], which defines a stable schema
for traces, metrics, and logs, and which the [file exporter spec][OTLP file]
stores as JSON Lines (matching the file layer's one-record-per-line structure).
Because `jp_trace` owns serialization, it can emit OTLP JSON directly without
depending on the OpenTelemetry SDK.

Adopting OTLP JSON enables existing tools: `otel-tui --from-json-file` renders
trace waterfalls directly, `otel-desktop-viewer` loads files into DuckDB for SQL
queries, and the OpenTelemetry Collector's JSON file receiver bridges to Jaeger
or Tempo.
For an assistant querying its own traces, a standard schema with existing query
tools is more useful than a bespoke format.

The trade-off is verbosity and file splitting.
OTLP JSON encodes attributes as `[{key, value: {stringValue}}]` arrays and
timestamps as nanosecond strings, making raw files harder to read by hand or
filter with `jq`.
In addition, the file exporter specification is still in Development status and
requires one signal type per file, splitting spans and events across separate
files linked by `traceId` and `spanId`.

Because OTLP trace viewers are only useful once Phase 2's spans exist, this
decision can wait until after Phase 2.
Phase 1 only needs to keep the file layer's writer internal to `jp_trace` so the
line format can change later without touching call sites.

### `caller.file` paths in release builds

`file!()` expands to a path from the build environment, which can leak the
builder's directory structure in distributed binaries.
This is acceptable while users build from source, and release builds can strip
build prefixes with `--remap-path-prefix` in `RUSTFLAGS` when binary
distribution is added.

## Implementation Plan

### Phase 1: `jp_trace` crate and subscriber extraction

Create the `jp_trace` crate with:

- `Emit` trait and `emit!` macro
- `Blob` type (replacing `trace_to_tmpfile`)
- `testing` module (task-local recorder, `Captured` type)
- `configure` module (moved from `jp_cli::lib::configure_logging`)

Update `jp_cli` to call `jp_trace::configure(...)`, read `JP_LOG`,
`JP_LOG_FILE`, `JP_LOG_FORMAT`, and `JP_DEBUG`, and remove the `--log-file`,
`--log-filter`, and `--log-format` CLI flags.
Existing `tracing::info!(...)` calls continue to work unchanged.

This phase depends on [RFD D15] Phase 1 (persistent log directory) and can be
merged independently.

### Phase 2: Initial spans

Add the six span constructors (`cmd_span`, `query_span`, `turn_span`,
`stream_span`, `tool_execution_span`, and `conversation_persist_span`), wire
them into their call sites, and generate the ULID `invocation_id` for
`cmd_span`.

This phase depends on Phase 1 and can be merged independently.

### Phase 3: Typed events in `jp_llm`

Create `crates/jp_llm/src/trace.rs` with event structs for the LLM provider
layer (request sent, response complete, retry, stream error, cache hit, and tool
definition sent), migrate existing `tracing::*!()` calls in `jp_llm` to
`emit!()`, and replace `trace_to_tmpfile` with `Blob::json`.

This phase depends on Phase 1 and can be merged independently.

### Phase 4: Typed events in remaining crates

Migrate remaining crates one PR at a time, ordered by event density:

1. `jp_workspace` (conversation locking, persistence, sanitization)
2. `jp_plugin` (plugin lifecycle, stderr, protocol)
3. `jp_storage` (backend operations, validation)
4. `jp_conversation` (stream operations, compatibility)
5. `jp_cli` (command dispatch, query loop, tool coordination)
6. Remaining crates as needed

Each crate migration depends on Phase 1 and can be merged independently.

### Phase 5: Verbosity model migration

Repurpose `-v` / `-vv` / `-vvv` as chrome verbosity controls once the `Printer`
chrome verbosity API (separate RFD) is implemented or stubbed, and remove the
`-v` to tracing-level mapping from `jp_trace::configure`.

This phase depends on Phase 1 and the chrome verbosity RFD.

## References

- [RFD D15: Structured Logging Infrastructure][RFD D15]: parent RFD that defines
  log file plumbing (persistent directory, deferred path resolution, and
  in-memory buffer).
  This RFD extends [RFD D15] with typed events, revised verbosity semantics, and
  the `jp_trace` crate.
- [RFD 048: Four-Channel Output Model][RFD 048]: establishes stdout, stderr,
  tty, and log-file channel separation.
  This RFD's two-channel model (chrome and tracing) refines the stderr and
  log-file channels.
- [RFD 072: Command Plugin System][RFD 072]: foundation for a future `jp-log`
  command plugin for pretty-printing and managing log files.
- `crates/jp_cli/src/lib.rs`: current `configure_logging` and `TracingGuard`.
- `crates/jp_llm/src/provider.rs`: current `trace_to_tmpfile`.

[OTLP JSON]: https://opentelemetry.io/docs/specs/otlp/#json-protobuf-encoding
[OTLP file]: https://opentelemetry.io/docs/specs/otel/protocol/file-exporter/
[RFD 048]: ../048-four-channel-output-model.md
[RFD 072]: ../072-command-plugin-system.md
[RFD D15]: D15-structured-logging-infrastructure.md
[RFD D34]: D34-versioned-machine-output-envelope.md
