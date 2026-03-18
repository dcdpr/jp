# RFD 048: Four-Channel Output Model

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-17

## Summary

This RFD separates JP's output into four channels: **stdout** for assistant
responses, **stderr** for chrome (progress indicators, tool headers),
**`/dev/tty`** for interactive prompts, and a **log file** for tracing. This
makes piped usage clean (`jp query | jq`), keeps prompt I/O independent of
redirections, and prevents tracing output from contaminating either user-facing
stream.

## Motivation

JP currently writes everything to stdout and stderr. Assistant output, progress
indicators, tool call headers, tracing logs, and interactive prompts all share a
single stream. This means:

- `jp query "fix it" | jq` includes chrome and progress indicators in the JSON.
- `jp query > answer.txt` captures tool headers alongside the response.
- `-v` tracing output mixes with chrome when redirecting stderr.
- Interactive prompts break when stdout is piped.

Other CLI tools solve this with channel separation: `git` prompts on `/dev/tty`,
`curl` separates data (stdout) from progress (stderr), `cargo` puts diagnostics
on stderr. JP should follow the same conventions.

## Design

### Output Channels

JP uses four output channels, each with a single purpose:

| Channel        | Purpose                                |
|----------------|----------------------------------------|
| **stdout**     | Assistant responses, structured data   |
| **stderr**     | Chrome: progress, tool headers, status |
| **`/dev/tty`** | Interactive prompts                    |
| **Log file**   | Tracing logs (`-v`)                    |

**stdout** always contains only assistant output. This makes `jp query | jq`,
`jp query > answer.txt`, and `jp query | less` work without special-casing based
on whether stdout is a TTY. When `--format json` is used, stdout contains the
structured JSON response.

**stderr** always contains chrome: progress indicators, tool call headers,
status messages. In a normal terminal session, stdout and stderr both display on
the same screen, so the user sees the same interleaved experience as today. The
separation only matters when redirecting.

**`/dev/tty`** is the controlling terminal device. It bypasses all redirections
— even `jp query > out.txt 2> err.txt` still renders prompts on the terminal.
This is the same pattern used by git (password prompts), fzf (interactive UI),
and sudo (password entry).

**Tracing logs** (`-v` through `-vvvvv`) are written to a log file, not to
stderr. This prevents tracing output from mixing with chrome when a user
redirects stderr (e.g., `jp query 2> chrome.log` captures chrome only, not
tracing data). The log file location is configurable via `--log-file` or
`JP_LOG_FILE`, defaulting to `~/.local/share/jp/logs/`. The `--log-format` flag
controls the format (text or JSON). `--log-file=-` writes tracing to stderr, for
users who want logs and chrome on the same stream.

### Output Formatting

**Output formatting** is controlled by `--format` and applies to both stdout and
stderr. When `--format json` is set, assistant output on stdout and chrome on
stderr are both rendered as NDJSON. When `--format auto` resolves to
`text-pretty` (stdout is a terminal), both channels use ANSI-formatted text.

`stdout.is_terminal()` controls output format resolution (`--format auto` →
`text-pretty` for terminals, `text` otherwise). This is independent of
`/dev/tty` availability, which controls interactivity. The two checks serve
different purposes:

- `stdout.is_terminal()` → can the consumer handle ANSI escape codes?
- `/dev/tty` available → can a user answer prompts?

### Integration with `Printer`

All output channels except tracing are managed through the `Printer` type. This
preserves the single-point-of-output invariant for testing and mocking.

`Printer` gains a `Tty` output target alongside the existing `Out` and `Err`:

```rust
pub enum PrintTarget {
    Out,    // stdout — assistant output, structured data
    Err,    // stderr — chrome, progress, status
    Tty,    // /dev/tty — interactive prompts
}
```

The `Tty` target:

- Always renders with ANSI (it is a terminal by definition).
- Ignores `--format` (prompts are not data output).
- Is opened lazily (first call to `tty_writer()`). Most commands never prompt,
  so `/dev/tty` is not opened unless needed.
- Returns an error if `/dev/tty` is unavailable, which feeds into `has_client =
  false` (see [RFD 049]).

The `PromptBackend` trait already accepts a `writer` parameter. The change is to
wire `TerminalPromptBackend` to `printer.tty_writer()` instead of
`printer.out_writer()`. Input reading via `inquire` similarly uses `/dev/tty`
instead of stdin.

In tests, the mock `Printer` provides an in-memory buffer for the `Tty` target,
allowing prompt rendering to be asserted independently of stdout/stderr output.

Tracing logs are handled separately via the `tracing` subscriber, configured to
write to the log file. They do not flow through `Printer`.

### Standard Input

`stdin` is exclusively for query content and context injection (e.g., `cat
file.rs | jp query "fix this"`). It is **never** used for answering prompts.
Prompt input always comes from `/dev/tty` (interactive mode) or the detached
policy (non-interactive mode, see [RFD 049]).

This means `echo "y" | jp query "do the thing"` does not answer a tool
permission prompt with "y." The "y" is treated as query content.

### Platform Portability

| Unix          | Windows                  | Rust crate               |
|---------------|--------------------------|--------------------------|
| `/dev/tty`    | `CONIN$` / `CONOUT$`     | `crossterm`, `termwiz`   |
| `ttyname(fd)` | Console handle detection | —                        |

The `Printer` and `PromptBackend` abstractions hide the platform-specific
details. The `/dev/tty` path is an implementation detail of `tty_writer()` on
Unix; on Windows, the same method opens `CONIN$`/`CONOUT$`.

## Drawbacks

**Output separation changes rendering.** Chrome (progress, tool headers) goes to
stderr, assistant output goes to stdout. When redirecting, the streams separate.
Users who redirect stdout to a file only see the assistant's final answer, not
the interleaved rendering they see in the terminal.

**Tracing to a log file changes discoverability.** Users accustomed to `-v`
output appearing in the terminal must know to check the log file or use
`--log-file=-`.

## Alternatives

### Keep everything on stdout, add `--quiet`

Suppress non-data output with a flag. Works but requires the user to remember it
every time. Channel separation is automatic based on standard Unix conventions.

### Tracing to stderr (current behavior)

Keep tracing on stderr. Simple but means `2> chrome.log` captures both chrome
and tracing, and there is no way to separate them.

## Non-Goals

- **Non-interactive mode and detached prompt policies.** What happens when no
  human is available to answer prompts is addressed in [RFD 049].
- **Per-renderer redirection.** All renderers share a single `Printer`. This RFD
  does not add per-renderer output targeting.
- **Changing `OutputFormat` at runtime.** The format is set at construction.

## Risks and Open Questions

### Interaction with `--format json`

When `--format json` is set, both stdout and stderr emit NDJSON. Consumers that
parse stdout as JSON may be confused if stderr also contains JSON-formatted
chrome on the same terminal. In practice, scripts redirect or suppress stderr,
so this is unlikely to be a problem.

### Log file rotation

The default log location (`~/.local/share/jp/logs/`) will accumulate logs over
time. A log rotation policy or configurable max age is worth considering but is
not required for the initial implementation.

## Implementation Plan

### Phase 1: Route chrome to stderr

Move progress indicators, tool call headers, and status messages to
`PrintTarget::Err`. Assistant output stays on `PrintTarget::Out`. No `/dev/tty`
changes yet.

Can be merged independently. No behavioral change in a terminal (both streams
display on the same screen). Piped usage becomes cleaner.

### Phase 2: Add `PrintTarget::Tty`

Add the `Tty` target to `Printer`. Wire `TerminalPromptBackend` to
`printer.tty_writer()`. Open `/dev/tty` lazily. Return an error when
unavailable.

Depends on Phase 1.

### Phase 3: Move tracing to a log file

Configure the `tracing` subscriber to write to a log file. Add `--log-file` and
`--log-format` flags. `--log-file=-` writes to stderr for backward
compatibility.

Independent of Phases 1-2.

## References

- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049] — uses
  `/dev/tty` availability to determine `has_client`.
- [RFD 021: Printer Live Redirection][RFD 021] — `swap_writers()` on the
  `Printer`.
- [RFD 029: Scriptable Structured Output][RFD 029] — depends on output channel
  separation for clean piped JSON.
- [RFD 019: Non-Interactive Mode][RFD 019] — the original combined RFD that this
  was split from.
- `crates/jp_printer/src/printer.rs` — current Printer implementation.
- `curl` / `git` — precedent for stdout/stderr output separation.

[RFD 019]: 019-non-interactive-mode.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 029]: 029-scriptable-structured-output.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
