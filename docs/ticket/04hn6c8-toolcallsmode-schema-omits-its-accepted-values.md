# `ToolCallsMode` schema omits its accepted values

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-18

`ToolCallsMode` hand-writes its `Schematic` impl and returns a bare string
(`crates/jp_config/src/conversation/compaction.rs:590-593`):

```rust
impl schematic::Schematic for ToolCallsMode {
    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        schema.string_default()
    }
}
```

So the four values it actually accepts never reach the schema. A consumer
validating a workspace config against it accepts `tool_calls = "nonsense"`, and
an editor driven by the schema offers no completion for a key whose whole
vocabulary is four fixed strings.

It is the only `string_default()` in `jp_config`. Its sibling on the same rule,
`ReasoningMode`, derives `ConfigEnum`, which generates
`schema.enumerable(EnumType::from_fields(...))`
(`crates/contrib/schematic_macros/src/config_enum/mod.rs:270-280`) and does
carry its variants. The two keys sit next to each other in
`CompactionRuleConfig` and describe their values to a schema consumer
differently.

## Why it is hand-written

Not an oversight to delete — `FromStr` accepts two or three spellings per
variant:

| Variant          | Accepted                                     |
| ---------------- | -------------------------------------------- |
| `Strip`          | `strip`, `s`                                 |
| `StripResponses` | `strip-responses`, `strip_responses`, `sres` |
| `StripRequests`  | `strip-requests`, `strip_requests`, `sreq`   |
| `Omit`           | `omit`, `o`                                  |

`ConfigEnum` supports a single `alias` per variant
(`crates/contrib/schematic_macros/src/config_enum/variant.rs:109-112`), so
deriving it is not a drop-in for the two variants that need three spellings.
The hand-written impl exists to keep the aliases; only the *schema* half was
left as a placeholder.

## The decision the fix forces

Which spellings the schema lists is a real choice, not a detail:

- **Canonical only** (`strip`, `strip-requests`, `strip-responses`, `omit`)
  gives clean completion, but a schema-validating editor then flags
  `tool_calls = "sres"` as invalid even though `jp` accepts it. The tool and its
  schema would disagree about valid input.
- **Every accepted spelling** (ten values) keeps them in agreement but makes
  completion noisy, and pushes the aliases into a published contract that is
  currently only an input convenience.

Worth settling before writing the `EnumType`, since it determines whether the
schema describes what `jp` accepts or what `jp` recommends.

Two implementation routes:

1. Hand-write an `EnumType` schema alongside the existing hand-written
   `FromStr` / `Serialize` / `Display`. Smallest change, keeps the divergence
   between how the two enums on this rule are described.
2. Extend `ConfigEnum` to take multiple aliases per variant and derive the whole
   thing. Larger, touches the vendored macro crate, but removes the one-off and
   would apply to any future enum with more than one alias.

## Severity

No internal consumer is affected. `AppConfig::schema()`'s only in-tree use is
`jp_conversation::compat::strip_unknown_fields`, which reads struct field
*names* to drop keys that no longer exist and returns early on any non-`Struct`
node (`crates/jp_conversation/src/compat.rs:64-67`), so a value's type is never
consulted. Config loading rejects a bad mode string on its own via `FromStr`,
independent of the schema.

The gap is in what JP tells external tooling. There is no generated `AppConfig`
schema checked into the repository today, so nothing is currently shipping the
wrong thing — the cost is paid the first time something consumes it.

## Why it is filed rather than fixed in place

Raised in review of PR \#994, which added an `over` size threshold to the
compaction policies and wrapped both mode enums in `PolicySpec<P>`. That PR
fixed `PolicySpec`'s own schema, which had collapsed to "any JSON value" and
erased whatever `P` contributed. Fixing `P` itself is a separate change: it
touches a type \#994 otherwise leaves alone, and it needs the
canonical-versus-aliases decision above, which \#994 has no reason to make.
