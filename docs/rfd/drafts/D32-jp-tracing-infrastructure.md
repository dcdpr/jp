# RFD D32: JP Tracing Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-17
- **Extends**: RFD D15

## Summary

This RFD introduces a typed tracing system for JP, built on the `tracing`
ecosystem. It replaces ad-hoc `tracing::info!(...)` calls with typed event
structs, adds span-based execution context, and establishes a two-channel output
model: structured tracing for developers (always written to the log file),
chrome for users (controlled by `-v`). The work lives in a new `jp_trace` crate
that owns subscriber configuration, the `emit!` macro, a test capture API, and
content-addressed blob storage for large payloads.

## Motivation

JP's tracing is ad-hoc. Every crate calls `tracing::info!("some message",
field = value)` with free-form strings and inconsistent field names. There are
no spans — not a single `#[instrument]` or `tracing::span!()` in the codebase.
Errors propagate through `?` without recording which span they occurred in.
Large payloads (LLM request bodies) are dumped to temp files via a one-off
`trace_to_tmpfile()` helper. The subscriber is configured in a 150-line function
in `jp_cli::lib` that mixes verbosity semantics, file writing, and stderr
formatting.

This creates concrete problems:

1. **No structure.** Tracing output is only useful to someone who already knows
   the codebase. A `warn!("retrying")` in one provider looks different from
   `warn!("retry")` in another. There is no way to programmatically find "all
   retry events" or "all events from the Anthropic provider" without grepping
   source code.

2. **No hierarchy.** Without spans, there is no way to see that an error
   occurred during the 3rd retry of the 2nd turn of a query against
   `claude-sonnet-4`. Events are a flat stream with no parent-child
   relationships. Post-mortem debugging requires reconstructing the call chain
   manually.

3. **No test observability.** Tests cannot assert "this code path emitted a
   retry event" without capturing stderr output and pattern-matching strings.
   Typed events with a test capture API enable semantic assertions on traced
   behavior.

4. **Mixed audiences.** The `-v` flag controls both user-facing status ("what is
   JP doing?") and developer tracing (internal state, protocol details). These
   serve different audiences with different needs. Users running `jp -vvv` are
   flooded with implementation details they cannot act on. Developers who always
   set `JP_DEBUG=1` get tracing noise mixed into their terminal.

5. **Large payload handling is fragile.** `trace_to_tmpfile()` writes to
   `/tmp` with no cleanup, no connection to the log directory, and no
   deduplication. Each provider re-implements the same pattern.

As JP grows — agentic workflows, server integrations, plugin ecosystems — these
problems compound. A typed tracing system addresses them at the foundation
level, before the codebase doubles in size.

## Design

### Two-channel output model

JP separates output into two channels with distinct audiences:

| Channel | Audience   | Controls              | Content                              |
|---------|------------|-----------------------|--------------------------------------|
| Chrome  | Users      | `-v` / `-q`           | Status lines, progress, tool headers |
| Tracing | Developers | `JP_LOG` / `JP_DEBUG` | Typed events, spans, structured data |

**Chrome** is user-facing status written to stderr via `Printer`. It is curated:
each line is explicitly authored by CLI command code with control over wording
and format. The `-v` / `-vv` / `-vvv` flags control chrome verbosity.

**Tracing** is structured diagnostic data written to a JSON log file. It is
automatic and complete: every typed event and span is recorded at TRACE level
regardless of flags. Tracing is never shown on stderr unless a developer
explicitly opts in via `JP_LOG`.

When an event matters to both audiences (e.g., a rate-limit retry), the CLI code
does both at the same call site:

```rust
printer.chrome_v(format!("⟳ Retrying ({attempt}/{max})…"));
emit!(events::Retrying { attempt, max, backoff, kind });
```

Chrome gets a polished status line. Tracing gets a structured event with all
fields. The two representations are authored independently.

### Verbosity and environment variables

All tracing controls are environment variables. No CLI flags. This keeps `jp
--help` clean of developer-only knobs.

| Variable                  | Purpose                                  | Default                              |
|---------------------------|------------------------------------------|--------------------------------------|
| `JP_DEBUG=1`              | Developer mode. Prints log file path at  | off                                  |
|                           | end of run.                              |                                      |
| `JP_LOG=<filter>`         | Mirrors tracing to stderr with the given | off                                  |
|                           | `EnvFilter` expression. Setting it       |                                      |
|                           | enables the mirror.                      |                                      |
| `JP_LOG_FILE=<path>`      | Overrides the default log file location. | `~/.local/share/jp/logs/…`           |
| `JP_LOG_FORMAT=text/json` | Controls the format of the stderr        | `auto` (text on TTY, JSON otherwise) |
|                           | mirror.                                  |                                      |

CLI flags for verbosity:

| Flag                  | Effect                                   |
|-----------------------|------------------------------------------|
| `-v` / `-vv` / `-vvv` | Increases chrome verbosity. Does not     |
|                       | affect tracing.                          |
| `-q`                  | Suppresses chrome. Does not affect       |
|                       | tracing.                                 |

The log file always captures full TRACE. No flag or variable reduces its
verbosity. This ensures users can always attach a complete log when filing
issues.

Examples:

```sh
# Regular user: chrome only, complete log in background
jp query "fix the bug"

# User with JP_DEBUG always set: same, but prints log path at end
JP_DEBUG=1 jp query "fix the bug"

# Developer wants live tracing for a specific run
JP_LOG=info,jp_llm=trace jp query "fix the bug"

# Verbose chrome + live tracing (orthogonal, combinable)
JP_LOG=warn jp -vv query "fix the bug"
```

This model supersedes the verbosity semantics in [RFD D15]. D15's `-v` through
`-vvvvv` levels, which controlled tracing output, are replaced by the two-knob
model above. D15's log file plumbing (persistent directory, deferred path
resolution, in-memory buffering) remains unchanged.

### The `jp_trace` crate

A new workspace crate that owns JP's tracing infrastructure.

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

`jp_trace` depends on `tracing` and `tracing-subscriber`. It does not depend on
any `jp_*` domain crate. Domain crates depend on `jp_trace` for the `Emit` trait
and `emit!` macro.

### Typed events

Each event is a plain struct with typed fields. Events are defined in a
`trace::events` module within the crate that emits them, and are `pub(crate)` to
enforce that events are only emitted by the code that owns them.

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

The convention: each crate that emits trace events has a `src/trace.rs` file
containing a `pub(crate) mod events` with event structs and their `Emit` impls.
This is the single place to look when browsing a crate's observable behavior.

Event structs have no trait requirements beyond `Sized` (required by `Emit`).
They can carry any typed data their domain needs, including error types and
other non-trivially-copyable values.

A small set of cross-cutting events live in `jp_trace::events::common` as `pub`
types. These cover genuinely shared primitives that no single domain crate owns:
HTTP requests/responses, process lifecycle, file I/O operations.

### The `Emit` trait and `emit!` macro

The `Emit` trait is the interface between event structs and the `tracing`
subscriber:

```rust
// crates/jp_trace/src/lib.rs

pub trait Emit: Sized {
    fn emit(self, file: &'static str, line: u32);
}
```

Call sites never invoke `Emit::emit` directly. The `emit!` macro captures the
caller's source location and invokes the test recorder:

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

The macro is intentionally thin. It exists for two reasons: to capture
`file!()`/`line!()` at the call site (not inside the `Emit` impl), and to
interpose the test recorder (compiled out in non-test builds). `file!()` and
`line!()` are compile-time constants with zero runtime cost.

Caller location fields (`caller.file`, `caller.line`) are always recorded in the
log file. They are part of the structured event data, not metadata about the
`tracing` callsite. Output formatters can strip them if desired, but the default
JSON log includes them unconditionally.

### Typed spans

Spans carry shared context that events within the span inherit automatically via
the `tracing` subscriber. Each span is defined as a function returning a
`tracing::Span`, grouped in the same `trace` module as the crate's events.

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

Call sites:

```rust
let _guard = trace::cmd_span(&cmd_path, &invocation_id).entered();

// For async code:
let stream = provider.chat_completion_stream(model, query)
    .instrument(trace::stream_span(provider_id, &model.name))
    .await?;
```

The initial span set covers the critical path:

| Span                   | Crate          | Fields                  | Scope                             |
|------------------------|----------------|-------------------------|-----------------------------------|
| `cmd`                  | `jp_cli`       | `name`, `invocation_id` | One per JP invocation. Root span. |
| `query`                | `jp_cli`       | `conversation_id`       | Duration of the query command.    |
| `turn`                 | `jp_cli`       | `number`                | One turn within a query.          |
| `llm_stream`           | `jp_llm`       | `provider`, `model`     | One LLM provider call.            |
| `tool_execution`       | `jp_llm`       | `tool`                  | One tool invocation.              |
| `conversation_persist` | `jp_workspace` | `conversation_id`       | Conversation save to disk.        |

The `cmd` span carries a ULID correlation ID (`invocation_id`) that uniquely
identifies a single `jp` invocation. This enables filtering a log file to a
specific run: `jq 'select(.spans[] | .invocation_id == "01HXZ...")'`.

Spans are `pub(crate)` like events. The same rule applies: a span is defined and
entered by the crate that owns the execution boundary it represents.

Because spans carry fields like `provider` and `model`, events emitted inside an
`llm_stream` span do not need to repeat those fields. The subscriber attaches
them automatically.

### Blob storage for large payloads

The `Blob` type replaces `trace_to_tmpfile()`. It decides at construction time
whether a value is small enough to inline or should be written to a sidecar
file:

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

The sidecar directory is `~/.local/share/jp/logs/blobs/`, colocated with the log
files from [RFD D15]. Content-addressed naming (SHA-256 of the payload) means
identical request bodies (common during retries) produce a single file. The
`INLINE_THRESHOLD` is 4 KB.

Event structs use `Blob` as a field type. The `Emit` impl formats it via
`Display`, which writes either the inline JSON or a `blob:<path>` reference.

### Plugin tracing

Plugin processes produce two kinds of trace data: raw stderr lines and
structured log messages from the JSON-RPC protocol. Both are typed.

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

The `plugin_span` carries the plugin ID. Events inside inherit it, so filtering
a log file to a specific plugin is `jq 'select(.spans[] | .id == "jp-path")'`.

The `LogMessage` event dispatches to the appropriate `tracing` level in its
`Emit` impl, translating the plugin's reported level to a `tracing` event.

### Test capture API

The test capture API uses `tokio::task_local!` to scope a recorder to a test
body. The recorder stores event type identity, not event values.

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

Usage in a test:

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

Tests assert on event types and counts, not field values. Field correctness is
verified through function return values and observable side effects, not trace
output. This follows Vector's approach: record event identity, not payloads.

The task-local approach isolates parallel tests without a global mutex. Events
emitted on spawned tasks within the same `tokio::task::LocalSet` share the
recorder. Events on independently spawned tasks (via `tokio::spawn`) do not —
this is a known limitation. Tests that need cross-task capture should use
`LocalSet` or structure their assertions around the coordinating task.

### Subscriber construction

The `configure_logging` function and `TracingGuard` type move from `jp_cli::lib`
to `jp_trace::configure`. The function reads the environment variables described
in [Verbosity and environment variables](#verbosity-and-environment-variables)
and builds the subscriber stack:

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

The file layer writes JSON to the log directory from [RFD D15]. The stderr layer
is only installed when `JP_LOG` is set. The in-memory buffering layer from [RFD
D15] (for deferred file path resolution) remains part of the file layer's setup
— this RFD does not change that mechanism.

`jp_cli::lib` calls `jp_trace::configure(...)` at startup and receives a
`TracingGuard`. The guard's behavior on drop (flushing, persisting) is unchanged
from [RFD D15].

## Drawbacks

**Boilerplate per event.** Every typed event requires a struct definition and an
`Emit` impl with a hand-written `tracing::event!()` call. For a crate with 15
events, this is ~200 lines of mechanical code. A derive macro would reduce this,
but we deliberately avoid one to keep the system simple and debuggable. If the
boilerplate becomes painful at 50+ events, a derive macro can be introduced
later without changing the external API.

**Two representations for dual-audience events.** When an event matters to both
users and developers, the call site has two lines: a chrome call and an
`emit!()`. This is deliberate (chrome is curated UX, tracing is structured
data), but it means the two can drift out of sync. A retry event might update
its chrome wording without updating the event struct's fields, or vice versa.
Code review is the mitigation.

**Migration cost.** Converting existing `tracing::info!(...)` calls to typed
events across ~30 crate-level call sites is not free. Each conversion requires
defining a struct, writing an `Emit` impl, and updating the call site. The
work is mechanical but touches many files.

**No field-level test assertions on events.** The test capture API records event
type identity, not values. Tests cannot assert "the retry event's attempt field
was 2" through the capture API. This is deliberate: field correctness is tested
through return values and side effects, not trace output. If this proves too
limiting for specific cases, a per-event opt-in recording mechanism can be added
later.

**Breaking change: CLI flag removal.** Removing `--log-file`, `--log-filter`,
and `--log-format` in favor of environment variables is a breaking change for
users who have these in shell aliases or scripts. The flags were not widely
advertised and targeted developers, but the change should be communicated in the
changelog.

## Alternatives

### Keep ad-hoc tracing calls

Do nothing. Continue using `tracing::info!("message", field = value)` across
the codebase. This avoids the migration cost and boilerplate, but the problems
in the Motivation section compound as the codebase grows. Rejected because the
current approach does not support test observability, consistent field naming, or
audience separation.

### Derive macro for `Emit`

Generate the `Emit` impl from attributes on the struct:

```rust
#[derive(TraceEvent)]
#[trace(level = "warn", target = "jp_llm::retrying")]
pub(crate) struct Retrying {
    pub attempt: u32,
    // ...
}
```

This reduces boilerplate but adds a proc-macro dependency, increases compile
times, and makes the tracing call opaque (harder to debug what fields are
actually emitted). The manual approach is verbose but transparent. If event
counts grow past ~50 per crate, the derive macro becomes worth the trade-off.

### Enum-based event hierarchy

Group events into namespace structs with a `kind` enum:

```rust
pub struct LlmEvent {
    pub provider: ProviderId,
    pub model: ModelId,
    pub kind: LlmEventKind,
}
```

This reduces the number of top-level types but duplicates fields already carried
by spans (`provider`, `model`). Since spans automatically attach their fields to
events emitted within them, the namespace struct adds redundancy without value.
Flat events + spans is cleaner. Rejected.

### `#[instrument]` instead of typed spans

`tracing`'s `#[instrument]` attribute auto-generates spans from function
signatures. This is convenient but noisy (`skip(...)` annotations everywhere),
leaks internal parameter names into trace output, and does not align spans with
architectural boundaries. Manual span functions give precise control over what
fields appear and where spans start/end. Rejected.

### Centralized event definitions

Define all events in `jp_trace::events::*` (like Vector's
`src/internal_events/`). This gives one place to browse all events but creates a
god-crate that depends on every domain type, or forces domain types into
`jp_trace`. Per-crate `trace::events` modules avoid both problems. Rejected.

### `RUST_LOG` instead of `JP_LOG`

`RUST_LOG` is the ecosystem standard for `EnvFilter` expressions. However, JP
already uses `JP_DEBUG` as a namespaced env var, and `RUST_LOG` would also
affect third-party crate logging (reqwest, hyper, tokio) in unexpected ways.
`JP_LOG` gives JP full control over the filter baseline while using the same
`EnvFilter` syntax. Users familiar with `RUST_LOG` will recognize the format.

## Non-Goals

- **Chrome verbosity API.** The two-channel model commits to chrome as the
  user-facing channel, but the `Printer` API changes (e.g., `chrome_v()`,
  `chrome_vv()`) and the curation of which lines appear at which level are a
  separate RFD.
- **Error emission convention.** The pattern for emitting trace events when
  errors are first materialized is a separate RFD. This RFD provides the
  infrastructure (`emit!`, typed events); the convention for where and when to
  emit is a distinct concern.
- **Log rotation and cleanup.** [RFD D15] defers this. The log directory and
  blob sidecar directory will accumulate files until rotation is implemented.
- **Pretty-printing and `jp-log` command plugin.** A command plugin for
  pretty-printing log files as hierarchical trees is a separate effort, built
  on [RFD 072].
- **OpenTelemetry export.** OTLP integration (for shipping traces to Jaeger,
  Grafana, etc.) is out of scope. The JSON log files with span IDs are
  sufficient for external tooling to reconstruct traces.
- **Metrics.** Unlike Vector's `InternalEvent` system, this RFD does not add
  metric counters (request counts, error rates, token usage). If metrics become
  needed, the `Emit` trait can be extended to emit metrics alongside trace
  events.

## Risks and Open Questions

### Task-local recorder and `tokio::spawn`

The test capture API uses `tokio::task_local!`, which does not propagate across
`tokio::spawn` boundaries. Events emitted on independently spawned tasks are not
captured. This affects tests for code that spawns background tasks (e.g., plugin
process monitoring). Mitigation: use `LocalSet` in tests, or structure
assertions around the coordinating task. If this proves too limiting, a global
recorder with per-test keys (similar to `tracing-test`) is a fallback.

### `Blob` writes on the hot path

`Blob::json()` performs file I/O (SHA-256, `fs::write`) synchronously. If
called from an async context on the hot path, this could block the tokio
runtime. In practice, blob creation happens at TRACE level for LLM request
payloads — once per provider call, not per-event. The I/O is small (a single
write of a few hundred KB). If this becomes measurable, blob writes can be
deferred to a `spawn_blocking` call.

### Migration ordering

Converting a crate's tracing calls to typed events requires the `jp_trace`
crate to exist. But `jp_trace::configure` replaces `jp_cli`'s
`configure_logging`, which must be done carefully to avoid breaking the
subscriber setup. The implementation plan addresses this by phasing: crate
extraction first, then event migration.

### Span overhead

Each active span adds a small per-event cost (the subscriber records span
context for every event). With 6 spans on the critical path, this is negligible.
If span count grows significantly, profiling should confirm the overhead remains
acceptable.

### `caller.file` paths in release builds

`file!()` expands to an absolute path on the build machine. In release builds
distributed to users, this leaks the build environment's directory structure.
This is acceptable for a developer-focused tool where users are typically
building from source, but worth noting. If binary distribution becomes common,
the paths can be stripped via `--remap-path-prefix` in rustc flags.

## Implementation Plan

### Phase 1: `jp_trace` crate and subscriber extraction

Create the `jp_trace` crate with:
- `Emit` trait and `emit!` macro
- `Blob` type (replacing `trace_to_tmpfile`)
- `testing` module (task-local recorder, `Captured` type)
- `configure` module (moved from `jp_cli::lib::configure_logging`)

Update `jp_cli` to call `jp_trace::configure(...)` instead of its local
function. Read `JP_LOG`, `JP_LOG_FILE`, `JP_LOG_FORMAT`, and `JP_DEBUG`
environment variables. Remove `--log-file`, `--log-filter`, and `--log-format`
CLI flags.

No typed events yet — existing `tracing::info!(...)` calls continue to work.
The subscriber stack is unchanged in behavior.

Can be merged independently. Depends on [RFD D15] Phase 1 (persistent log
directory).

### Phase 2: Initial spans

Add the 6 span functions (`cmd_span`, `query_span`, `turn_span`,
`stream_span`, `tool_execution_span`, `conversation_persist_span`) and wire
them into the call sites. Add ULID generation for the `cmd` span's
`invocation_id`.

Can be merged independently. Depends on Phase 1.

### Phase 3: Typed events — `jp_llm`

Create `crates/jp_llm/src/trace.rs` with event structs for the LLM provider
layer: request sent, response complete, retry, stream error, cache hit, tool
definition sent. Migrate existing `tracing::*!()` calls in `jp_llm` to use
`emit!()`. Replace `trace_to_tmpfile` calls with `Blob::json`.

Can be merged independently. Depends on Phase 1.

### Phase 4: Typed events — remaining crates

Migrate one crate at a time, in order of event density:
1. `jp_workspace` (conversation locking, persistence, sanitization)
2. `jp_plugin` (plugin lifecycle, stderr, protocol)
3. `jp_storage` (backend operations, validation)
4. `jp_conversation` (stream operations, compatibility)
5. `jp_cli` (command dispatch, query loop, tool coordination)
6. Remaining crates as needed

Each crate migration is a standalone PR. Depends on Phase 1.

### Phase 5: Verbosity model migration

Change `-v` / `-vv` / `-vvv` from tracing level controls to chrome verbosity
controls. This requires the chrome verbosity API on `Printer` (separate RFD)
to be implemented, or at minimum stubbed. Remove the current `-v` → tracing
level mapping from `jp_trace::configure`.

Depends on Phase 1 and the chrome verbosity RFD.

## References

- [RFD D15: Structured Logging Infrastructure][RFD D15] — parent RFD. Owns log
  file plumbing (persistent directory, deferred path resolution, in-memory
  buffer). This RFD extends D15 with typed events, revised verbosity semantics,
  and the `jp_trace` crate.
- [RFD 048: Four-Channel Output Model][RFD 048] — establishes stdout/stderr/tty/
  log-file channel separation. This RFD's two-channel model (chrome vs tracing)
  refines the stderr and log-file channels.
- [RFD 072: Command Plugin System][RFD 072] — a `jp-log` command plugin can
  provide pretty-printing and log management tools for developers.
- `crates/jp_cli/src/lib.rs` — current `configure_logging` and `TracingGuard`.
- `crates/jp_llm/src/provider.rs` — current `trace_to_tmpfile`.

[RFD D15]: D15-structured-logging-infrastructure.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 072]: 072-command-plugin-system.md
