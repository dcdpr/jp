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

- [x] Audit existing `#[cfg(test)]` sites across the workspace.
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

-----

- **From**: jp
- **Date**: 2026-11-26T12:56:26Z

Audit of 2026-11-26: every production `.rs` file with a `#[cfg(test)]`
attribute, checked for production code after the first one.

rustqual 1.8.2's `count_production_lines` trims each line and stops at the first
starting with `#[cfg(test)]`, so indented attributes (fields, methods,
statements) cut the count too.
`cfg(all(test, …))` and `cfg_attr(test, …)` do not.

55 files have code after their first `#[cfg(test)]`.
In 12 of them it is only the test-only item itself, so nothing is hidden: the
`PartialEq for Error` impls in `jp_openrouter`, `jp_storage`, `jp_mcp`,
`jp_workspace` and `jp_conversation`, the inline `mod tests` in
`jp_tool/src/access.rs` and `openai/parity.rs`, plus `swift.rs`,
`bookworm/src/mcp/tools.rs`, `mock.rs`, `async-anthropic/src/types.rs` and
`jp_llm/src/test.rs`.
`.config/jp/tools/src/code/fixture.rs` is a false positive: the attribute is
inside a string literal in a test-only module.

The other 42 hide production code:

- **Test modules declared before production code**, which only need to move to
  the end of the file: `jp_llm/src/lib.rs`, `jp_mcp/src/client.rs`,
  `comfort/src/cli.rs`, `comfort/src/lib.rs`, `review/src/lib.rs`,
  `.config/jp/tools/src/github.rs`, `jp_config/src/lib.rs`,
  `.config/jp/tools/src/code.rs`.
- **Test-only items with production code after them**, for `test_support`: the
  `PartialEq` impls in `jp_storage/src/load.rs` and `jp_llm/src/error.rs`;
  constructors in `jp_github/src/client.rs`, `jp_cli/src/bootstrap.rs`,
  `acp/rpc.rs`, `fs/modify_file.rs`, `tool/prompter.rs`,
  `serve-chat/src/state.rs` and `label/resolve.rs`; methods in
  `jp_printer/src/region.rs`, `event_id.rs`, `stream/entry.rs`,
  `jp_cli/src/shutdown.rs`, `jp_cli/src/signals.rs`, `dash/src/ui.rs`,
  `tool/pending.rs`, `turn_selection.rs`, `jp_conversation/src/stream.rs` and
  `tool/mcp_executor.rs`; free functions in `jp_cli/src/cli.rs`,
  `jp_llm/src/provider.rs` and `acp/cassette.rs`; `MockInquiryBackend` in
  `tool/inquiry.rs`; `request_value` in `ollama.rs`, `cerebras.rs`, `vllm.rs`,
  `openrouter.rs` and `llamacpp.rs`; a re-export in `jp_cli/src/editor.rs`;
  imports and helpers in `wiki/src/access.rs` and
  `async-anthropic/src/errors.rs`.
- **Items that cannot move**, which need a production change: struct fields and
  `now()` twins in `jp_cli/src/ctx.rs`, a field and statements in
  `cmd/query/turn_start.rs`, and an enum variant and match arm in
  `grizzly/src/db.rs`.

Counted in full, four files exceed the 800-line `SRP-002` limit:
`.config/jp/tools/src/code.rs` (1,667), `tool/mcp_executor.rs` (1,188),
`jp_config/src/lib.rs` (1,014) and `jp_conversation/src/stream.rs` (985).
`fs/modify_file.rs` comes to 792.
