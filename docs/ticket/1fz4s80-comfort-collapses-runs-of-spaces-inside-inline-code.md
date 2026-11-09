# comfort collapses runs of spaces inside inline code

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-09
- **Label**: domain=tooling
- **Label**: package=comfort
- **Label**: type=bug

With `format_markdown = true`, an inline code span holding a run of spaces comes
back with the run collapsed to one space:

```text
Use `a  b` here.    becomes    Use `a b` here.
```

Unlike the padding in T-1f2jmb5, this changes what renders.
The plain reflow (`reflow_markdown`) leaves the span alone; the collapse comes
from comrak's `format_commonmark`.
Its `format_code` writes the span's text through `output(literal, true, ...)`,
and with `render.width > 0` (comfort sets `usize::MAX`) the wrap path skips
every space that follows another.

Comfort can't restore the run by re-parsing comrak's output, since the original
text is gone by then.
A fix has to carry each code span's text from the input across the canonical
pass (for example by sentinelising code spans the way
`protect_reference_form_links` does for reference links), or be fixed upstream
in comrak.

Found while fixing T-1f2jmb5: a doc comment holding `` ` # ` `` lost its double
space when `cargo_format` ran comfort over it.
