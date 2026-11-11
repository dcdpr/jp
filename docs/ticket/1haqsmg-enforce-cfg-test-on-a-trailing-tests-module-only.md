# Enforce `#[cfg(test)]` on a trailing tests module only

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=tooling
- **Label**: type=task

The convention: a module's tests live in `<module>_tests.rs`, and the only
`#[cfg(test)]` in `<module>.rs` is the last item of the file:

```rust
#[cfg(test)]
#[path = "<module>_tests.rs"]
mod tests;
```

Nothing checks it.
Production files carry other `#[cfg(test)]` items above that one:
`crates/jp_llm/src/provider/openai.rs` has 18 `#[cfg(test)]` lines,
`anthropic.rs` 13, `google.rs` 4, `crates/jp_cli/src/cmd/conversation_id.rs` 3.
The sites in those four files are of three kinds:

- test-only constructors and `impl` blocks on production types
  (`Openai::with_credential`, `#[cfg(test)] impl Google`);
- test-support statics and types that other modules' tests use (`TEST_SUPPORT`,
  `API_ROUTE`);
- further test modules declared at the top of the file (`openai.rs` declares six
  `#[path = "openai/…_tests.rs"]` modules).

It matters beyond style: rustqual counts a file's production lines up to the
first `#[cfg(test)]`, so `anthropic.rs` (about 3,700 lines) counts as 22 and
passes the 800-line `SRP-002` limit (T-0yh5j3r item 9).

## The rule

- A `#[cfg(test)]` attribute appears once per file, on a `#[path =
  "…_tests.rs"] mod tests;` item.
- No item follows that one.
- No inline `#[cfg(test)] mod tests { … }`.

clippy doesn't cover it: `items_after_test_module` skips out-of-line modules,
and `tests_outside_test_module` is about `#[test]` functions.

## Decide

- Where each of the three kinds goes.
  A test-only constructor needs access to private fields, so it can't simply
  move to the tests file of another module.
- Whether a module may have several tests files, and if so whether all of them
  are declared together at the end.
- How to enforce it: a text check in `just lint-ci`, next to the ones already
  there, or a lint in the dylint library of T-1dhk8ac, which already lists the
  inline-module case.
  The text check needs no new tooling.
- Whether `cfg(any(test, …))` and `cfg_attr(test, …)` are in scope.

## Then

- [ ] Count the existing sites across the workspace.
- [ ] Move them, or mark the ones that must stay.
- [ ] Add the check.
- [ ] State the rule in the `rust-development` skill, which only says tests go
      in a `_tests` file.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T13:54:56Z

Done on 2026-11-11 for `openai.rs`, `anthropic.rs`, `google.rs`
(`crates/jp_llm/src/provider/`) and `crates/jp_cli/src/cmd/conversation_id.rs`,
which settles two of the points under *Decide* by example:

- **Test-only code that isn't a test goes in a `test_support` child module**,
  `<module>/test_support.rs`: constructors and `impl` blocks on production
  types, recorded-test statics, and their helpers.
  A child module reaches the parent's private fields, so a test-only constructor
  can live there.
  Other modules name it by path, such as `openai::test_support::TEST_SUPPORT`.
- **A module may declare several `#[cfg(test)]` modules, all at the end of the
  file**, with `tests` last and nothing after it.
  Nesting the extra test modules under `tests` would rename every test in them,
  and with that their snapshots.

So the rule a check enforces is: every `#[cfg(test)]` in a production file sits
on a `mod` declaration, and only `#[cfg(test)]` module declarations follow the
first one.

Still open in the rest of the workspace: at least 26 sites in 17 files that are
indented or sit on a non-module item (found by text search, which misses a
`#[cfg(test)]` line directly above an unindented item), and every other
provider's `TEST_SUPPORT` block (`cerebras.rs`, `llamacpp.rs`, `ollama.rs`,
`openrouter.rs`, `vllm.rs`, `mock.rs`) plus the test-support traits in
`provider.rs`.
`crates/jp_llm/src/provider/openai/parity.rs` also has an inline `mod tests { …
}`.

Two things the move showed about the gate:

- With the count no longer cut short, `anthropic.rs` (2287 production lines),
  `openai.rs` (2754) and `google.rs` (1129) are over the 800-line `SRP-002`
  limit, and `qual diff` reports each as new, because the base counted them as
  22, 52 and 134.
  Every file fixed under this ticket that is over 800 lines will do the same.
- `qual diff` keys a finding by `(rule, file, symbol)` and maps whole-file
  renames only, so a function that moves to another file with an existing
  finding is reported as new.
  `Openai::with_credential` (`BP-008`) did; `erase_model_output` (`CX-002`) did
  too and was simplified instead.

-----

- **From**: jp
- **Date**: 2026-11-11T14:12:42Z
- **Re**: #1

Both gate problems from the comment above are handled:

- The three provider files carry a `qual:allow(srp, file_length=N)` pin (2400,
  2900, 1200), each naming the ticket that splits the file: T-1hb8d4j,
  T-1hb8d4k, T-1hb8d4m.
  A file fixed under this ticket that turns out to be over 800 lines gets the
  same: a pin within 10% of its count, and a ticket.
- `qual diff` follows a finding to another file by rule and symbol, so moving a
  function into `test_support.rs` reports nothing.
  File-level findings have no symbol and don't follow.
