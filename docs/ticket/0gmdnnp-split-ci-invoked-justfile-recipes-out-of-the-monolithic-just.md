# Split CI-invoked justfile recipes out of the monolithic justfile

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-10
- **Label**: domain=tooling
- **Label**: type=task

`.github/workflows/rust.yml`'s `changes` job treats any edit to `justfile` as
touching `source`, and also matches it directly for `fmt`, `fmt-comments`,
`fmt-markdown`, and `vet`:

```
justfile='^justfile$'
source="$rust|$cargo|$workflow|$justfile"
```

That means editing *any* recipe in the 3700+ line `justfile` flips on the full
Rust CI matrix (lint, fmt, fmt-comments, fmt-markdown, test, docs, coverage,
insta, shear, vet, plus the Windows test job), even when the change is nowhere
near the recipes CI actually runs.

Case in point: PR #1155 added `rfd-track`, `rfd-start`, `rfd-stop`, and
`install-plugin` recipes (net +191 lines to `justfile`) and touched no `.rs`
file.
The full Rust CI matrix still ran.
Every job that had finished by the time this was checked (lint, fmt,
fmt-comments, fmt-markdown, deny, coverage, docs) passed, so nothing was broken
— the PR just paid for the whole matrix on a docs/tooling change.

The filter can't currently distinguish "this touched `test-ci`" from "this
touched `rfd-track`" because both live in the same file, so it conservatively
runs everything.
That's a reasonable default, but it's needlessly expensive for the common case
of adding an unrelated recipe.

## Proposed fix

Move the recipes CI actually invokes (`lint-ci`, `fmt-ci`, `fmt-comments-ci`,
`fmt-markdown-ci`, `test-ci`, `docs-ci`, `coverage-ci`, `insta-ci`, `shear-ci`,
`vet-ci`) plus their private dependencies (`_install_ci_matchers`,
`_rustup_component`, `_install`, `_coverage-setup`, `non_jp_excludes`, etc.)
into their own justfile module, e.g. `just/ci.just`, imported by the root
`justfile`.
Point the `changes` job's `justfile` pattern at that module instead of the whole
file, so unrelated recipes (RFD tooling, ticket tooling, plugin builds, docs
helpers, …) no longer trigger the full Rust matrix.

Needs an audit of the full dependency chain of each `*-ci` recipe before moving
anything, so nothing CI relies on is silently left out of the new module.
