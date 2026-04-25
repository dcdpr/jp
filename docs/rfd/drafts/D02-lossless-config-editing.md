# RFD D02: Lossless Config Editing

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-31

## Summary

Replace the lossy deserialize-mutate-serialize round-trip in `config set` with
format-preserving editors for every config format JP supports (TOML, JSON,
JSON5, YAML). This eliminates git diff noise when a user changes a single config
value.

## Motivation

JP supports four configuration file formats: TOML, JSON, JSON5, and YAML. The
`config set` command currently reads the file, deserializes it into a
`PartialAppConfig`, merges the delta, and re-serializes the entire struct back
to disk. This full round-trip through serde destroys the original file's
formatting:

- **Key reordering**: serde serializes fields in struct definition order, not
  the order they appeared in the file.
- **Representation normalization**: compact inline values get expanded. For
  example, a TOML alias `anthropic = "anthropic/claude-sonnet-4-6"` becomes a
  multi-line table with `provider` and `name` keys because the `Deserialize`
  impl accepts both forms but `Serialize` always produces the struct form.
- **Whitespace and comment destruction**: comments in YAML, JSON5, and TOML are
  discarded entirely. Blank lines used for visual grouping disappear.
  Indentation style may change.
- **Quote style changes**: TOML single-quoted keys become double-quoted. JSON5
  unquoted keys gain quotes.

The result is that setting a single value like `conversation.default_id=ask`
produces a diff that touches most of the file. For users who commit their JP
config to version control (which we encourage), this creates noisy, unreviable
diffs that obscure the actual change.

### Example

Running `jp cfg set --cfg conversation.default_id=ask` on a 55-line TOML config
produces a 130-line diff. Every alias, every nested key, and the key ordering
are rewritten, even though only one value was added.

## Design

### Approach

Instead of round-tripping through serde, `config set` should:

1. Parse the original file into a format-preserving document tree.
2. Serialize only the delta (the changed fields) to the same format.
3. Deep-merge the delta into the original tree, touching only the keys present
   in the delta.
4. Write the modified tree back to disk.

Untouched content — comments, whitespace, key order, quote styles, blank lines —
is preserved because it was never reparsed through serde.

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

For **existing keys**, the value at that position in the document is replaced.
Surrounding whitespace and comments stay intact. For **new keys**, they are
appended to the relevant section/object. The new key inherits the formatting
conventions of its siblings (indentation, trailing commas, etc.) as best as the
editor library supports.

### Format-specific editors

Each format needs a library that can parse source text into an editable tree and
write it back losslessly.

| Format       | Editor                      | Source                              |
|--------------|-----------------------------|-------------------------------------|
| JSON / JSON5 | `json_edit`                 | New crate in `crates/contrib`       |
| YAML         | [`yaml-edit`][yaml-edit]    | External crate                      |
| TOML         | [`toml_edit`][toml-edit] or | See [TOML strategy](#toml-strategy) |
|              | `json_edit`-derived         |                                     |

#### JSON and JSON5: `json_edit`

No format-preserving JSON editor exists in the Rust ecosystem. We build one as
`crates/contrib/json_edit`, designed for eventual publication to crates.io.

The crate is built on [`rowan`][rowan], the lossless syntax tree library created
for rust-analyzer. `yaml-edit` already uses `rowan`, so this is a shared
dependency rather than a new one.

JSON's grammar is LL(1) and trivial compared to YAML. The full crate is
estimated at ~900 lines including tests:

| Component                                | Est. lines | Notes                               |
|------------------------------------------|------------|-------------------------------------|
| `SyntaxKind` enum + rowan glue           | ~50        | ~15 token types                     |
| Lexer                                    | ~150       | String escapes are the hardest part |
| Parser                                   | ~150       | Recursive descent                   |
| High-level API (`Document`, `Object`,    | ~250       | `get`, `set`, `remove`, `insert`    |
| `Array`)                                 |            |                                     |
| Tests                                    | ~300       |                                     |

JSON5 extends JSON with: single-line comments (`//`), block comments (`/* */`),
trailing commas, unquoted identifier keys, single-quoted strings, and additional
number literals. These are handled in the lexer; the parser and high-level API
are nearly identical.

The public API:

```rust
let doc = json_edit::Document::parse(input)?;
let root = doc.as_object()?;
root.set("key", json_edit::Value::from("value"));
root.remove("old_key");
println!("{}", doc); // lossless output
```

#### YAML: `yaml-edit`

[`yaml-edit`][yaml-edit] is an existing crate that provides exactly what we
need: lossless YAML parsing and editing, also built on `rowan`. It preserves
comments, whitespace, and formatting through in-place syntax tree mutations.

Its API maps directly to our deep-merge pattern:

```rust
let doc = yaml_edit::Document::from_str(content)?;
let mapping = doc.as_mapping()?;
mapping.set("key", "value");    // preserves surrounding formatting
mapping.remove("old_key");
```

`yaml-edit` is at version 0.2.1 with ~1.7k downloads. The API is clean but the
crate is young. We should pin to a specific version and monitor for breaking
changes.

#### TOML strategy

[`toml_edit`][toml-edit] is the established solution for lossless TOML editing
(it underpins Cargo's `Cargo.toml` manipulation). It works well for our use
case, but has one documented limitation: it does not preserve the order of
dotted keys (e.g., `a.b.c = 1` may be rewritten as a nested table).

Two options:

1. **Use `toml_edit` as-is.** The dotted-key limitation is minor — most users
   write section headers (`[section]`) rather than deeply dotted keys. This is
   the pragmatic choice.

2. **Build a `toml_edit` equivalent on top of `json_edit`'s rowan
   infrastructure.** After building `json_edit`, we have a lexer/parser
   framework that could be extended to TOML. TOML's grammar is more complex than
   JSON but simpler than YAML. This gives us full control over formatting
   preservation, including dotted keys. The cost is ~1500-2000 additional lines.

This RFD recommends option 1 for the initial implementation. If the dotted-key
limitation causes real user complaints, option 2 becomes viable with low
marginal effort since the rowan infrastructure already exists.

### Integration with `ConfigFile`

`ConfigFile::merge_delta` becomes the single entry point. It dispatches to the
appropriate editor based on `self.format`:

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

Each `merge_*` method follows the same pattern:

1. Parse `self.content` into the format's lossless document type.
2. Serialize `delta` to a `serde_json::Value` (a common intermediate
   representation that all formats can consume).
3. Deep-merge the intermediate value into the lossless document.
4. Write the document back to `self.content`.

Step 2 uses `serde_json::Value` as the interchange format because all four
formats can round-trip through it without loss for the subset of types
`PartialAppConfig` uses (strings, numbers, booleans, arrays, objects).

### Interaction with `config fmt`

The `config fmt` command intentionally normalizes formatting — it runs a full
deserialize-serialize round-trip. This RFD does not change `config fmt`. The
distinction is:

- `config set`: preserve the user's formatting, change only what was requested.
- `config fmt`: normalize the entire file to a canonical format.

`format_content` continues to use the serde round-trip.

## Drawbacks

- **New dependency on `rowan`** (~4 crates in its dependency tree). Mitigated by
  the fact that `yaml-edit` already brings it in.
- **Maintenance cost of `json_edit`**. We own this crate. JSON's grammar is
  stable and small, so ongoing maintenance should be minimal, but it is
  nonzero.
- **Three different editor backends.** Each format has its own tree
  representation and API. The `merge_delta` dispatcher and the shared
  deep-merge-via-`serde_json::Value` pattern keep the integration surface
  small, but bugs could manifest differently per format.

## Alternatives

### Span-guided string surgery

Parse the original file with a span-tracking parser (e.g.,
`json-spanned-value`), then use byte offsets to surgically splice replacement
values into the raw string. This avoids needing a full lossless tree.

Rejected because: insertion of new keys requires finding the right position,
matching indentation, and handling trailing commas — essentially rebuilding
the formatting logic that a lossless tree provides for free. Also,
`json-spanned-value` only handles standard JSON (not JSON5) and depends on
`indexmap` v1 (we use v2).

### Serde round-trip with `preserve_order`

Use `serde_json` with `preserve_order` for JSON, accept the whitespace
normalization, and only fix TOML/YAML.

Rejected because: key ordering is only part of the problem. Comment
preservation matters for JSON5 and YAML, and indentation normalization creates
diff noise even with preserved ordering.

### Do nothing

Accept the noisy diffs.

Rejected because: users commit config files to version control. Noisy diffs
from `config set` erode trust in the tool and discourage use of the command.

## Non-Goals

- **Conflict-free concurrent editing.** `config set` reads, modifies, and writes
  the file non-atomically. This RFD does not add file locking or merge conflict
  resolution.
- **Schema migration.** If a config key is renamed across JP versions, that is
  handled by the config loading pipeline, not by the editor.
- **Formatting preferences.** This RFD preserves existing formatting; it does
  not allow users to configure a preferred style for new keys (e.g., "always use
  2-space indent"). New keys inherit the style of their surroundings as best as
  the editor library supports.

## Risks and Open Questions

- **`yaml-edit` maturity.** The crate is at 0.2.1. It may have parsing bugs on
  edge-case YAML documents. Mitigation: pin the version, add integration tests
  against realistic JP config files, and contribute fixes upstream.
- **JSON5 comment variants.** JSON5 allows `//` and `/* */` comments. The
  `json_edit` lexer must handle both, including comments inside objects and
  arrays. This is straightforward but must be tested thoroughly.
- **`toml_edit` dotted key reordering.** If users rely on dotted keys like
  `a.b.c = 1`, `toml_edit` may rewrite them as nested tables. The [toml_edit
  issue tracker][toml-edit-dotted] documents this. Monitor for user reports.
- **Empty delta serialization.** `PartialAppConfig` skips `None` fields during
  serialization. If the delta is empty (all fields `None`), the serialized
  output is `""` or `{}`. The merge must handle this as a no-op without
  corrupting the file.

## Implementation Plan

### Phase 1: `json_edit` crate

Create `crates/contrib/json_edit` with:

- Rowan-based lexer and parser for JSON and JSON5.
- `Document`, `Object`, `Array`, `Value` wrapper types.
- `set`, `remove`, `insert` operations that preserve formatting.
- Unit tests covering: comment preservation, whitespace preservation, key
  ordering, nested edits, trailing commas (JSON5), unquoted keys (JSON5).

Can be reviewed and merged independently. No changes to JP's config pipeline.

### Phase 2: JSON/JSON5 integration

Wire `json_edit` into `ConfigFile::merge_delta` for `Format::Json` and
`Format::Json5`. Add integration tests that verify `config set` on JSON and
JSON5 files produces minimal diffs.

Depends on Phase 1.

### Phase 3: YAML integration

Add `yaml-edit` as a dependency. Wire it into `ConfigFile::merge_delta` for
`Format::Yaml`. Add integration tests with comment-heavy YAML configs.

Independent of Phases 1-2. Can be done in parallel.

### Phase 4: TOML integration

Add `toml_edit` as a dependency. Wire it into `ConfigFile::merge_delta` for
`Format::Toml`. Add integration tests verifying alias strings, key order, and
comments are preserved.

Independent of Phases 1-3. Can be done in parallel.

### Phase 5 (optional): Rowan-based TOML editor

If the `toml_edit` dotted-key limitation or other formatting issues prove
problematic, build a TOML lexer/parser on the same rowan infrastructure as
`json_edit`. This replaces the `toml_edit` dependency.

Depends on Phase 1 (for the rowan infrastructure) and Phase 4 (for user
feedback on `toml_edit` limitations).

## References

- [`toml_edit`][toml-edit] — format-preserving TOML editor (used by Cargo)
- [`yaml-edit`][yaml-edit] — lossless YAML parser/editor built on rowan
- [`rowan`][rowan] — generic lossless syntax tree library (from rust-analyzer)
- [`json-spanned-value`][json-spanned] — span-tracking JSON parser (considered
  and rejected)
- [toml_edit dotted key issue][toml-edit-dotted]

[toml-edit]: https://docs.rs/toml_edit
[yaml-edit]: https://docs.rs/yaml-edit
[rowan]: https://docs.rs/rowan
[json-spanned]: https://docs.rs/json-spanned-value
[toml-edit-dotted]: https://github.com/toml-rs/toml/issues/163
