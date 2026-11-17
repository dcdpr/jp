# Remove `qual:api` from `md_html::Lines` once it has callers

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: relevant-rfd=124
- **Label**: type=follow-up

`Lines::line_column` and `Lines::offset` in
`crates/internal/md-html/src/position.rs` carry `// qual:api` because no code in
the workspace calls them yet.
Without the marker, rustqual reports each method as dead code (`DRY-002`) and
the `qual` CI job fails.
Once a method has a caller, its marker serves no purpose, and it would stop
rustqual from reporting the method if that caller were later removed.

Remove each marker, and the comment above it naming this ticket, in the change
that first calls its method.
RFD 136, the review engine, is expected to make the first call, in its Phase 1.
