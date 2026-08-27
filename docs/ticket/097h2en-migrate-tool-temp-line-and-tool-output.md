# Migrate tool temp line and tool output

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091

Migrates `ToolRenderer`'s preparing line and execution-progress ticker to the
printer, deleting `spawn_tick_sender` and the manual rewrite/clear machinery.
Adds a line channel to `forward_stderr`, has the coordinator own one sink per
executing tool, and carries the RFD 095 reasoning-region background across the
migrated rows including the worker's own erases.
