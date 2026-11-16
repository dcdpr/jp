# RFD D02: Lossless Config Editing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-31
- **Summary**: Replaces serde round-tripping in `config set` with
  format-preserving document editors for TOML, JSON, JSON5, and YAML.

## Summary

This RFD replaces the lossy deserialize-mutate-serialize round-trip in `config
set` with format-preserving editors for every config format JP supports (TOML,
JSON, JSON5, and YAML).
Updating a single config value modifies only that key in place, eliminating
unrelated git diff noise.

## Motivation

JP supports four configuration file formats: TOML, JSON, JSON5, and YAML.
The `config set` command reads the file, deserializes it into a
`PartialAppConfig`, merges the delta, and re-serializes the entire struct to
disk.
This full round-trip through serde destroys the file's original formatting:

- **Key reordering**: serde serializes fields in struct definition order rather
  than their order in the file.
- **Representation normalization**: compact inline values expand into full
  structures.
  For example, a TOML alias `anthropic = "anthropic/claude-sonnet-4-6"` becomes
  a multi-line table with `provider` and `name` keys because `Deserialize`
  accepts both forms while `Serialize` always emits the struct form.
- **Comment and whitespace loss**: comments in YAML, JSON5, and TOML are
  discarded, blank lines used for visual grouping disappear, and indentation is
  normalized.
- **Quote style changes**: TOML single-quoted keys become double-quoted, and
  JSON5 unquoted keys are quoted.

Running `jp cfg set --cfg conversation.default_id=ask` on a 55-line TOML config
produces a 130-line diff.
Every alias, nested key, and key order is rewritten even though only one value
changed.
For users who commit their JP config to version control, these diffs obscure the
actual change and make changes hard to review.

## Design

### Approach

Instead of round-tripping the full config through serde, `config set` updates
the document tree in place:

1. Parse the original file into a format-preserving document tree.
2. Serialize only the delta (the changed fields).
3. Deep-merge the serialized delta into the document tree, touching only the
   keys present in the delta.
4. Write the modified tree back to disk.

Untouched content (comments, whitespace, key order, quote styles, and blank
lines) stays intact because the original file is never round-tripped through
serde.

### Deep merge algorithm

The merge is recursive and applies to all formats:

```rust
fn deep_merge(target: &mut Tree, source: &Tree) {
    for (key, source_value) in source {
        if target[key] is a map AND source_value is a map {
            deep_merge(&mut target[key], source_value)
        } else {
            target.insert(key, source_value)  // add or overwrite
        }
    }
}
```

**Existing keys** are updated in place, preserving surrounding whitespace and
comments.
**New keys** are appended to the target section or object and inherit the
formatting conventions of their siblings (such as indentation and trailing
commas) as supported by the editor library.

### Format-specific editors

Each format uses a library that parses source text into an editable syntax tree
and writes it back losslessly:

| Format       | Editor              | Source                              |
| ------------ | ------------------- | ----------------------------------- |
| JSON / JSON5 | `json_edit`         | New crate in `crates/contrib`       |
| YAML         | [`yaml-edit`][yaml-edit]       | External crate                      |
| TOML         | [`toml_edit`][toml-edit] or    | See [TOML strategy](#toml-strategy) |
|              | `json_edit`-derived |                                     |

#### JSON and JSON5: `json_edit`

Because no format-preserving JSON editor exists in the Rust ecosystem, this RFD
introduces `crates/contrib/json_edit`, designed for eventual publication to
crates.io.

`json_edit` is built on [`rowan`][rowan], the lossless syntax tree library from
rust-analyzer.
`yaml-edit` also uses `rowan`, so both editors share the same underlying syntax
tree dependency.

JSON's grammar is LL(1) and much simpler than YAML's.
The full crate is estimated at ~900 lines including tests:

| Component                             | Est. lines | Notes                               |
| ------------------------------------- | ---------- | ----------------------------------- |
| `SyntaxKind` enum + rowan glue        | ~50        | ~15 token types                     |
| Lexer                                 | ~150       | String escapes are the hardest part |
| Parser                                | ~150       | Recursive descent                   |
| High-level API (`Document`, `Object`, | ~250       | `get`, `set`, `remove`, `insert`    |
| `Array`)                              |            |                                     |
| Tests                                 | ~300       |                                     |

JSON5 extends JSON with single-line comments (`//`), block comments (`/* */`),
trailing commas, unquoted identifier keys, single-quoted strings, and additional
number literals.
The lexer handles these extensions; the parser and high-level API remain nearly
identical.

The public API exposes in-place mutation on parsed documents:

```rust
let doc = json_edit::Document::parse(input)?;
let root = doc.as_object()?;
root.set("key", json_edit::Value::from("value"));
root.remove("old_key");
println!("{}", doc); // lossless output
```

#### YAML: `yaml-edit`

[`yaml-edit`][yaml-edit] provides lossless YAML parsing and editing on top of
`rowan`, preserving comments, whitespace, and formatting through in-place syntax
tree mutations.

Its API maps directly to our deep-merge operations:

```rust
let doc = yaml_edit::Document::from_str(content)?;
let mapping = doc.as_mapping()?;
mapping.set("key", "value");    // preserves surrounding formatting
mapping.remove("old_key");
```

At version 0.2.1 (~1.7k downloads), `yaml-edit` has a clean API but is still a
young crate.
We will pin to a specific version and monitor for breaking changes.

#### TOML strategy

[`toml_edit`][toml-edit] is the standard crate for lossless TOML editing and
underpins Cargo's `Cargo.toml` manipulation.
It fits our use case with one documented limitation: it does not preserve the
order of dotted keys (for example, `a.b.c = 1` may be rewritten as a nested
table).

There are two options:

1. **Use `toml_edit` as-is.** This is the pragmatic choice.
   The dotted-key limitation is minor because most users write `[section]`
   headers rather than deeply dotted keys.

2. **Build a `toml_edit` equivalent on top of `json_edit`'s `rowan`
   infrastructure.** Extending the `json_edit` lexer and parser framework to
   TOML gives full control over formatting preservation, including dotted keys.
   TOML's grammar is more complex than JSON's but simpler than YAML's, at an
   estimated cost of ~1500-2000 additional lines.

This RFD adopts option 1 for the initial implementation.
If dotted-key reordering causes user issues, option 2 can be built with low
marginal effort on top of the existing `rowan` infrastructure.

### Integration with `ConfigFile`

`ConfigFile::merge_delta` is the single entry point, dispatching to the
format-specific editor based on `self.format`:

```rust
pub fn merge_delta<T: Serialize>(
    &mut self,
    delta: &T,
) -> Result<(), Box<dyn Error + Send + Sync>> {
    match self.format {
        Format::Json | Format::Json5 => self.merge_json(delta),
        Format::Yaml => self.merge_yaml(delta),
        Format::Toml => self.merge_toml(delta),
    }
}
```

Each `merge_*` method follows the same four steps:

1. Parse `self.content` into the format's lossless document tree.
2. Serialize `delta` to a `serde_json::Value`.
3. Deep-merge the `serde_json::Value` into the lossless document.
4. Write the modified document back to `self.content`.

Step 2 uses `serde_json::Value` as the common intermediate representation
because all four formats round-trip through it without loss for the subset of
types `PartialAppConfig` uses (strings, numbers, booleans, arrays, and objects).

### Interaction with `config fmt`

This RFD does not change `config fmt` or `format_content`, which continue to run
a full serde deserialize-serialize round-trip.
The two commands serve distinct purposes:

- `config set`: preserve existing formatting and modify only the requested keys.
- `config fmt`: normalize the entire file to a canonical format.

## Drawbacks

- **New dependency on `rowan`.** `rowan` adds ~4 crates to the dependency tree,
  though `yaml-edit` already depends on it.
- **Maintenance cost of `json_edit`.** We maintain this crate in-tree.
  JSON's grammar is small and stable, keeping ongoing maintenance low, but not
  zero.
- **Three editor backends.** Each format has its own tree representation and
  API.
  `ConfigFile::merge_delta` and the shared `serde_json::Value` intermediate
  representation keep the integration surface small, but bugs can still manifest
  differently per format.

## Alternatives

### Span-guided string surgery

Parse the original file with a span-tracking parser (such as
`json-spanned-value`) and use byte offsets to splice replacement values into the
raw string, avoiding a full lossless syntax tree.

Rejected because inserting new keys requires finding the insertion point,
matching indentation, and updating trailing commas, which rebuilds the
formatting logic that a lossless tree already provides.
In addition, `json-spanned-value` supports only standard JSON (not JSON5) and
depends on `indexmap` v1 (JP uses v2).

### Serde round-trip with `preserve_order`

Enable `preserve_order` in `serde_json` for JSON, accept whitespace
normalization, and use lossless editors only for TOML and YAML.

Rejected because key ordering is only part of the problem.
Comment preservation matters for JSON5 and YAML, and indentation normalization
still produces diff noise even when key order is preserved.

### Do nothing

Keep the serde round-trip in `config set` and accept the noisy diffs.

Rejected because users commit config files to version control.
Noisy diffs from `config set` erode trust in the tool and discourage use of the
command.

## Non-Goals

- **Conflict-free concurrent editing.** `config set` reads, modifies, and writes
  the file non-atomically.
  This RFD does not add file locking or merge conflict resolution.
- **Schema migration.** The config loading pipeline handles renamed config keys
  across JP versions, not the document editor.
- **Formatting preferences.** This RFD preserves existing formatting rather than
  adding style settings for new keys (for example, configuring 2-space
  indentation).
  New keys inherit the formatting of their surrounding elements as supported by
  the editor library.

## Risks and Open Questions

- **`yaml-edit` maturity.** Version 0.2.1 may have parsing bugs on edge-case
  YAML documents.
  We will pin the version, add integration tests against realistic JP config
  files, and contribute fixes upstream.
- **JSON5 comment variants.** JSON5 allows `//` and `/* */` comments, including
  inside objects and arrays.
  The `json_edit` lexer and parser must test both forms across all syntactic
  positions.
- **`toml_edit` dotted key reordering.** If users rely on dotted keys like
  `a.b.c = 1`, `toml_edit` may rewrite them as nested tables (see the [toml_edit
  issue tracker][toml-edit-dotted]).
  We will monitor for user reports.
- **Empty delta serialization.** `PartialAppConfig` skips `None` fields during
  serialization, so an empty delta (all fields `None`) serializes to `""` or
  `{}`.
  The merge must treat an empty delta as a no-op without modifying the file.

## Implementation Plan

### Phase 1: `json_edit` crate

Create `crates/contrib/json_edit` with:

- A `rowan`-based lexer and parser for JSON and JSON5.
- `Document`, `Object`, `Array`, and `Value` wrapper types.
- Format-preserving `set`, `remove`, and `insert` operations.
- Unit tests covering comment and whitespace preservation, key ordering, nested
  edits, trailing commas (JSON5), and unquoted keys (JSON5).

This phase does not change JP's config pipeline and can be reviewed and merged
independently.

### Phase 2: JSON/JSON5 integration

Wire `json_edit` into `ConfigFile::merge_delta` for `Format::Json` and
`Format::Json5`, and add integration tests verifying that `config set` on JSON
and JSON5 files produces minimal diffs.

This phase depends on Phase 1.

### Phase 3: YAML integration

Add `yaml-edit` as a dependency, wire it into `ConfigFile::merge_delta` for
`Format::Yaml`, and add integration tests with comment-heavy YAML configs.

This phase is independent of Phases 1 and 2 and can proceed in parallel.

### Phase 4: TOML integration

Add `toml_edit` as a dependency, wire it into `ConfigFile::merge_delta` for
`Format::Toml`, and add integration tests verifying that alias strings, key
order, and comments are preserved.

This phase is independent of Phases 1 through 3 and can proceed in parallel.

### Phase 5 (optional): Rowan-based TOML editor

If the `toml_edit` dotted-key limitation or other formatting issues prove
problematic, build a TOML lexer and parser on the same `rowan` infrastructure as
`json_edit` to replace the `toml_edit` dependency.

This phase depends on Phase 1 for the `rowan` infrastructure and Phase 4 for
user feedback on `toml_edit` limitations.

## References

- [`toml_edit`][toml-edit]: format-preserving TOML editor used by Cargo.
- [`yaml-edit`][yaml-edit]: lossless YAML parser and editor built on `rowan`.
- [`rowan`][rowan]: generic lossless syntax tree library from rust-analyzer.
- [`json-spanned-value`][json-spanned]: span-tracking JSON parser (considered
  and rejected).
- [toml_edit dotted key issue][toml-edit-dotted]: upstream issue tracking
  dotted-key reordering in `toml_edit`.

[json-spanned]: https://docs.rs/json-spanned-value
[rowan]: https://docs.rs/rowan
[toml-edit]: https://docs.rs/toml_edit
[toml-edit-dotted]: https://github.com/toml-rs/toml/issues/163
[yaml-edit]: https://docs.rs/yaml-edit
