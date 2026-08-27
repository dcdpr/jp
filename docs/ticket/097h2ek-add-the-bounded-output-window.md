# Add the bounded output window

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091

Adds the bounded line buffer with coalesced redraw, source registration and
labelling, the SGR filter, ANSI-aware truncation, terminal height as a
capability input, multi-row draw/erase with relative row accounting, and the
`print_stderr` config shape (splitting `ProgressConfig`).
Opens with a spike against a real terminal to settle cursor-at-last-row,
concurrent-write, and resize/Windows cases, whose output pins `Printer::memory`
regression tests; no external client uses a non-zero window yet.
