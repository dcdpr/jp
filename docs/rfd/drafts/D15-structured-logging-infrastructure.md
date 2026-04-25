# RFD D15: Structured Logging Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-05
- **Extends**: RFD 048

## Summary

This RFD replaces JP's temporary-file tracing with persistent, structured log
files at `~/.local/share/jp/logs/`, adds an in-memory buffering layer for
deferred file path resolution, and introduces a post-run log report. It
redefines the semantics of `-v`, `-q`, `JP_DEBUG`, and `--log-file` to give
users clear control over what gets logged and where.

## Motivation

[RFD 048] separates JP's output into four channels: stdout (assistant content),
stderr (chrome), `/dev/tty` (prompts), and a log file (tracing). Phases 1 and 2
are implemented. Phase 3 — moving tracing to a log file — was deferred because
the original design (a simple flag to redirect tracing) lost important nuance
around `-v` behavior, log discoverability, and post-mortem debugging.

The current tracing setup writes to a temporary file that is silently discarded
on success. Users who run `jp -v query` see tracing on stderr mixed with chrome.
Users who don't use `-v` have no persistent log history. When something goes
wrong, the only recourse is re-running with `-v` and hoping to reproduce the
issue.

This RFD addresses these problems:

1. **No persistent logs by default.** Tracing data is lost unless the user
   explicitly enables it or the process fails.
2. **`-v` mixes tracing with chrome on stderr.** This makes `2> chrome.log`
   capture both chrome and tracing noise, defeating the channel separation from
   Phase 1.
3. **No post-mortem summary.** When warnings or errors occur during a run, the
   user has no summary — they must scroll through interleaved output or grep a
   log file.
4. **Log file naming is opaque.** The current temp file has no connection to the
   session or conversation it belongs to.

## Design

### Log file location and naming

Every JP invocation writes a log file to:

```
~/.local/share/jp/logs/<session-id>-<conversation-id>-<timestamp>.log
```

- `session-id` is the resolved session identity (from `session::resolve()`).
  Falls back to `unknown` if no session is available.
- `conversation-id` is the primary conversation ID for the command. Falls back
  to `none` for commands that don't use a conversation (e.g., `jp init`).
- `timestamp` is the UTC start time in `YYYYMMDD-HHMMSS` format.

Example: `~/.local/share/jp/logs/12345-a1b2c3-20260405-143022.log`

### Deferred file path resolution

The log file path depends on values (session ID, conversation ID) that are not
available at process start. Logging must begin immediately to capture early
startup events. The solution is an in-memory buffer layer:

1. **At startup**, `configure_logging()` installs a custom `tracing` layer that
   buffers events in memory.
2. **After session and conversation resolution**, `run_inner()` calls
   `guard.activate(session_id, conversation_id)` which:
   - Builds the log file path
   - Creates the `~/.local/share/jp/logs/` directory if needed
   - Opens the file
   - Flushes all buffered events to it
   - Switches to tee mode (memory + file) for subsequent events
3. **If activation never happens** (e.g., `jp init` or early error), the buffer
   is written to a fallback file named `unknown-none-<timestamp>.log`.

### In-memory event layer

A custom `tracing` layer that serves three purposes:

1. **Buffering**: Holds events in a `Vec` until the file path is resolved.
2. **Counting**: Tracks `warn_count` and `error_count` for the log report.
3. **Recent errors**: Keeps the last 5 WARN/ERROR messages in a ring buffer
   for the post-run summary.

After activation, the layer tees events to both the file and the in-memory
counters. The buffer is cleared after flushing to the file.

### Verbosity semantics

| Flag | File log level | Stderr tracing | Log report |
|------|---------------|----------------|------------|
| *(none)* | TRACE | off | on error only |
| `-v` | WARN | WARN | yes |
| `-vv` | INFO | INFO | yes |
| `-vvv` | DEBUG | DEBUG | yes |
| `-vvvv` | TRACE | TRACE (jp crates) | yes |
| `-vvvvv` | TRACE | TRACE (+ third-party) | yes |
| `JP_DEBUG=1` | TRACE | TRACE (jp crates) | yes |
| `-q` | TRACE | off | off |

Key changes from current behavior:

- **Default (no `-v`)**: The file captures TRACE. Stderr has no tracing layer.
  The log report is printed only on error.
- **`-v` once or more**: The file captures at the specified level (not full
  TRACE). Stderr shows tracing at the same level. The log report is always
  printed.
- **`JP_DEBUG=1`**: Equivalent to `-vvvv`. Resolved before `configure_logging`
  so it affects both file and stderr layers from the start.
- **`-q`**: Suppresses tracing output and the log report entirely. The file
  still captures TRACE for post-mortem use.

### `--log-file` flag

```
--log-file <PATH>
```

Writes tracing to an additional destination:

- `--log-file=-` writes to stderr (useful for piping to external tools).
- `--log-file=/path/to/file` writes to the specified file.
- `--log-file=/dev/null` suppresses the additional output (the default log
  file is still written).

This is **additive** — the log at `~/.local/share/jp/logs/` is always written
regardless of `--log-file`.

### Log report

When enabled (any `-v`, `JP_DEBUG`, or non-zero exit code), the report is
printed to stderr after the run completes:

```
⚠ 3 warnings, 1 error during run (2.4s)

  WARN  Rate limited, retrying (1/3)
  WARN  Tool fs_create_file skipped by user
  ERROR Stream error: connection reset
  ... (2 more warnings, see log file)

Log: ~/.local/share/jp/logs/12345-a1b2c3-20260405-143022.log
```

The report includes:
- Total warning and error counts, plus run duration
- Up to 5 most recent WARN/ERROR messages (one line each)
- Truncation notice if more than 5
- Log file path

The report is suppressed by `-q`.

### `--log-format`

Controls the format of stderr tracing output (when enabled via `-v` or
`--log-file=-`). Unchanged from current behavior:

- `auto`: text for terminals, JSON for pipes
- `text`: compact human-readable format
- `json`: NDJSON format

The persistent log file always uses JSON format for machine parseability.

## Drawbacks

**Default TRACE logging produces large files.** Without log rotation, the
`~/.local/share/jp/logs/` directory will grow unbounded. This is mitigated by a
planned log rotation task (not in this RFD's scope) and by the fact that
individual log files for typical queries are small (< 1 MB).

**The in-memory buffer layer adds complexity.** A custom `tracing` layer with
buffering, flushing, counting, and tee semantics is approximately 150-250 lines
of non-trivial code. It must be thread-safe and handle the transition from
buffer-only to tee mode atomically.

**`-v` changes file verbosity.** A user who runs with `-v` and later needs the
full trace for debugging won't have it. This is an intentional trade-off —
users who explicitly set a verbosity level are communicating what level of detail
they want. The default (no `-v`) still captures full TRACE.

## Alternatives

### Keep the temp file approach

Continue writing to `NamedUtf8TempFile` and persisting on error. This loses log
history and makes post-mortem debugging harder for intermittent issues.

### Always write full TRACE regardless of `-v`

Simpler but creates large files on every run and removes the user's ability to
control file verbosity. Rejected in favor of the "default = TRACE, -v = user's
choice" model.

### Use the `tracing-appender` crate

Provides rolling file appenders out of the box. However, it doesn't support the
deferred path resolution or in-memory buffering needed here. The custom layer is
simpler than wrapping `tracing-appender` with the required lifecycle.

## Non-Goals

- **Log rotation and cleanup.** Planned as a follow-up task. The directory will
  accumulate files until that work is done.
- **Graduated `-q` levels** (`-qq` = no chrome, `-qqq` = no reasoning, etc.).
  This requires changes across the `Printer`, `ChatResponseRenderer`, and
  interactivity systems. Tracked separately.
- **Structured log querying.** The JSON log files can be parsed with `jq`, but
  this RFD does not add a `jp log` subcommand or similar.

## Risks and Open Questions

### Buffer memory usage

The in-memory buffer holds all events from process start until the file path is
resolved. For typical startup sequences, this is a few hundred events (< 100
KB). For pathological cases (e.g., workspace with thousands of conversations
triggering warnings during index load), the buffer could grow larger. A cap
(e.g., 10,000 events, dropping oldest) would bound memory usage at the cost of
losing early events.

### File I/O on the hot path

Opening the log file and flushing the buffer happens once during startup, not on
the streaming hot path. Subsequent writes go through the `tracing` subscriber's
normal I/O path. No performance concern for typical usage.

### Log file format stability

The JSON log format is an internal implementation detail, not a public API.
However, users may build tooling around it (e.g., `jq` scripts). A format
version field in each log entry would allow future changes without silent
breakage.

## Implementation Plan

### Phase 1: Persistent log directory and file layer

Replace the `NamedUtf8TempFile` with a file at `~/.local/share/jp/logs/`. Use
a timestamp-only filename initially. Wire `JP_DEBUG` as a `-vvvv` alias. Update
the `TracingGuard` to always persist (no conditional).

Can be merged independently.

### Phase 2: In-memory buffer layer

Implement the custom `tracing` layer with buffering, counting, and tee
semantics. Add `guard.activate(session_id, conversation_id)` to `run_inner()`.
Rename the log file to include session and conversation IDs.

Depends on Phase 1.

### Phase 3: Log report and `-v` semantics

Add the post-run log report. Change `-v` to control file log level and enable
the stderr layer. Add `--log-file` flag.

Depends on Phase 2.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — parent RFD; this extends
  Phase 3 (tracing to log file).
- `crates/jp_cli/src/lib.rs` — current `configure_logging` and `TracingGuard`.
- `crates/jp_cli/src/session.rs` — session identity resolution.

[RFD 048]: 048-four-channel-output-model.md
