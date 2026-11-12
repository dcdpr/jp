# Warn on unknown config fields instead of rejecting them

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: package=jp_config
- **Label**: package=jp_label
- **Label**: package=schematic
- **Label**: type=enhancement

A configuration written for a newer JP fails to load in an older one.
`#[derive(Config)]` types reject unknown fields by default: `schematic_macros`
emits `deny_unknown_fields` unless a type opts out with `allow_unknown_fields`
(`ToolsConfig`, `ModelConfig`, and `ParametersConfig` do).
A plugin's label vocabulary does the same (`jp_label::vocabulary::Entry`), so
adding a vocabulary field such as `type` (T-1hva7bz) breaks every plugin binary
not rebuilt with it.

The opposite default has the opposite cost: a typo such as `max_retires = 3`
loads without complaint.
Neither default suits every user, so make it a setting.

## Proposal

A config loading strictness setting, with two levels:

- **Lenient (default).** An unknown field is dropped with a warning that names
  its path and the file or flag it came from.
  A configuration targeting a newer JP still loads.
- **Strict.** An unknown field is an error, so a typo is caught where it is
  made.
  Fields from a newer JP version also fail unless their paths are explicitly
  allowed, letting strict users opt into newer fields before upgrading.

Some types remain strict regardless of the setting, because ignoring a
misspelled key would widen permissions rather than narrow them.
`PolicySpec` rejects unknown keys for this reason: ignoring a typo like `oer =
"1MB"` (for `over = "1MB"`) applies the policy to every item instead of only
large ones.
Tool access rules are in the same position.
The config schema marks these types, and both levels reject unknown fields
inside them.

Sketch, names to be settled:

```toml
[config]
unknown_fields = "warn"   # or "error"
allow_unknown = ["providers.llm.newprovider"]
```

## Design notes

- **Detection stays out of the type tree.** The `#[derive(Config)]` types accept
  unknown fields, and the loader (`jp_config::util` for files, the `--cfg`
  parser, `JP_CFG_*`) detects unknown keys when reading a partial and returns
  their paths for the CLI to warn on or reject.
  The schema walk in `jp_conversation::compat` (`strip_unknown_fields`) already
  finds unknown paths for stored conversation configs; reuse it rather than
  adding `serde_ignored`.
  Keys captured by an intentional catch-all map, such as
  `ParametersConfig::other`, are not treated as unknown.
- **The strict marker** replaces `allow_unknown_fields` in `schematic_macros`:
  types are lenient unless marked, the inverse of today.
- **Where the setting is read.** Because the setting governs how config layers
  are loaded, it must be extracted from the raw layers before validation or read
  from an environment variable.
  Decide which mechanism to use and whether a later layer can relax a strict
  setting from an earlier layer.
- **Stored conversation config** keeps its own recovery in
  `jp_conversation::compat`: a conversation written by a newer JP has to load
  whatever the setting says.
- **Vocabularies.** `Vocabulary::from_value` returns its unknown fields
  alongside the vocabulary.
  Writes follow the setting; `jp ticket check` and `jp wiki check` always report
  them, so CI stays strict.
  This is the split T-1ah5z7x proposes for undeclared labels.

## Out of scope

- `jp_plugin` protocol messages, which also deny unknown fields: a wire contract
  between host and plugin is a separate decision.

- `comfort` and `qual` configuration: standalone tools, not JP config.

- [ ] `#[derive(Config)]` types accept unknown fields unless marked strict

- [ ] Access and policy types are marked strict

- [ ] Unknown keys from files, `--cfg`, and `JP_CFG_*` warn with path and source

- [ ] Strict level errors on unknown keys, except allowed paths

- [ ] `Vocabulary` returns unknown fields; plugins follow the setting on write,
      `check` reports them

- [ ] Tests that pin rejection are updated, such as
      `fan_out_rejects_an_unknown_field`
