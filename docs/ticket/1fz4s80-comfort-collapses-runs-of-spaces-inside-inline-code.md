# comfort collapses runs of spaces inside inline code

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-09
- **Label**: domain=tooling
- **Label**: package=comfort
- **Label**: type=bug

With `format_markdown = true`, comfort collapses consecutive spaces inside an
inline code span to a single space:

```text
Use `a  b` here.    becomes    Use `a b` here.
```

Unlike the delimiter padding in T-1f2jmb5, this changes the rendered output.
Plain reflow (`reflow_markdown`) leaves the span untouched; the collapse happens
in comrak's `format_commonmark`.
Its `format_code` writes the span's literal through `output(literal, true,
...)`, and when `render.width > 0` (comfort sets `usize::MAX`) the line-wrapping
path collapses consecutive spaces.

Re-parsing comrak's output alone cannot recover the original spacing because the
extra spaces have already been stripped.
A fix must either preserve each code span's literal from the input across the
canonical pass (for example by pairing AST nodes or substituting sentinels as
`protect_reference_form_links` does for reference links) or be fixed upstream in
comrak.

Found while fixing T-1f2jmb5: a doc comment containing `` ` # ` `` lost its
double space when `cargo_format` ran comfort over it.
