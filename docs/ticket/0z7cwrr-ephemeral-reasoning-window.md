# Ephemeral Reasoning Window

- **Status**: Todo
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-08
- **Label**: tracking-rfd=120
- **Label**: type=tracking

Tracking ticket for [RFD 120].

## Implementation plan

- **Rename StderrRows to WindowRows**: Renames `StderrRows` in
  `jp_config::style::stderr_rows` to `WindowRows`, since its name and docs
  describe child processes.
  No behavior or config change; the existing `stderr_rows` keys and serialized
  values stay the same.
  Mergeable on its own.
- **Add reasoning window config**: Adds `ReasoningWindowConfig` under
  `style.reasoning.window` with `rows` (default off) and `timer` (default true),
  including the usual config trait implementations and user-facing doc comments.
  The `display` doc comment points at it.
  No renderer effect yet.
  Mergeable on its own; depends on phase 1's `WindowRows`.
- **Render reasoning into the status-region window**: Adds
  `Printer::window_rows`, the single emission seam, per-block placement, window
  lifetime and release, region background, chrome-width wrapping, `timer`
  bookkeeping parity, and the live-flow gate.
  Covered by renderer tests in `render/chat_tests.rs`, a screen-level test via
  `jp_pty`, and a manual terminal check for flicker, default size, resize, and
  the post-interrupt window.
  Depends on phase 2.

[RFD 120]: ../rfd/120-ephemeral-reasoning-window.md
