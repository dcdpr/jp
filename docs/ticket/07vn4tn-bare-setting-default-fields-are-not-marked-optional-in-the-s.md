# Bare `#[setting(default)]` fields are not marked optional in the schema

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-24

`FieldArgs::default` is an `Option<Expr>`, and `parse_default`
(`crates/contrib/schematic_macros/src/utils.rs`) maps the bare
`#[setting(default)]` form to `None` — the same value an absent `default`
attribute produces.
The two are therefore indistinguishable downstream.

Two consequences, both in `crates/contrib/schematic_macros/src/common/field.rs`:

- `Field::is_optional` gates on `args.default.is_some()`, so a bare-default
  field reports `optional: false` in the generated `SchemaField`.
- `PartialConfig::default_values()` contains no entry for the field, so it stays
  `None` there.

Neither is a regression: on `main` the bare form made `preserve_str_literal`
return `Err`, and `FieldArgs::from_attributes(...).unwrap_or_default()`
swallowed it, leaving `args.default = None` as well.
`parse_default` reproduces that deliberately so the ~20 existing bare-default
fields keep byte-identical generated code.

There is also no resolved-config difference: an unset partial field resolves
through `Config::from_partial`'s fallback to the type's `Default` impl, which is
exactly what bare `default` declares.

## Impact today

None observable.
`SchemaField::optional` is write-only in this workspace — the macro populates
it and nothing reads it.
The only two consumers of `AppConfig::schema()` are `AppConfig::fields()` (reads
the field map) and `jp_conversation::compat` (reads `fields` and `flatten`).
There is no JSON Schema export.

So this is metadata that will be wrong for the first consumer that reads it, not
a bug anyone can currently hit.

## Fix

Replace `FieldArgs::default: Option<Expr>` with a three-state representation —
absent / bare / expression:

- `generate_default_value` (`config/field_value.rs`) already treats "absent" and
  "bare" identically; keep that.
- `is_optional` returns true for bare and expression.

Roughly 25 lines in `schematic_macros` across three call sites (the third is the
`Expr::Lit` inspection in `generate_schema_type`).

Because nothing reads the flag, the change is safe to make whenever, and equally
fine to defer until a schema consumer exists.

## Context

Raised in review on \#887 (comment 3657548560) and dismissed there as out of
scope for that PR, which was explicitly behavior-neutral in the macro hunk.
