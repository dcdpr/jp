# Let jp doctor --fix pick fixes interactively

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-23
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=enhancement
- **Parent**: T-1n2tnwv

`jp doctor --fix` without a name walks the report section by section: for each
section with fixable findings it asks which fix to apply, then whether to apply
it to one finding or to every finding in the section.

Built on the named fixes from T-1n2ttac; without a terminal to ask, `--fix`
without a name fails and lists the names it accepts.
