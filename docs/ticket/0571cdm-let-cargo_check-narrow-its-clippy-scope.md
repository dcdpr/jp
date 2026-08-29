# Let cargo\_check narrow its clippy scope

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-08-19

`cargo_check` hardcodes `--all-targets --all-features`
(`.config/jp/tools/src/cargo/check.rs`).
That matches `just lint-ci` and is the right default for this workspace.

It becomes expensive when the tool is pointed at a different cargo workspace via
`options.root`: a dependency-heavy project turns a routine check into a large
compile, which is the opposite of what the inner loop needs.

## Suggested change

A tool option to drop `--all-features`, `--all-targets`, or both.
Default unchanged.

## Related

`check.rs` and `format.rs` also run `comfort` against the same root, so a second
workspace inherits this workspace's doc-comment conventions.
That is defensible (one formatter, one style), but if it proves annoying it
wants its own opt-out rather than being bundled into the flag above.

## Priority

Speculative.
File-and-forget until someone actually feels the cost; do not add knobs
pre-emptively.
