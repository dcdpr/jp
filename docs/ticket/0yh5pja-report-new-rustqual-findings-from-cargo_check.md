# Report new rustqual findings from cargo_check

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Make the `cargo_check` tool (`.config/jp/tools/src/cargo/check.rs`) report the
rustqual findings that CI would fail on, so an assistant can find out before
pushing.
It uses the `qual` crate from T-0yh5frr as a library, so it gives the same
verdict and the same messages as CI.

## Behaviour

- Add a `QualCheck` step next to `ComfortCheck`, with the same `Clean` /
  `Findings(note)` / `Failed(stderr)` shape.
- Run it only after clippy succeeds: findings on code that doesn't compile are
  noise.
- The head is the working tree, including uncommitted changes.
  The base is `merge-base(HEAD, origin/main)`, falling back to `main` when
  there's no remote, and its report is cached by `qual`.
- The `package` parameter filters which findings are *reported*, not what is
  analysed.
  Dead-code and duplicate detection need the whole workspace.
- Render in the AI-oriented format with per-rule guidance, and list the allowed
  escape hatches (see the escape-hatch ticket).
- Update the tool summary in `.jp/mcp/tools/cargo/check.toml`, and the
  `rust-development` skill description if it lists what `cargo_check` covers.

## Tests

Follow `check_tests.rs`: cover clean, findings and failure with
`MockProcessRunner`, plus the case where clippy fails and the qual step is
skipped.
