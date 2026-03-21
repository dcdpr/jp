# RFD 060: Config Explain

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21

## Summary

Add a global `--explain` flag that prints the config resolution chain for all or
a specific field, then exits without running the command. This gives users
visibility into JP's layered configuration system — which files, environment
variables, CLI flags, and conversation deltas contribute to the final resolved
value.

## Motivation

JP's configuration is loaded from up to 9 sources, merged in a specific order:

1. User global config (`$XDG_CONFIG_HOME/jp/config.toml`)
2. Workspace config (`.jp/config.toml`)
3. CWD config (`.jp.toml`, recursive upwards)
4. User workspace config (`$XDG_DATA_HOME/jp/<id>/config.toml`)
5. Environment variables (`JP_CFG_*`)
6. Conversation delta (stored in the active conversation's event stream)
7. CLI `--cfg` arguments
8. Command-specific CLI flags (`--model`, `--reasoning`, `--tool`, etc.)
9. Defaults (applied last via `default_values`)

When a config value isn't what the user expects, they have no way to determine
which layer set it. The current debugging workflow is: check `config.toml`,
check env vars, check conversation history, check CLI flags, give up and ask
someone. This is particularly painful for:

- **Model ID resolution**: `--model=opus` resolves through the alias system to
  `anthropic/claude-sonnet-4-5`. Users can't see this transformation.
- **Inheritance**: A workspace config sets `inherit = false`, silently ignoring
  the global config. Users don't know why their global settings aren't applying.
- **Conversation deltas**: A previous `jp config set` or editor-provided config
  change is persisted in the conversation stream and silently overrides file
  config. Users don't know the conversation is carrying state.
- **Conflicting layers**: An env var sets one value, a `--cfg` flag sets
  another. Which wins?

`--explain` makes the resolution chain visible, turning "why is my config wrong"
from a debugging session into a single command.

## Design

### User experience

#### Broad mode (no field specified)

```sh
$ jp query --model=opus --no-reasoning --explain
```

Prints all fields that differ from their defaults, grouped by the layer that set
them:

```txt
Config resolution for `jp query --model=opus --no-reasoning`:

  User global config (~/.config/jp/config.toml):
    assistant.name = "JP"
    style.reasoning.display = "full"
    providers.llm.aliases.opus = "anthropic/claude-opus-4-6"

  Workspace config (/home/user/project/.jp/config.toml):
    conversation.tools.*.run = "ask"
    assistant.instructions.0 = { title = "Rust", ... }

  Environment:
    (none)

  Conversation delta (conversation jp-c1234):
    assistant.model.id = "anthropic/claude-haiku-4-5"

  CLI --cfg:
    (none)

  CLI flags:
    assistant.model.id = "anthropic/claude-opus-4-6"  (--model=opus, resolved alias)
    assistant.model.parameters.reasoning = "off"  (--no-reasoning)

  Final resolved config:
    assistant.model.id = "anthropic/claude-opus-4-6"
    assistant.model.parameters.reasoning = "off"
    assistant.name = "JP"
    conversation.tools.*.run = "ask"
    providers.llm.aliases.opus = "anthropic/claude-opus-4-6"
    style.reasoning.display = "full"
    ... (2 more fields)
```

The "Final resolved config" section shows the merged result — what the command
would actually run with. Only non-default fields are shown.

#### Focused mode (specific field)

```sh
$ jp query --model=opus --explain=assistant.model.id
```

Traces a single field through every layer:

```txt
assistant.model.id = "anthropic/claude-opus-4-6"

  Resolution chain:
    1. User global     (~/.config/jp/config.toml)    (not set)
    2. Workspace       (.jp/config.toml)             (not set)
    3. CWD             (not found)
    4. User workspace  (not found)
    5. Environment     JP_CFG_ASSISTANT_MODEL_ID      (not set)
    6. Conversation    (conversation zy2a)            "anthropic/claude-haiku-4-5"
    7. CLI --cfg                                      (not set)
    8. CLI flag        --model=opus                   "opus" → alias → "anthropic/claude-opus-4-6"

  Documentation:
    The model to use for the assistant.
    Format: provider/model-name or an alias defined in providers.llm.aliases.
```

The focused mode shows every layer, including ones that didn't set the field
("not set") and config files that don't exist ("not found"). This makes the full
resolution chain visible — users can see exactly where their value came from and
what it overrode.

The "Documentation" section is pulled from the schema description (the `///` doc
comment on the corresponding `AppConfig` field using `schematic`s schema
introspection).

#### Dry-run semantics

`--explain` suppresses command execution. The command is parsed, config is fully
resolved, and the provenance report is printed to stdout. The command itself
(query, conversation ls, etc.) does not run.

This follows the precedent set by `terraform plan`, `docker compose config`, and
`systemd-analyze`. The user re-runs without `--explain` to execute.

#### JSON output

`--explain` respects the `--format` flag:

```sh
$ jp query --model=opus --explain --format=json
```

```json
{
  "field": null,
  "layers": [
    {
      "type": "file",
      "name": "user_global",
      "path": "~/.config/jp/config.toml",
      "config": {
        "assistant": { "name": "JP" },
        "style": { "reasoning": { "display": "full" } }
      }
    },
    ...
  ],
  "resolved": {
    "assistant": { "model": { "id": "anthropic/claude-opus-4-6" } },
    ...
  }
}
```

This enables scripting: `jp query --explain --format=json | jq
'.resolved.assistant.model.id'`.

### Architecture

#### Snapshot-and-diff provenance

Rather than threading provenance metadata through the config merge pipeline, we
reconstruct it on demand. When `--explain` is active, `load_partial_config()`
takes a JSON snapshot of the `PartialAppConfig` after each layer, alongside
metadata about that layer (name, file path).

```rust
struct ConfigSnapshot {
    /// Human-readable layer name.
    name: &'static str,

    /// File path, if the layer comes from a file.
    path: Option<Utf8PathBuf>,

    /// The cumulative PartialAppConfig state after this layer was merged.
    state: PartialAppConfig,
}
```

After all layers are processed, the provenance for any field is determined by
diffing adjacent snapshots using `PartialAppConfig::delta()` — the same method
already used for conversation config deltas and config persistence:

```rust
fn layer_contributions(
    prev: &ConfigSnapshot,
    next: &ConfigSnapshot
) -> PartialAppConfig {
    prev.state.delta(next.state.clone())
}
```

A non-empty field in the delta means that layer changed the value. `delta()`
already handles nested structs, `IndexMap`-based tool configs, and all the
custom merge strategies in the config system.

#### Layer definitions

The layers correspond to the steps in `load_partial_config()`:

| Layer | Name             | Source                                   |
|-------|------------------|------------------------------------------|
| 1     | `user_global`    | `$XDG_CONFIG_HOME/jp/config.toml` +      |
|       |                  | extends                                  |
| 2     | `workspace`      | `.jp/config.toml` + extends              |
| 3     | `cwd`            | `.jp.toml` (recursive upwards)           |
| 4     | `user_workspace` | `$XDG_DATA_HOME/jp/<id>/config.toml` +   |
|       |                  | extends                                  |
| 5     | `environment`    | `JP_CFG_*` variables                     |
| 6     | `conversation`   | Active conversation's config delta       |
| 7     | `cli_cfg`        | `--cfg KEY=VALUE` arguments              |
| 8     | `cli_flags`      | Command-specific flags (`--model`, etc.) |
| 9     | `defaults`       | `PartialAppConfig::default_values()`     |

#### Extends sub-layers

Config files can use `extends` to include other files. For example, a workspace
config might have:

```toml
extends = ["config.d/**/*"]
```

This causes `.jp/config.d/tools.toml`, `.jp/config.d/style.toml`, etc. to be
merged into the workspace layer. `--explain` decomposes these into sub-layers so
the user can see exactly which file set a value:

```txt
  Workspace config (.jp/config.toml):
    assistant.model.id = "anthropic/claude-sonnet-4-5"

    extends: .jp/config.d/tools.toml:
      conversation.tools.*.run = "ask"

    extends: .jp/config.d/style.toml:
      style.reasoning.display = "full"
```

The implementation uses `load_config_file_with_extends()` which already
processes extends files individually. Each call to `loader.file()` for an
extended file gets its own snapshot. The snapshot metadata records both the
parent file and the extends path:

```rust
struct ConfigSnapshot {
    name: &'static str,
    path: Option<Utf8PathBuf>,
    /// If this is an extends sub-layer, the parent file that declared it.
    extends_parent: Option<Utf8PathBuf>,
    state: PartialAppConfig,
}
```

The output groups sub-layers under their parent, indented to show the
relationship. In focused mode, the trace shows the specific extends file:

```txt
    2a. Workspace       (.jp/config.toml)              (not set)
    2b. Workspace ext   (.jp/config.d/tools.toml)      "ask"
    2c. Workspace ext   (.jp/config.d/style.toml)      (not set)
```

The `before` and `after` ordering of extends paths (controlled by
`ExtendingRelativePath::is_before`) is preserved in the snapshot sequence —
`before` extensions appear as sub-layers before the parent file, `after`
extensions appear after it.

Layers 1-4 are merged by `load_partials_with_inheritance()`. When `inherit =
false` is set, earlier layers are skipped — the snapshot still records them, but
marks them as `skipped: true` so the output can show the user that inheritance
was disabled.

Layer 5 (environment variables) has higher precedence than file layers (1-4),
per the documented ordering in `configuration.md`.

Layer 9 (defaults) is applied by `build()` via
`default_values().merge(partial)`. This layer fills in any fields not set by the
earlier layers.

#### CLI flag reverse-mapping

Layer 8 (CLI flags) needs special handling. The `apply_cli_config()` method on
each command converts typed CLI flags (like `--model=opus`) into
`PartialAppConfig` mutations. But the snapshot only sees the resulting partial —
it doesn't know that `assistant.model.id = "anthropic/claude-sonnet-4-5"` came
from `--model=opus` with alias resolution.

The approach is to record provenance at the point where the mapping happens —
inside `apply_cli_config()` itself. The code there already knows both sides: it
reads the CLI flag value and writes the config field. We add an optional
recorder parameter that captures this relationship:

```rust
trait IntoPartialAppConfig {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
        recorder: Option<&mut CliRecorder>,  // new
    ) -> Result<PartialAppConfig, BoxedError>;
}

struct CliRecord {
    /// The config field path. Compile-time constant.
    field: &'static str,
    /// The CLI flag that set it. Compile-time constant.
    flag: &'static str,
    /// The raw value as provided by the user.
    raw_value: String,
    /// Optional note (e.g., "resolved via alias").
    note: Option<String>,
}

struct CliRecorder(Vec<CliRecord>);
```

The helper functions that bridge CLI flags to config fields record their
assignments:

```rust
fn apply_model(
    partial: &mut PartialAppConfig,
    model: Option<&str>,
    recorder: Option<&mut CliRecorder>,
) {
    let Some(id) = model else { return };
    partial.assistant.model.id = id.into();

    if let Some(rec) = recorder {
        rec.record("assistant.model.id", "--model", id, None);
    }
}
```

The `&'static str` for `field` and `flag` means these are compile-time
constants, not runtime strings constructed elsewhere. The mapping lives next to
the code that performs the mapping — the only place that can keep it accurate.

During normal execution (no `--explain`), the recorder is `None` and no
allocation occurs. When `--explain` is active, `run_inner()` passes
`Some(&mut recorder)` and the records are used to annotate the CLI flags layer
in the output.

The recorder does not replace the snapshot diff — it augments it. If a flag sets
a field but forgets to record it, the diff still shows the field changed at the
CLI layer. The output just lacks the friendly flag name. The failure mode is
"degraded display" not "wrong data."

##### Drift prevention

A test validates that all recorded field paths are real `AppConfig` fields:

```rust
#[test]
fn cli_recorder_field_paths_are_valid() {
    let fields: HashSet<_> = AppConfig::fields().into_iter().collect();
    let mut recorder = CliRecorder::default();
    // Run apply_cli_config with a populated Query struct
    // ...
    for record in &recorder.0 {
        assert!(
            fields.contains(record.field),
            "CLI recorder references unknown field: {}",
            record.field,
        );
    }
}
```

This follows the same pattern as the existing
`test_ensure_no_missing_assignments` test — it catches field path drift at test
time.

#### Schema documentation

Focused mode shows documentation for the explained field. This comes from the
`schematic::Schema` description, which is populated from the `///` doc comments
on `AppConfig` fields:

```rust
fn field_description(field_path: &str) -> Option<String> {
    use schematic::{SchemaBuilder, SchemaType, Schematic as _};

    let builder = SchemaBuilder::default();
    let mut stack = vec![(AppConfig::build_schema(builder), "")];

    // Walk the schema tree to find the field and return its description.
    // Same traversal pattern as AppConfig::fields().
}
```

This reuses the same schema walking logic already in `AppConfig::fields()`.

#### Integration point

`--explain` is a global flag on `Globals`:

```rust
#[derive(Debug, Default, clap::Args)]
struct Globals {
    // ... existing fields ...

    /// Explain config resolution and exit without running the command.
    ///
    /// Without a value, shows all non-default fields grouped by source.
    /// With a field path, traces that field through the resolution chain.
    #[arg(long = "explain", global = true, value_name = "FIELD")]
    explain: Option<Option<String>>,
}
```

`Option<Option<String>>`:
- `None` — flag not provided, normal execution.
- `Some(None)` — bare `--explain`, broad mode.
- `Some(Some("assistant.model.id"))` — focused mode for a specific field.

In `run_inner()`, after `load_partial_config()` completes (with snapshots
collected), the explain check runs:

```rust
if let Some(field) = &cli.globals.explain {
    let report = build_explain_report(&snapshots, field.as_deref());
    printer.println(format_explain_report(&report, format));
    return Ok(());
}
```

This runs after config loading but before `Ctx::new()`, workspace stream
initialization, and `Commands::run()`. The command is never executed.

### Field validation

In focused mode, the provided field path is validated against
`AppConfig::fields()`. If the field doesn't exist, an error is printed with
suggestions (using the same fuzzy matching that `missing_key()` in
`assignment.rs` uses):

```
Error: Unknown config field 'assistant.model'

Did you mean one of:
  assistant.model.id
  assistant.model.parameters.max_tokens
  assistant.model.parameters.reasoning
  assistant.model.parameters.temperature
```

## Drawbacks

- **Snapshot cloning cost**: Cloning `PartialAppConfig` at each of the ~9 layers
  adds a small overhead to every `--explain` invocation. In practice this is
  sub-millisecond and only runs when `--explain` is present.

- **CLI recorder is opt-in per call site**: Each `apply_*` helper that bridges a
  CLI flag to a config field needs a `recorder.record(...)` call. Forgetting to
  add one when a new flag is introduced means the diff still shows the field
  changed, but without the flag annotation. A test validates that recorded field
  paths are valid, but cannot detect missing recordings — that requires code
  review discipline.

- **Inheritable config complexity**: When `inherit = false` is set, the explain
  output needs to show which layers were skipped and why. This adds conditional
  logic to the output formatting.

## Alternatives

### Eager provenance tracking

Thread a `ProvenanceMap<String, (Value, LayerName)>` through the entire merge
pipeline. Every `load_partial()` and `assign()` call records where each field
was set.

Rejected because: it adds complexity to every config load (even when `--explain`
is not used), requires modifying the `PartialAppConfig` merge infrastructure,
and the snapshot approach achieves the same result with less invasive changes.

### `jp config explain` subcommand

A dedicated subcommand under `jp config` instead of a global flag.

Rejected because: it can't show CLI flag resolution. `jp config explain
assistant.model.id` doesn't know that `--model=opus` would resolve to
`anthropic/claude-sonnet-4-5`, because no command context is provided. The
global flag captures the full resolution chain including command-specific
transformations.

### Verbose logging (`-vvv`)

Users can already get config resolution details via `jp query -vvv`, which
outputs trace logs that include the merged config JSON. This shows the final
state but not the per-layer provenance.

Not a replacement: trace logs are noisy, unstructured, and not designed for
end-user consumption. `--explain` provides a curated, human-readable view of
exactly what the user needs.

## Non-Goals

- **Config editing**: `--explain` is read-only. It doesn't modify config files
  or suggest corrections. Config editing is the domain of `jp config set` and a
  future interactive `--cfg` feature.

- **Runtime config changes**: `--explain` shows the static config resolution at
  startup. It doesn't show config changes that happen during a turn (e.g.,
  editor-provided config in the query editor).

- **Dependency tracking**: `--explain` doesn't show *why* a config value matters
  (e.g., "reasoning is off because your model doesn't support it"). It shows
  where the value came from, not what it does.

## Risks and Open Questions

- **Tool config fields**: The `conversation.tools.*` fields use dynamic keys
  (tool names). The snapshot diff will show these correctly, but the field
  validation in focused mode needs to handle wildcard patterns
  (`conversation.tools.fs_read_file.run`) that aren't in the static
  `AppConfig::fields()` list.

- **Conversation delta readability**: The conversation delta is a
  `PartialAppConfig` stored in the event stream. For `--explain`, we show its
  fields as flat key-value pairs. But the delta might contain complex nested
  structures (tool configs, system prompt sections). The formatting needs to
  handle these gracefully — likely by truncating long values and showing a
  `use --explain=<field> for details` hint.

## Implementation Plan

### Phase 1: Snapshot infrastructure

1. Define `ConfigSnapshot` and `ExplainReport` types in a new `jp_cli::explain`
   module.
2. Add snapshot collection points to `load_partial_config()`, gated behind a
   boolean flag (only collect when `--explain` is active). This includes
   sub-layer snapshots for each extends file.
3. Refactor `load_config_file_with_extends()` to accept an optional snapshot
   collector, taking a snapshot after each extends file is loaded.
4. Implement the diff logic: given a sequence of snapshots (including
   sub-layers), determine which fields changed at each layer.
5. Unit test the diff logic with known partial configs, including extends
   scenarios.

### Phase 2: Output formatting

1. Implement broad mode: group changed fields by layer, print to stdout.
2. Implement focused mode: trace a single field through all layers.
3. Add schema documentation lookup for focused mode.
4. Add JSON output format.
5. Add field validation with suggestions for unknown field paths.

### Phase 3: CLI integration

1. Add `--explain` to `Globals`.
2. Wire the explain check into `run_inner()`, after config loading.
3. Add `recorder: Option<&mut CliRecorder>` parameter to
   `IntoPartialAppConfig::apply_cli_config` and thread it through the
   `apply_*` helper functions in `Query` and other command structs.
4. Add `cli_recorder_field_paths_are_valid` test.
5. Test with representative scenarios: alias resolution, inheritance cutoff,
   conversation deltas, env var overrides.

Phases 1 and 2 can be developed and tested independently. Phase 3 integrates
them into the CLI and can be merged as a single PR.

## References

- [RFD 059]: Shell completions and man pages
- Future RFD: Interactive config (bare `--cfg` flag)
- `load_partial_config()` in `jp_cli/src/lib.rs` — the 9-layer merge pipeline
- `load_config_file_with_extends()` in `jp_config/src/util.rs` — extends
  resolution
- `AppConfig::fields()` in `jp_config/src/lib.rs` — schema-driven field
  enumeration
- `PartialAppConfig::delta()` in `jp_config/src/delta.rs` — field-level diffing
- `terraform plan` — precedent for dry-run config explanation
- `git config --show-origin --list` — precedent for config provenance display

[RFD 059]: 059-shell-completions-and-man-pages.md
