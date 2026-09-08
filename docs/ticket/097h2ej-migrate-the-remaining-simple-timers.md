# Migrate the remaining simple timers

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091

Migrates the reasoning timer, lock-wait countdown, MCP startup timer, both drain
timers, and the stream-retry notice to the printer, and moves the
lock-contention prompt from `err_writer()` to `prompt_writer()`.
Deletes `spawn_line_timer` and `LineTimer`; the retry notice keeps its
non-terminal branch and `retry_tests` coverage.
