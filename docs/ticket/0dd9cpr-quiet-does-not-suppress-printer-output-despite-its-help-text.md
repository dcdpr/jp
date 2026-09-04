# `--quiet` does not suppress printer output despite its help text

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-04

`--quiet` is documented as "Suppress all output, including errors"
(`crates/jp_cli/src/lib.rs:144-146`), but it only turns tracing off:
`configure_logging` sets the level filter to `OFF` and drops the stderr log
layer (`lib.rs:1344-1346`, `lib.rs:1403-1404`).
Nothing else consults the flag.

So `jp -q query` still prints the assistant response, tool headers, retry
notices, and the non-standard-finish notice.
The run's error message is the sharpest contradiction: `run` writes it with
`eprintln!` regardless of the flag (`lib.rs:444-452`), which is exactly what the
help text promises not to do.

The single exception is `jp c grep`, which reads `ctx.term.args.quiet` itself
(`cmd/conversation/grep.rs:218-244`) to short-circuit to an exit status.
That is grep's own `-q` semantics, not a general rule.

## The docs disagree with each other too

- [RFD 072] (Discussion) says plugin output "goes through the protocol as
  `print` commands, which JP routes through its printer", giving plugins
  "automatic support for `--quiet`" (lines 127-131, 649-653).
  The printer has no such support.
- [D15] defines `-q` as tracing-only (line 122), which matches the code, and
  lists graduated levels (`-qq` = no chrome) as future work spanning the
  `Printer`, `ChatResponseRenderer`, and interactivity systems (lines 223-225).
- [D32] defines `-q` as "Suppresses chrome.
  Does not affect tracing." (line 128), the inverse of what is implemented.

## Where the fix goes

The `Printer` is the only place that sees every emission, so the flag belongs
there rather than at each of its call sites.
`Printer::sink()` already discards everything
(`crates/jp_printer/src/printer.rs:182`), which makes the crude version a
one-line change in `run_inner`.
It is crude because it is all-or-nothing: it would swallow command *data* on
stdout too, and `jp -q c path` printing nothing is not what a script wants.

Deciding what `-q` covers is the actual work:

- chrome only (stderr), leaving data on stdout, per [RFD 048]'s channel split
- everything, matching today's help text
- graduated levels, per D15's future-work note

Whichever is chosen, the help text and the behaviour need to end up saying the
same thing.

## Why it is filed rather than fixed in place

Raised while reviewing PR \#1074, which adds a chrome line announcing a run that
left the cwd's workspace.
Guarding that one line on `!quiet` would have made it the only printer emission
in the CLI honouring the flag while everything around it ignores it.

## Severity

Contained and visible.
A user asking for silence gets output; nothing is lost or corrupted, and the
exit status is unaffected.

[D15]: ../rfd/drafts/D15-structured-logging-infrastructure.md
[D32]: ../rfd/drafts/D32-jp-tracing-infrastructure.md
[RFD 048]: ../rfd/048-four-channel-output-model.md
[RFD 072]: ../rfd/072-command-plugin-system.md
