# Eight prompts write straight to stderr, past the printer

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-08

Eight interactive prompts construct their own `io::stderr()` writer, or let
`inquire` pick its default, instead of taking one from `Printer`:

| Site                      | Prompt                            | Writer                         |
| ------------------------- | --------------------------------- | ------------------------------ |
| `bootstrap.rs:473`        | conflicting-root picker           | `io::stderr()`                 |
| `target.rs:994`           | "Select a conversation"           | `io::stderr()`                 |
| `target.rs:1082`          | "Select an archived conversation" | `io::stderr()`                 |
| `target.rs:1117`          | "Select archived conversations"   | `io::stderr()`                 |
| `target.rs:1153`          | "Select conversations"            | `io::stderr()`                 |
| `workspace/target.rs:462` | root picker                       | `io::stderr()`                 |
| `workspace/target.rs:482` | workspace picker                  | `io::stderr()`                 |
| `edit.rs:182`             | "Re-open the editor to fix it?"   | `.prompt()`, inquire's default |

Every other prompt goes through `Printer::prompt_writer()` or
`owned_prompt_writer()`, which is what RFD 048 asks for: prompts are a channel
the printer owns, so it can serialize them against everything else it writes.

## What bypassing costs

**No status-region suspension.** Acquiring a prompt writer erases any drawn
region and blocks redraws for the writer's lifetime (RFD 091).
A prompt that writes directly to stderr gets no such protection, so the
printer's worker can repaint a region row in the middle of a widget's
cursor-relative redraw and corrupt it.

Mostly theoretical today: `bootstrap.rs` and `workspace/target.rs` run before a
region can exist.
`target.rs`'s four conversation pickers are the exception — they run
mid-command, where a region plausibly is up.

**No prompt trace.** `PromptTrace` records `Prompt opened.` / `Prompt closed.`
with `waited_ms` on the `prompt` target, so a trace shows how much of a run was
spent waiting on the user.
These eight are invisible to it, which makes the trace quietly incomplete rather
than obviously so.

**No chrome policy.** `--quiet` closes the chrome channel by dropping writes at
the printer.
A prompt writing past it still renders, so `--quiet` is not the silence it
advertises.

**No `/dev/tty` fallback.** `prompt_writer()` prefers the controlling terminal,
so prompts survive `2>file`.
These eight land in the redirect and the user is left facing a prompt they
cannot see.

## Fix

Mechanically, `io::stderr()` becomes `printer.prompt_writer()` and `.prompt()`
becomes `.prompt_with_writer(&mut printer.prompt_writer())`.

The work is in reaching a printer at each site rather than in the substitution.
`target.rs`'s pickers have one to hand.
`bootstrap.rs` runs during pre-workspace resolution and may need it threaded in;
check before assuming it is available.

## Watch for

`prompt_writer()` returns a `PromptWriter`, not a `PrinterWriter`.
Both implement `fmt::Write` and `io::Write`, so call sites compile unchanged,
but a signature naming the concrete type has to change with them.

Each acquisition takes and releases a suspension.
A site that acquires one writer and reuses it across several prompts holds the
region down for the whole sequence, which is usually what you want; a site that
acquires per prompt lets the region reappear between them.
