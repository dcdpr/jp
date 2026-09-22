# Prompt notice lines skip the region background

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-22
- **Label**: package=jp_cli
- **Label**: package=jp_printer
- **Label**: type=bug

`Printer::prompt_println` and `Printer::prompt_eprintln` build a `PrintTask` and
send it straight to the worker.
Neither goes through `Canvas`, so neither carries the background
`set_prompt_background` last named, and a notice printed inside a reasoning
block cuts an unshaded row through it.

This is the hole `T-0fg8cjs` described for the prompt itself, in the two helpers
that print a line *beside* a prompt rather than through its writer.

## Reachable today

`report_editor_failure` (`crates/jp_cli/src/editor.rs:103`) writes through
`prompt_eprintln`, and is reached from inside a shaded region two ways:

- `ToolPrompter::inline_edit` (`prompter.rs:394`), when `Ctrl+X` fails during an
  `e` or `r` prompt on a tool called from a shaded reasoning block.
- `InterruptHandler` (`handler.rs:455`, `:462`, `:504`, and the bare
  `prompt_eprintln` at `:463`), on the interrupt reply path.
  Now that the renderer publishes the region while reasoning streams, an
  interrupt taken during reasoning is shaded, so these land inside a region too.

`prompt_println`'s callers are the plugin install flow (`dispatch.rs:1986`,
`:2052`, `install.rs:54`), which is not on the query path.
It has the same gap and no way to reach it today.

## Severity

Contained and visible: one row renders on the terminal default in the middle of
a shaded block, and the block resumes after it.
Nothing leaks, since the line never opens a background to leave open.

## Shape of a fix

The shading lives in `Canvas`, which decorates a `fmt::Write`.
These two build a task instead, so there is no writer to wrap.
Either run the content through a throwaway `Canvas` over a `String` before
sending it, or move the assertion down into the worker for `PrintOrigin::Prompt`
tasks, which would cover both helpers and anything added beside them later.

## Watch for

`prompt_eprintln` targets `Err` while the prompt writer targets `Tty`.
They are separate file descriptors onto the same terminal, so a background
opened by one is not closed by the other: whatever asserts it has to close it
inside the same task.

The notice starts with `\n` so it lands on a fresh row below the prompt.
The fill belongs on that new row, not on the one the widget left mid-line.

`a_prompt_notice_on_the_chrome_stream_is_not_held` (`printer_tests.rs:1099`)
asserts the exact unshaded output, so it has to move with the fix.
