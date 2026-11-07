# RFD 048: Four-Channel Output Model

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-17
- **Tracking Issue**: [\#518]
- **Extended by**: [RFD 086], [RFD 088], [RFD 091], [RFD 095], [RFD 104]
- **Summary**: Separates JP output into four channels: stdout for responses,
  stderr for chrome, `/dev/tty` for prompts, and files for logs.

## Summary

JP separates its output into four channels: **stdout** for assistant responses,
**stderr** for chrome (progress indicators and tool headers), **`/dev/tty`** for
interactive prompts, and a **log file** for tracing.
Channel separation keeps piped output clean (`jp query | jq`), isolates prompt
I/O from stream redirections, and keeps tracing logs out of user-facing streams.

## Motivation

Without dedicated output channels, assistant output, progress indicators, tool
call headers, tracing logs, and interactive prompts share stdout and stderr:

- `jp query "fix it" | jq` includes chrome and progress indicators in the JSON.
- `jp query > answer.txt` captures tool headers alongside the response.
- `-v` tracing output mixes with chrome when stderr is redirected.
- Interactive prompts break when stdout is piped.

Standard CLI tools solve this with channel separation: `git` prompts on
`/dev/tty`, `curl` writes data to stdout and progress to stderr, and `cargo`
sends diagnostics to stderr.
JP adopts the same conventions.

## Design

### Output Channels

JP uses four output channels, each with a single purpose:

| Channel        | Purpose                                |
| -------------- | -------------------------------------- |
| **stdout**     | Assistant responses, structured data   |
| **stderr**     | Chrome: progress, tool headers, status |
| **`/dev/tty`** | Interactive prompts                    |
| **Log file**   | Tracing logs (`-v`)                    |

**stdout** carries only assistant output, or the structured JSON response when
`--format json` is set.
Pipelines and redirections such as `jp query | jq`, `jp query > answer.txt`, and
`jp query | less` work without special-casing whether stdout is a TTY.

**stderr** carries chrome: progress indicators, tool call headers, and status
messages.
In an unredirected terminal session, stdout and stderr both render to the screen
as a single interleaved view; the split only takes effect when a stream is
redirected.

**`/dev/tty`** is the controlling terminal device.
Because it bypasses stdout and stderr redirections, even `jp query > out.txt 2>
err.txt` renders prompts on the terminal, matching `git` (password prompts),
`fzf` (interactive UI), and `sudo` (password entry).

**Tracing logs** (`-v` through `-vvvvv`) go to a log file rather than stderr, so
redirecting stderr (e.g., `jp query 2> chrome.log`) captures chrome without
tracing data.
`--log-file` and `JP_LOG_FILE` configure the log path (defaulting to
`~/.local/share/jp/logs/`), and `--log-format` selects text or JSON output.
Passing `--log-file=-` routes tracing to stderr alongside chrome.

### Output Formatting

The `--format` flag applies to both stdout and stderr.
`--format json` renders assistant output on stdout and chrome on stderr as
NDJSON; when `--format auto` resolves to `text-pretty`, both streams emit
ANSI-formatted text.

Format resolution for `--format auto` checks `stdout.is_terminal()`, selecting
`text-pretty` for terminals and `text` otherwise.
Interactivity checks `/dev/tty` availability instead.
The two checks answer different questions:

- `stdout.is_terminal()`: can the consumer render ANSI escape codes?
- `/dev/tty` available: can a user answer prompts?

> [!TIP]
> [RFD 104] splits `--format auto` per channel: stdout resolves from
> `stdout.is_terminal()`, stderr resolves from `stderr.is_terminal()`, and an
> explicit `--format` still forces both channels.
> Deriving a single format from stdout meant `jp query | less` treated the
> user's terminal as unable to render escapes and stripped in-place stderr
> chrome.
> [RFD 104] also implements the `/dev/tty` promptability check specified in this
> section, which the initial implementation omitted.

### Integration with `Printer`

`Printer` manages every output channel except tracing, preserving a single
output boundary for testing and mocking.

`Printer` adds a `Tty` output target alongside `Out` and `Err`:

```rust
pub enum PrintTarget {
    Out,    // stdout — assistant output, structured data
    Err,    // stderr — chrome, progress, status
    Tty,    // /dev/tty — interactive prompts
}
```

The `Tty` target:

- Always renders with ANSI escape codes, since `/dev/tty` is a terminal by
  definition.
- Ignores `--format`, because prompts are not data output.
- Opens `/dev/tty` lazily on the first call to `tty_writer()`, avoiding
  unnecessary device opens for commands that never prompt.
- Returns an error when `/dev/tty` is unavailable, setting `has_client = false`
  ([RFD 049]).

`PromptBackend` already takes a `writer` parameter; `TerminalPromptBackend`
passes `printer.tty_writer()` instead of `printer.out_writer()`, and `inquire`
reads input from `/dev/tty` instead of stdin.

In tests, the mock `Printer` backs `Tty` with an in-memory buffer so tests can
assert prompt rendering independently of stdout and stderr.

The `tracing` subscriber writes tracing logs directly to the log file without
going through `Printer`.

### Standard Input

`stdin` carries only query content and injected context (e.g., `cat file.rs | jp
query "fix this"`) and is **never** read to answer prompts.
Prompts read from `/dev/tty` in interactive mode or fall back to the detached
policy in non-interactive mode ([RFD 049]).

As a result, `echo "y" | jp query "do the thing"` treats `"y"` as query content
rather than answering a tool permission prompt.

### Platform Portability

| Unix          | Windows                  | Rust crate             |
| ------------- | ------------------------ | ---------------------- |
| `/dev/tty`    | `CONIN$` / `CONOUT$`     | `crossterm`, `termwiz` |
| `ttyname(fd)` | Console handle detection | -                      |

`Printer` and `PromptBackend` hide these platform differences: `tty_writer()`
opens `/dev/tty` on Unix and `CONIN$`/`CONOUT$` on Windows.

## Drawbacks

**Output separation changes redirected output.** Because chrome (progress
indicators and tool headers) goes to stderr and assistant output goes to stdout,
redirecting stdout to a file captures only the assistant's response rather than
the interleaved terminal view.

**Log files are less discoverable than terminal output.** Users expecting `-v`
output in the terminal must check the log file or pass `--log-file=-`.

## Alternatives

### Keep everything on stdout, add `--quiet`

A `--quiet` flag could suppress non-data output on stdout, but users would have
to pass it on every piped invocation.
Channel separation works automatically by following standard Unix conventions.

### Keep tracing on stderr

Writing tracing logs to stderr is simple, but `2> chrome.log` then captures both
chrome and tracing with no way to separate them.

## Non-Goals

- **Non-interactive mode and detached prompt policies.** [RFD 049] defines how
  JP handles prompts when no human is available to answer them.
- **Per-renderer redirection.** All renderers share a single `Printer` without
  per-renderer output targets.
- **Changing `OutputFormat` at runtime.** `Printer` sets `OutputFormat` at
  construction.

## Risks and Open Questions

### Interaction with `--format json`

With `--format json`, both stdout and stderr emit NDJSON.
If both streams stay attached to the same terminal, JSON-formatted chrome on
stderr appears alongside stdout, though scripts parsing JSON typically redirect
or suppress stderr.

### Log file rotation

The default log directory (`~/.local/share/jp/logs/`) accumulates log files over
time.
Log rotation or a configurable maximum age can be added later and is not
required for the initial implementation.

## Implementation Plan

### Phase 1: Route chrome to stderr

Route progress indicators, tool call headers, and status messages to
`PrintTarget::Err` while keeping assistant output on `PrintTarget::Out`, leaving
`/dev/tty` untouched.
This phase can merge independently: terminal sessions look unchanged because
both streams render on the same screen, while piped stdout stays clean.

### Phase 2: Add `PrintTarget::Tty`

Add the `Tty` target to `Printer`, wire `TerminalPromptBackend` to
`printer.tty_writer()`, and open `/dev/tty` lazily, returning an error when the
device is unavailable.
This phase depends on Phase 1.

### Phase 3: Move tracing to a log file

Configure the `tracing` subscriber to write to a log file and add `--log-file`
and `--log-format` flags, with `--log-file=-` writing to stderr for backward
compatibility.
This phase is independent of Phases 1 and 2.

## References

- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]: uses
  `/dev/tty` availability to determine `has_client`.
- [RFD 021: Printer Live Redirection][RFD 021]: `swap_writers()` on `Printer`.
- [RFD 029: Scriptable Structured Output][RFD 029]: depends on output channel
  separation for clean piped JSON.
- [RFD 019: Non-Interactive Mode][RFD 019]: the original combined RFD that this
  proposal was split from.
- `crates/jp_printer/src/printer.rs`: `Printer` implementation.
- `curl` / `git`: precedent for stdout/stderr output separation.

[RFD 019]: 019-non-interactive-mode.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 029]: 029-scriptable-structured-output.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 086]: 086-line-oriented-stdin-input-for-cli-arguments.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[RFD 091]: 091-printer-owned-status-region.md
[RFD 095]: 095-reasoning-region-shading-across-tool-calls.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
