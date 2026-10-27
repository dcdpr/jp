# Migrate the waiting indicator

- **Status**: Done
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-27
- **Implements**: 091
- **Label**: type=feature

Moves the turn loop's waiting indicator, including its status transitions, from
`LineTimer` to the new printer handle.
The existing `turn_loop_tests` waiting-indicator suite carries over as
characterization tests.
