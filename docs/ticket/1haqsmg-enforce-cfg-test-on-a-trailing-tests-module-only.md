# Enforce `#[cfg(test)]` on a trailing tests module only

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

By convention, a module's unit tests live in `<module>_tests.rs`, and the only
`#[cfg(test)]` attribute in `<module>.rs` is on the trailing `mod tests;`
declaration at the end of the file:

```rust
#[cfg(test)]
#[path = "<module>_tests.rs"]
mod tests;
```

This convention is not yet enforced by CI.
Several production files place `#[cfg(test)]` items earlier in the file:
`crates/jp_llm/src/provider/openai.rs` has 18 `#[cfg(test)]` attributes,
`anthropic.rs` has 13, `google.rs` has 4, and
`crates/jp_cli/src/cmd/conversation_id.rs` has 3.
Those items fall into three categories:

- test-only constructors and `impl` blocks on production types
  (`Openai::with_credential`, `#[cfg(test)] impl Google`);
- test-support statics and types used by other modules' tests (`TEST_SUPPORT`,
  `API_ROUTE`);
- additional test modules declared near the top of the file (`openai.rs`
  declares six `#[path = "openai/…_tests.rs"]` modules).

Beyond style consistency, early `#[cfg(test)]` attributes break `rustqual`'s
file-length check: `rustqual` stops counting production lines at the first
`#[cfg(test)]` in a file, causing `anthropic.rs` (~3,700 lines) to count as 22
production lines and pass the 800-line `SRP-002` limit (T-0yh5j3r item 9).

## The rule

- Every `#[cfg(test)]` attribute in a production file must be on a trailing
  module declaration (`#[path = "…_tests.rs"] mod tests;` or `mod
  test_support;`).
- No non-test items may follow the first `#[cfg(test)]` module declaration.
- No inline `#[cfg(test)] mod tests { … }` blocks.

Clippy does not cover this: `items_after_test_module` ignores out-of-line
`#[path = "..."] mod tests;` declarations, and `tests_outside_test_module` only
checks `#[test]` functions.

## Decide

- Where to place each of the three categories of test-only items.
  Test-only constructors need access to private fields on the parent type, so
  they cannot move into an unrelated module's test file.
- Whether a module may declare multiple test files, and whether all of them must
  appear at the end of the file.
- How to enforce the rule: a text check in `just lint-ci` alongside the existing
  checks, or a custom lint in the `dylint` library (T-1dhk8ac, which already
  tracks inline test modules).
- Whether `cfg(any(test, …))` and `cfg_attr(test, …)` are in scope.

## Then

- [ ] Audit existing `#[cfg(test)]` sites across the workspace.
- [ ] Move them to trailing module declarations, or exempt any that must stay.
- [ ] Add the enforcement check to CI.
- [ ] Document the rule in the `rust-development` skill.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T13:54:56Z

Applied on 2026-11-11 to `openai.rs`, `anthropic.rs`, and `google.rs` in
`crates/jp_llm/src/provider/`, and to
`crates/jp_cli/src/cmd/conversation_id.rs`, resolving the first two design
questions:

- **Non-test support code lives in a `test_support` child module**
  (`<module>/test_support.rs`): constructors and `impl` blocks on production
  types, recorded-test statics, and their helpers.
  Because `test_support` is a child module, it can access private fields on the
  parent module's types, and sibling modules can reference items via
  `<module>::test_support::*` (such as `openai::test_support::TEST_SUPPORT`).
- **A module may declare multiple `#[cfg(test)]` submodules at the end of the
  file**, with `mod tests;` last and no items after it.
  Keeping sibling `*_tests` modules on the parent rather than nesting them under
  `tests` preserves their module paths and `insta` snapshot names.

The rule to enforce is: every `#[cfg(test)]` in a production file sits on a
`mod` declaration at the end of the file, with no non-`#[cfg(test)]` items after
the first one.

Remaining across the workspace: at least 26 sites in 17 files where
`#[cfg(test)]` is indented or attached to a non-module item, plus the
`TEST_SUPPORT` blocks in the other LLM providers (`cerebras.rs`, `llamacpp.rs`,
`ollama.rs`, `openrouter.rs`, `vllm.rs`, `mock.rs`), the test-support traits in
`provider.rs`, and an inline `mod tests { … }` in
`crates/jp_llm/src/provider/openai/parity.rs`.

Moving the early `#[cfg(test)]` items surfaced two interactions with the `qual`
gate:

- With the line count no longer truncated early, `anthropic.rs` (2,287
  production lines), `openai.rs` (2,754), and `google.rs` (1,129) exceed the
  800-line `SRP-002` limit.
  Because the base revision counted them as 22, 52, and 134 lines, `qual diff`
  reports each file as a new `SRP-002` finding.
  Any other file over 800 production lines will trigger `SRP-002` when its early
  `#[cfg(test)]` items are moved.
- `qual diff` originally keyed findings by `(rule, file, symbol)` and only
  mapped whole-file renames, so moving a function with an existing finding into
  `test_support.rs` reported that finding as new (`Openai::with_credential` hit
  `BP-008`, and `erase_model_output` hit `CX-002` and was refactored).

-----

- **From**: jp
- **Date**: 2026-11-11T14:12:42Z
- **Re**: #1

Both `qual` gate interactions from the comment above are resolved:

- The three provider files carry `qual:allow(srp, file_length=N)` pins (2400,
  2900, and 1200) referencing the tickets that split them (T-1hb8d4j, T-1hb8d4k,
  and T-1hb8d4m).
  Any other file that exceeds 800 lines after moving its `#[cfg(test)]` items
  should receive a `file_length` pin within ~10% of its line count and a
  follow-up split ticket.
- `qual diff` keeps the baseline of a finding whose item moved to another file,
  so moving a function into `test_support.rs` does not report its existing
  findings as new.
  It follows an item only when the source confirms the move: the old file no
  longer defines the item, and the new file didn't define it at the base commit.
  A same-named item in another file gets no credit.
  File-level findings have an empty symbol and remain scoped to their file.
