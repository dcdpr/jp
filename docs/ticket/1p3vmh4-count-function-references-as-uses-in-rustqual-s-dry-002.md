# Count function references as uses in rustqual's DRY-002

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-20
- **Label**: domain=tooling
- **Label**: type=bug

rustqual's dead-code rules (`DRY-002`, `DRY-006`) count only direct calls, so a
function used as a value is reported as dead code.
T-0yh5j3r item 13 records the known cases (a `static` initializer, a clap value
parser, a `distributed_slice` registration); a new one turned up on 2026-11-20:
`failure_json` in `crates/jp_cli/src/format/conversation.rs`, passed to
`Option::map` as `.map(failure_json)`.

The code was rewritten as an explicit `match` to get past the finding, which is
the workaround the guidance forbids in spirit.
A function reference in any expression position (an argument, a field
initializer, a `static`, an attribute such as `#[arg(value_parser = ...)]` or
`#[serde(with = ...)]`) should count as a use.

This is a fix in the analyzer, so it lands after rustqual is vendored
(T-1dhk659), as its own commit there, and is then offered upstream.

- [ ] A path expression that resolves to a project function counts as a use,
      wherever it appears
- [ ] Revert `failure_json`'s call site in `format/conversation.rs` to
      `.map(failure_json)`
- [ ] Remove item 13's cases from any exemption or marker once they stop being
      reported
