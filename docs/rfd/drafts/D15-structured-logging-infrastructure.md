# RFD D15: Structured Logging Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-05
- **Extends**: [RFD 048]
- **Summary**: Persists structured per-invocation log files under
  `~/.local/share/jp/logs/` and defines `-v`, `-q`, `JP_DEBUG`, and `--log-file`
  controls.

## Summary

This RFD replaces JP's temporary-file tracing with persistent, structured log
files at `~/.local/share/jp/logs/`, adds an in-memory buffer layer for deferred
file path resolution, and introduces a post-run log report.
It also redefines `-v`, `-q`, `JP_DEBUG`, and `--log-file` to control log
verbosity, stderr tracing, and output destinations.

## Motivation

[RFD 048] separates JP's output into four channels: stdout (assistant content),
stderr (chrome), `/dev/tty` (prompts), and a log file (tracing).
Phases 1 and 2 are implemented.
Phase 3, moving tracing to a log file, was deferred because the original design
(a simple flag to redirect tracing) did not account for `-v` behavior, log
discoverability, and post-mortem debugging.

The current tracing setup has four problems:

1. **No persistent logs by default.** Tracing writes to a temporary file that is
   silently discarded on success.
   Users who run without `-v` have no persistent log history; when something
   goes wrong, their only option is to re-run with `-v` and hope to reproduce
   the issue.
2. **`-v` mixes tracing with chrome on stderr.** Running `jp -v query` writes
   tracing to stderr alongside chrome, so `2> chrome.log` captures both and
   defeats the channel separation from Phase 1.
3. **No post-mortem summary.** When warnings or errors occur during a run, users
   have no summary and must scroll through interleaved output or grep a log
   file.
4. **Opaque log filenames.** The temporary file name has no connection to the
   session or conversation it belongs to.

## Design

### Log file location and naming

Every JP invocation writes a log file to:

```
~/.local/share/jp/logs/<session-id>-<conversation-id>-<timestamp>.log
```

- `session-id` is the resolved session identity from `session::resolve()`, or
  `unknown` if no session is available.
- `conversation-id` is the command's primary conversation ID, or `none` for
  commands that do not use a conversation (such as `jp init`).
- `timestamp` is the UTC start time in `YYYYMMDD-HHMMSS` format.

Example: `~/.local/share/jp/logs/12345-a1b2c3-20260405-143022.log`

### Deferred file path resolution

Logging must begin immediately to capture early startup events, before the
session ID and conversation ID needed for the log file path are available.
To bridge that gap, JP buffers startup events in memory:

1. **At startup**, `configure_logging()` installs a custom `tracing` layer that
   buffers events in memory.
2. **After session and conversation resolution**, `run_inner()` calls
   `guard.activate(session_id, conversation_id)`, which builds the log file
   path, creates `~/.local/share/jp/logs/` if needed, opens the file, flushes
   all buffered events to it, and switches the layer to tee mode (memory plus
   file) for subsequent events.
3. **If activation never happens** (for example, during `jp init` or on an early
   error), the buffer is flushed to a fallback file named
   `unknown-none-<timestamp>.log`.

### In-memory event layer

The custom `tracing` layer maintains three pieces of state:

1. **Buffering**: Holds events in a `Vec` until the file path is resolved, then
   clears the `Vec` after flushing it to the file.
2. **Counting**: Tracks `warn_count` and `error_count` for the log report.
3. **Recent errors**: Keeps the last 5 `WARN` and `ERROR` messages in a ring
   buffer for the post-run summary.

After activation, the layer writes each event to the log file while continuing
to update the in-memory counters and ring buffer.

### Verbosity semantics

| Flag         | File log level | Stderr tracing        | Log report    |
| ------------ | -------------- | --------------------- | ------------- |
| *(none)*     | TRACE          | off                   | on error only |
| `-v`         | WARN           | WARN                  | yes           |
| `-vv`        | INFO           | INFO                  | yes           |
| `-vvv`       | DEBUG          | DEBUG                 | yes           |
| `-vvvv`      | TRACE          | TRACE (jp crates)     | yes           |
| `-vvvvv`     | TRACE          | TRACE (+ third-party) | yes           |
| `JP_DEBUG=1` | TRACE          | TRACE (jp crates)     | yes           |
| `-q`         | TRACE          | off                   | off           |

- **Default (no `-v`)**: The file captures `TRACE`, stderr has no tracing layer,
  and the log report prints only on error.
- **`-v` once or more**: Both the file and stderr capture events at the
  specified level (rather than keeping the file at `TRACE`), and the log report
  always prints.
- **`JP_DEBUG=1`**: Equivalent to `-vvvv`.
  JP resolves it before `configure_logging()` so it applies to both the file and
  stderr layers from process start.
- **`-q`**: Suppresses stderr tracing and the log report while keeping `TRACE`
  logging in the file for post-mortem debugging.

### `--log-file` flag

```
--log-file <PATH>
```

`--log-file` writes tracing to an additional destination.
This flag is **additive**: the log at `~/.local/share/jp/logs/` is always
written regardless of `--log-file`.

- `--log-file=-` writes to stderr, which is useful for piping to external tools.
- `--log-file=/path/to/file` writes to the specified file.
- `--log-file=/dev/null` suppresses the additional output while still writing
  the default log file.

### Log report

When enabled (by any `-v`, `JP_DEBUG`, or a non-zero exit code), JP prints a log
report to stderr after the run completes, unless suppressed by `-q`:

```
⚠ 3 warnings, 1 error during run (2.4s)

  WARN  Rate limited, retrying (1/3)
  WARN  Tool fs_create_file skipped by user
  ERROR Stream error: connection reset
  ... (2 more warnings, see log file)

Log: ~/.local/share/jp/logs/12345-a1b2c3-20260405-143022.log
```

The report includes the total warning and error counts, the run duration, up to
the 5 most recent `WARN` and `ERROR` messages (one line each, with a truncation
notice when more than 5 occurred), and the log file path.

### `--log-format` flag

`--log-format` controls the format of stderr tracing output when enabled via
`-v` or `--log-file=-` (unchanged from current behavior):

- `auto`: text for terminals, JSON for pipes
- `text`: compact human-readable format
- `json`: NDJSON format

The persistent log file always uses JSON format for machine parseability.

## Drawbacks

**Default `TRACE` logging produces large files.** Without log rotation, the
`~/.local/share/jp/logs/` directory grows without bound.
Typical query log files are small (< 1 MB), and a planned follow-up task will
add log rotation.

**The in-memory buffer layer adds complexity.** A custom `tracing` layer with
buffering, flushing, counting, and tee semantics requires roughly 150-250 lines
of thread-safe code and must transition atomically from buffer-only mode to tee
mode.

**`-v` lowers file verbosity.** A user who runs with `-v` and later needs the
full `TRACE` log for debugging will not have it.
This trade-off is intentional: setting `-v` explicitly selects the level of
detail for the run, while the default (no `-v`) still captures full `TRACE`.

## Alternatives

### Keep the temp file approach

Continuing to write to `NamedUtf8TempFile` and persisting the file only on error
discards log history for successful runs and makes intermittent issues harder to
debug after the fact.

### Always write full `TRACE` regardless of `-v`

Writing full `TRACE` to the file on every run is simpler, but it produces large
files even when the user asked for a higher threshold and removes control over
file verbosity.
This RFD instead defaults the file to `TRACE` and lets `-v` override it.

### Use the `tracing-appender` crate

`tracing-appender` provides rolling file appenders out of the box, but it does
not support deferred path resolution or in-memory startup buffering.
Implementing a custom `tracing` layer is simpler than wrapping
`tracing-appender` with that lifecycle.

## Non-Goals

- **Log rotation and cleanup.** `~/.local/share/jp/logs/` will accumulate files
  until a planned follow-up task adds rotation.
- **Graduated `-q` levels** (`-qq` = no chrome, `-qqq` = no reasoning, etc.).
  Supporting multiple quiet levels requires changes across the `Printer`,
  `ChatResponseRenderer`, and interactivity systems, and is tracked separately.
- **Structured log querying.** The JSON log files can be parsed with `jq`, but
  this RFD does not add a `jp log` subcommand.

> [!TIP]
> [RFD D34] wraps JP's machine-readable output in a versioned `{format, version,
> data}` envelope and leaves the observability channel out of scope.
> This RFD must decide two boundary questions: whether the post-run log report's
> path line on stderr (currently a bare `{"trace_log": "…"}` object parsed by
> `.config/jp/tools`) uses that versioned envelope, and whether
> `--log-format=json` output on stderr carries a `format` identifier.

## Risks and Open Questions

### Buffer memory usage

The in-memory buffer holds all events from process start until the file path is
resolved.
Typical startup sequences emit a few hundred events (< 100 KB), but pathological
cases (such as a workspace with thousands of conversations triggering warnings
during index load) can grow the buffer much larger.
Capping the buffer (for example at 10,000 events, dropping the oldest) would
bound memory usage at the cost of losing early startup events.

### File I/O on the hot path

Opening the log file and flushing the buffer happen once during startup, not on
the streaming hot path.
Subsequent writes go through the `tracing` subscriber's normal I/O path, so
typical runs see no measurable overhead.

### Log file format stability

The JSON log format is an internal implementation detail, not a public API, but
users may still build tooling around it (such as `jq` scripts).
Adding a format version field to each log entry would allow future schema
changes without silent breakage.

## Implementation Plan

### Phase 1: Persistent log directory and file layer

Replace `NamedUtf8TempFile` with a file at `~/.local/share/jp/logs/`, using a
timestamp-only filename initially.
Wire `JP_DEBUG` as a `-vvvv` alias and update `TracingGuard` to persist the log
file unconditionally.
This phase can be merged independently.

### Phase 2: In-memory buffer layer

Implement the custom `tracing` layer with buffering, counting, and tee
semantics.
Add `guard.activate(session_id, conversation_id)` to `run_inner()` and rename
the log file to include the session and conversation IDs.
This phase depends on Phase 1.

### Phase 3: Log report and `-v` semantics

Add the post-run log report, change `-v` to control the file log level and
enable the stderr layer, and add the `--log-file` flag.
This phase depends on Phase 2.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: parent RFD; this extends Phase
  3 (tracing to log file).
- `crates/jp_cli/src/lib.rs`: current `configure_logging` and `TracingGuard`.
- `crates/jp_cli/src/session.rs`: session identity resolution.

[RFD 048]: ../048-four-channel-output-model.md
[RFD D34]: D34-versioned-machine-output-envelope.md
