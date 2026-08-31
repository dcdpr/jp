# Emit spanned diagnostics for invalid `#[setting]` attributes instead of panicking

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-24

`Field::from` (`crates/contrib/schematic_macros/src/common/field.rs`) treats an
invalid `#[setting]` / `#[schema]` attribute as fatal:

```rust
let args = FieldArgs::from_attributes(&field.attrs).unwrap_or_else(|error| {
    panic!("Invalid `#[setting]` or `#[schema]` attribute on field `{name}`: {error}");
});
```

The hard failure is correct and has already earned its keep — it surfaced four
fields whose `#[setting]` keys had been silently discarded, two of which had a
declared merge strategy that never applied.
What it loses is the span: a proc-macro panic surfaces as

```
error: proc-macro derive panicked
  --> crates/jp_config/src/lib.rs:96:35
   |
96 | #[derive(Debug, Clone, PartialEq, Config)]
   |                                   ^^^^^^
   = help: message: Invalid `#[setting]` ... on field `inherit`: Unknown field: `optional`
```

anchored on the whole `#[derive(Config)]`, with darling's span discarded and the
real message demoted to a `help:` note.
On a struct with thirty fields the compiler cannot point at the offending key.

In practice the struct location plus the field name in the message is usually
enough to find it, which is why this is a papercut rather than a bug.

## Fix

The idiomatic shape is `Field::from` returning `darling::Result<Field>` and the
derive entry point emitting `error.write_errors()`, which produces a spanned
`compile_error!` at the offending key.

That threads `Result` through `Container::from`, the struct and enum builders,
and both derive entry points — a refactor across the macro crate, larger than
the schematic changes in \#887 combined.
It should carry its own compile-fail test coverage to be worth doing.

Explicitly not worth a shortcut: a thread-local error accumulator would avoid
the signature churn but is exactly the kind of subtle statefulness this crate
does not need.

## Context

Raised in review on \#887 (comment 3659116544), where the reviewer scoped it as
a follow-up.
