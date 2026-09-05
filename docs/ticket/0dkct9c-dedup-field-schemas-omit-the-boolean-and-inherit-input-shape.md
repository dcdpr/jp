# Dedup field schemas omit the boolean and `inherit` input shapes

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-04

Both `dedup` fields accept more input shapes than their generated schema
describes.

- `MergedString::dedup` (`crates/jp_config/src/types/string.rs`) is
  `Option<StringDedup>`, so the schema advertises `off` / `exact` / `block` /
  `contains` plus null.
  `deserialize_string_dedup` also accepts `true`, `false`, and `"inherit"`, and
  the field's doc comment advertises the boolean shorthand.
- `MergedVec::dedup` (`crates/jp_config/src/types/vec.rs`) is `Option<bool>`, so
  the schema advertises a boolean plus null.
  `deserialize_dedup` also accepts `"inherit"`.

`deserialize_with` does not alter the inferred schema: the schema comes from the
field type in `Field::generate_schema_type`
(`crates/contrib/schematic_macros/src/common/field.rs`), which never consults
the deserializer.

## Impact today

None observable.
Nothing exports a JSON Schema for `AppConfig`.
`SchemaBuilder::build_root::<AppConfig>()` appears only in [RFD 061] and [RFD
063], both unimplemented, and `jp init` uses `ConfigEnum` for variant listing
rather than the schema renderer.
So this is metadata that will be wrong for the first schema consumer that
validates a user's config, not something anyone can currently hit.

When that consumer arrives, a config writing the documented `dedup = false` gets
flagged as invalid while continuing to load correctly.

## Fix

`EnableConfig` (`crates/jp_config/src/conversation/tool.rs`) solves the same
problem with `schema_union_with`, which unions caller-supplied variants into the
derived schema — `enable_input_shapes` there declares the bool and the legacy
strings, and `test_enable_schema` pins the result.

That mechanism is container-only: `schema_union_with` lives on `MacroArgs`
(`crates/contrib/schematic_macros/src/common/macros.rs`), which is
`FromDeriveInput`.
A plain field cannot reach it.
Two ways out:

- Add a field-level `schema_union_with` to `schematic_macros`, and point both
  `dedup` fields at a shared function returning the bool and `"inherit"`
  variants.
- Promote each `dedup` to its own `#[derive(Config)]` container with a
  hand-written `Deserialize`, mirroring `EnableConfig`.
  Heavier, and it changes the persisted shape of a field that reaches
  conversation config deltas.

The first is the smaller change and fixes both fields at once.
Either way, add a schema-shape test in the style of `test_enable_schema`.

## Context

Raised in review on \#1064 (comment 3926664348), for the string field only.
Deferred there because the vec field has the same gap, no schema consumer exists
yet, and the fix needs a `schematic_macros` change that is out of scope for a
config-merge bug fix.

[RFD 061]: https://jp.computer/rfd/061
[RFD 063]: https://jp.computer/rfd/063
