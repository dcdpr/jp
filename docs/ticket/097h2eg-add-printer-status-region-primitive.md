# Add printer status region primitive

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091

Adds the claim stack, `Command` variants, `recv_timeout` ticking,
erase-before-write/redraw-after, the enabling predicate, row background,
shutdown erasure, prompt-writer suspension, and the `suspend_status` guard to
`jp_printer`.
Regions are one row only (zero window budget), with unit tests against
`Printer::memory` and an explicit terminal-capability override.
