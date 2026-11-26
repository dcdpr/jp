# The final command error is printed past the printer

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=bug

`run()` in `crates/jp_cli/src/lib.rs` prints the error a command returns with a
bare `eprintln!` (around lines 303-310, from `d11f209`).
The trace-log notice just below it (`Full trace log written to: …`, and its
`{"trace_log": …}` JSON form) does the same.
RFD 091 allows exactly one stderr writer outside the printer: the opt-in tracing
layer.

What bypassing costs:

- **No format handling.** The printer strips styling for `--format=text` and the
  JSON formats on its own output only.
  An error message carrying ANSI styling reaches the terminal, or a `"message"`
  JSON string, unstripped.

- **No ordering.** The error isn't serialized against output the printer has
  queued, so it relies on the printer having been flushed first.

- [ ] Build the printer in `run()` and pass it into `run_inner`, instead of
      building it inside `run_inner`

- [ ] Print the final error through the printer: `error_println` for text,
      `error_println_raw` for the JSON record `parse_error` builds, then flush.
      It still reaches stderr under `--quiet`

- [ ] Print the trace-log notice through the printer the same way

- [ ] Check errors raised before the printer exists today (argument parsing,
      format resolution) still print, and say so in the commit

- [ ] Test that a styled error message is plain under `--format=text` and inside
      the JSON `message`
