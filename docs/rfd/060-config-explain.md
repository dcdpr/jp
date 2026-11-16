# RFD 060: Config Explain

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21
- **Required by**: [RFD 061], [RFD 063]
- **Summary**: Adds a global `--explain` flag that prints the layer-by-layer
  origin of resolved configuration fields and exits.

## Summary

This RFD adds a global `--explain` flag that prints the configuration resolution
chain for all fields or a selected field path, then exits without running the
command.
The output shows which config files, environment variables, conversation deltas,
and CLI flags contributed to each resolved value.

## Motivation

JP loads configuration from up to 9 sources, merged in order:

1. User global config (`$XDG_CONFIG_HOME/jp/config.toml`)
2. Workspace config (`.jp/config.toml`)
3. CWD config (`.jp.toml`, recursive upwards)
4. User workspace config (`$XDG_DATA_HOME/jp/workspace/<name>-<id>/config.toml`)
5. Environment variables (`JP_CFG_*`)
6. Conversation delta (stored in the active conversation's event stream)
7. CLI `--cfg` arguments
8. Command-specific CLI flags (`--model`, `--reasoning`, `--tool`, etc.)
9. Defaults (applied last via `default_values`)

When a resolved value is unexpected, nothing indicates which layer set it.
Debugging means checking `config.toml`, environment variables, conversation
history, and CLI flags by hand.
Several cases are especially opaque:

- **Model ID resolution**: `--model=opus` resolves through the alias system to
  `anthropic/claude-sonnet-4-5`, with no output showing the alias expansion.
- **Inheritance**: A workspace config with `inherit = false` silently discards
  the user global config.
- **Conversation deltas**: Config changes from an earlier `jp config set` or
  query editor session persist in the conversation stream and silently override
  file config.
- **Conflicting layers**: When an environment variable and a `--cfg` flag set
  different values for the same field, the user has to know the precedence table
  to tell which wins.
- **`extends` vs `--cfg` resolution**: `extends` paths resolve only relative to
  the declaring file, whereas `--cfg <name>` searches all three config roots
  ([RFD 035]) and merges every match.
  A workspace file with `extends = ["../skill/web.toml"]` loads only the
  workspace file, with no signal that a matching user-global `skill/web.toml`
  was ignored.

`--explain` prints the resolution chain directly, replacing manual debugging
with a single command.

## Design

### User experience

#### Broad mode (no field specified)

```sh
$ jp query --model=opus --no-reasoning --explain
```

Without a field argument, `--explain` prints every field that differs from its
default, grouped by the layer that set it:

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

The `Final resolved config` section shows the merged non-default values that the
command would run with.

#### Focused mode (specific field)

```sh
$ jp query --model=opus --explain=assistant.model.id
```

With a field path, `--explain` traces that field through every layer:

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

Focused mode lists every layer, including layers that left the field unset (`not
set`) and config files that do not exist (`not found`), showing both the winning
source and the values it overrode.

The `Documentation` section comes from the `schematic` schema description, which
is populated from the `///` doc comment on the corresponding `AppConfig` field.

#### Dry-run semantics

`--explain` parses the CLI arguments, resolves the configuration, prints the
provenance report to stdout, and exits without running the subcommand (`query`,
`conversation ls`, etc.).
Like `terraform plan`, `docker compose config`, and `systemd-analyze`, it is a
dry-run flag; the user re-runs without `--explain` to execute the command.

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

Rather than threading provenance metadata through the config merge pipeline,
`--explain` reconstructs provenance on demand.
When `--explain` is active, `load_partial_config()` takes a JSON snapshot of the
cumulative `PartialAppConfig` after each layer, alongside the layer's name and
file path:

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

After all layers are merged, `PartialAppConfig::delta()` diffs adjacent
snapshots to find each layer's contribution, reusing the diff logic from
conversation config deltas and config persistence:

```rust
fn layer_contributions(
    prev: &ConfigSnapshot,
    next: &ConfigSnapshot
) -> PartialAppConfig {
    prev.state.delta(next.state.clone())
}
```

Any non-empty field in the delta was set or modified by that layer.
`delta()` already handles nested structs, `IndexMap`-based tool configs, and
custom merge strategies.

#### Layer definitions

The layers correspond to the steps in `load_partial_config()`:

| Layer | Name             | Source                                   |
| ----- | ---------------- | ---------------------------------------- |
| 1     | `user_global`    | `$XDG_CONFIG_HOME/jp/config.toml` +      |
|       |                  | extends                                  |
| 2     | `workspace`      | `.jp/config.toml` + extends              |
| 3     | `cwd`            | `.jp.toml` (recursive upwards)           |
| 4     | `user_workspace` | `$XDG_DATA_HOME/jp/workspace/`           |
|       |                  | `<name>-<id>/config.toml` + extends      |
| 5     | `environment`    | `JP_CFG_*` variables                     |
| 6     | `conversation`   | Active conversation's config delta       |
| 7     | `cli_cfg`        | `--cfg KEY=VALUE` arguments              |
| 8     | `cli_flags`      | Command-specific flags (`--model`, etc.) |
| 9     | `defaults`       | `PartialAppConfig::default_values()`     |

Layers 1-4 are merged by `load_partials_with_inheritance()`.
When a layer sets `inherit = false`, earlier layers are skipped; their snapshots
are still recorded with `skipped: true` so the output can report that
inheritance was disabled.

Layer 5 (environment variables) takes precedence over the file layers (1-4),
matching the ordering documented in `configuration.md`.

Layer 9 (defaults) is applied by `build()` via `default_values().merge(partial)`
to fill in any fields left unset by earlier layers.

#### Extends sub-layers

Config files can include other files via `extends`.
For example, a workspace config with:

```toml
extends = ["config.d/**/*"]
```

merges `.jp/config.d/tools.toml`, `.jp/config.d/style.toml`, and other matches
into the workspace layer.
`--explain` decomposes `extends` entries into sub-layers so the output shows
which file set each value:

```txt
  Workspace config (.jp/config.toml):
    assistant.model.id = "anthropic/claude-sonnet-4-5"

    extends: .jp/config.d/tools.toml:
      conversation.tools.*.run = "ask"

    extends: .jp/config.d/style.toml:
      style.reasoning.display = "full"
```

`load_config_file_with_extends()` already loads extended files one at a time.
Each `loader.file()` call for an extended file takes its own snapshot, recording
both the file path and the parent file that declared the `extends` entry:

```rust
struct ConfigSnapshot {
    name: &'static str,
    path: Option<Utf8PathBuf>,
    /// If this is an extends sub-layer, the parent file that declared it.
    extends_parent: Option<Utf8PathBuf>,
    state: PartialAppConfig,
}
```

Broad mode indents sub-layers under their parent file.
Focused mode lists each extended file as a numbered step:

```txt
    2a. Workspace       (.jp/config.toml)              (not set)
    2b. Workspace ext   (.jp/config.d/tools.toml)      "ask"
    2c. Workspace ext   (.jp/config.d/style.toml)      (not set)
```

The snapshot sequence preserves `before` and `after` ordering (controlled by
`ExtendingRelativePath::is_before`), placing `before` sub-layers ahead of the
parent file and `after` sub-layers behind it.

Each sub-layer shows the concrete path the entry resolved to.
This makes the difference between `extends` and `--cfg` visible: a workspace
file with `extends = ["../skill/web.toml"]` shows only the workspace's
`skill/web.toml`, revealing that no user-global file was loaded.
Named `--cfg` file arguments work the same way: an argument like `--cfg
skill/web` can resolve to multiple files across the config roots ([RFD 035]),
and each match becomes its own sub-layer under `cli_cfg`.

#### CLI flag reverse-mapping

In Layer 8 (CLI flags), each command's `apply_cli_config()` method converts
typed CLI flags such as `--model=opus` into `PartialAppConfig` mutations.
Because the snapshot only captures the resulting partial, it cannot tell that
`assistant.model.id = "anthropic/claude-sonnet-4-5"` came from `--model=opus`
through alias resolution.

To preserve that link, `apply_cli_config()` accepts an optional `CliRecorder`
and records each flag-to-field assignment as it writes the partial:

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
    /// The clap argument ID that set it (e.g. `model`, `no_reasoning`).
    /// Compile-time constant.
    arg_id: &'static str,
    /// Optional note (e.g., "resolved via alias").
    note: Option<String>,
}

struct CliRecorder(Vec<CliRecord>);
```

`CliRecord` stores only the config field path, the clap argument ID, and an
optional transformation note.
The raw value and display flag name (`--model`, `-m`) are looked up at render
time:

- **Raw value**: from `ArgMatches::get_raw(arg_id)`, available in `run_inner()`
  via Clap's `Cli::command().get_matches()`.
- **Display flag name**: from the `Command` definition via
  `Command::find_subcommand()` and `Arg::get_long()` / `Arg::get_short()`.

Looking these up on demand avoids cloning raw values in every `apply_*` helper
and keeps each record to two `&'static str` pointers and an optional note.
Each `apply_*` helper records its own assignment:

```rust
fn apply_model(
    partial: &mut PartialAppConfig,
    model: Option<&str>,
    recorder: Option<&mut CliRecorder>,
) {
    let Some(id) = model else { return };
    partial.assistant.model.id = id.into();

    if let Some(rec) = recorder {
        rec.record("assistant.model.id", "model", None);
    }
}
```

Because `field` and `arg_id` are `&'static str` constants declared alongside the
assignment, the mapping stays next to the code that mutates the partial.

During normal execution, `recorder` is `None` and does not allocate.
When `--explain` is active, `run_inner()` passes `Some(&mut recorder)` and uses
the recorded entries to annotate the `cli_flags` layer.

The recorder augments the snapshot diff rather than replacing it.
If a helper mutates a field without calling `recorder.record()`, the snapshot
diff still reports the change under `cli_flags`, without the flag name
annotation.

##### Drift prevention

A test checks that every recorded field path exists on `AppConfig`:

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

Like the existing `test_ensure_no_missing_assignments` test, this catches field
path drift at test time.

#### Schema documentation

Focused mode prints the field's documentation from its `schematic::Schema`
description (populated from the `///` doc comments on `AppConfig` fields), using
the same schema traversal as `AppConfig::fields()`:

```rust
fn field_description(field_path: &str) -> Option<String> {
    use schematic::{SchemaBuilder, SchemaType, Schematic as _};

    let builder = SchemaBuilder::default();
    let mut stack = vec![(AppConfig::build_schema(builder), "")];

    // Walk the schema tree to find the field and return its description.
    // Same traversal pattern as AppConfig::fields().
}
```

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

- `None`: flag omitted, normal execution.
- `Some(None)`: bare `--explain`, broad mode.
- `Some(Some("assistant.model.id"))`: `--explain=<FIELD>`, focused mode for that
  field.

In `run_inner()`, after `load_partial_config()` finishes collecting snapshots,
the explain check runs:

```rust
if let Some(field) = &cli.globals.explain {
    let report = build_explain_report(&snapshots, field.as_deref());
    printer.println(format_explain_report(&report, format));
    return Ok(());
}
```

This check runs after config loading and before `Ctx::new()`, workspace stream
initialization, and `Commands::run()`, so the command never executes.

### Field validation

In focused mode, JP validates the field path against `AppConfig::fields()`.
Unknown fields return an error with fuzzy-matched suggestions from
`missing_key()` in `assignment.rs`:

```
Error: Unknown config field 'assistant.model'

Did you mean one of:
  assistant.model.id
  assistant.model.parameters.max_tokens
  assistant.model.parameters.reasoning
  assistant.model.parameters.temperature
```

## Drawbacks

- **Snapshot cloning cost**: Cloning `PartialAppConfig` across ~9 layers adds
  sub-millisecond overhead, and only when `--explain` is passed.

- **Opt-in CLI recording**: Each `apply_*` helper must call
  `recorder.record(...)`.
  If a new flag omits the call, the snapshot diff still shows the changed field,
  but without the flag annotation.
  Tests verify that recorded `arg_id` values and field paths are valid, but
  catching a missing `record()` call requires code review.

- **Skipped-layer formatting**: When `inherit = false` is set, the formatter
  must report which earlier layers were skipped and why, adding conditional
  branches to output rendering.

## Alternatives

### Eager provenance tracking

Thread a `ProvenanceMap<String, (Value, LayerName)>` through the merge pipeline
so every `load_partial()` and `assign()` call records where each field was set.

Rejected because it complicates every config load even when `--explain` is not
used and requires changes across the `PartialAppConfig` merge infrastructure.
Diffing snapshots produces the same provenance without touching the merge
implementation.

### `jp config explain` subcommand

Provide a dedicated `jp config explain` subcommand instead of a global flag.

Rejected because a separate subcommand has no target command context and cannot
show command-specific CLI flags.
`jp config explain assistant.model.id` cannot know that `jp query --model=opus`
resolves `opus` to `anthropic/claude-sonnet-4-5`.
Making `--explain` a global flag captures the full resolution chain, including
command-specific flags.

### Verbose logging (`-vvv`)

`jp query -vvv` already emits trace logs containing the merged config JSON,
which shows the final state without per-layer provenance.

Trace logs are noisy, unstructured, and aimed at debugging internals rather than
answering which layer set a field.

## Non-Goals

- **Config editing**: `--explain` is read-only; it does not modify config files
  or suggest edits.
  Editing belongs to `jp config set` and a future interactive `--cfg` mode.

- **Runtime config changes**: `--explain` inspects static config resolution at
  startup, not changes applied mid-turn (such as config edited in the query
  editor).

- **Dependency tracking**: `--explain` reports where a value came from, not how
  downstream code interprets it (for example, that reasoning was disabled
  because the selected model does not support it).

## Risks and Open Questions

- **Dynamic tool config keys**: `conversation.tools.*` uses tool names as
  dynamic map keys.
  Snapshot diffing handles dynamic keys automatically, but focused-mode field
  validation must accept concrete tool paths like
  `conversation.tools.fs_read_file.run` that do not appear in the static
  `AppConfig::fields()` list.

- **Conversation delta readability**: Conversation deltas are stored as
  `PartialAppConfig` events and rendered in `--explain` as flat key-value pairs.
  Deeply nested values such as tool configs or system prompt sections can be
  large, so broad mode should truncate long values and point the user to
  `--explain=<field>` for the full value.

## Implementation Plan

### Phase 1: Snapshot infrastructure

1. Define `ConfigSnapshot` and `ExplainReport` in a new `jp_cli::explain`
   module.
2. Add snapshot collection points to `load_partial_config()`, including
   sub-layer snapshots for extended files, gated on a boolean flag so snapshots
   are collected only when `--explain` is active.
3. Update `load_config_file_with_extends()` to accept an optional snapshot
   collector and record a snapshot after each extended file loads.
4. Implement snapshot diffing to compute the fields changed by each layer and
   sub-layer.
5. Unit-test the diff logic across partial configs and `extends` hierarchies.

### Phase 2: Output formatting

1. Implement broad mode to group non-default fields by layer and print them to
   stdout.
2. Implement focused mode to trace a single field through every layer.
3. Add schema documentation lookup for focused mode.
4. Add JSON output formatting.
5. Validate focused-mode field paths and suggest close matches for unknown
   fields.

### Phase 3: CLI integration

1. Add `--explain` to `Globals`.
2. Run the explain check in `run_inner()` after config loading.
3. Add `recorder: Option<&mut CliRecorder>` to
   `IntoPartialAppConfig::apply_cli_config` and thread it through the `apply_*`
   helpers in `Query` and other command structs.
4. Add the `cli_recorder_field_paths_are_valid` test.
5. Test representative scenarios: alias resolution, `inherit = false` cutoff,
   conversation deltas, environment variable overrides, and file-relative
   `extends` alongside multi-root `--cfg` resolution (where a workspace file
   extending `../skill/web.toml` with a matching file in the user-global root
   resolves one file in the `extends` trace and both files in the `--cfg
   skill/web` trace).

Phases 1 and 2 can be developed and tested independently.
Phase 3 integrates them into the CLI and can be merged as a single PR.

## References

- [RFD 035]: Multi-root config load path resolution, explaining why `--cfg
  <name>` can resolve to several files while `extends` cannot
- [RFD 059]: Shell completions and man pages
- Future RFD: Interactive config (bare `--cfg` flag)
- `load_partial_config()` in `jp_cli/src/lib.rs`: the 9-layer merge pipeline
- `load_config_file_with_extends()` in `jp_config/src/util.rs`: `extends`
  resolution
- `AppConfig::fields()` in `jp_config/src/lib.rs`: schema-driven field
  enumeration
- `PartialAppConfig::delta()` in `jp_config/src/delta.rs`: field-level diffing
- `terraform plan`: precedent for dry-run config explanation
- `git config --show-origin --list`: precedent for config provenance display

[RFD 035]: 035-multi-root-config-load-path-resolution.md
[RFD 059]: 059-shell-completions-and-man-pages.md
[RFD 061]: 061-interactive-config.md
[RFD 063]: 063-usage-based-wizard-field-ordering.md
