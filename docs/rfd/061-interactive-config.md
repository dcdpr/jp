# RFD 061: Interactive Config

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21
- **Requires**: [RFD 060]
- **Required by**: [RFD 063]
- **Summary**: Opens an interactive configuration browser when `--cfg` is passed
  without a value to search and edit settings inline.

## Summary

Passing `--cfg` without a value opens an interactive configuration browser where
users search, inspect, and edit config fields using type-appropriate inline
prompts or `$EDITOR`.
The wizard produces `KvAssignment` values that feed into the existing `--cfg`
pipeline, then runs the command with the merged config.

## Motivation

JP has ~83 configuration fields spread across 7 top-level sections.
Overriding a setting for a single command requires up to four steps:

1. Remember the field name (`assistant.model.parameters.reasoning`).
2. Remember the valid values (`off`, `auto`, or a custom object).
3. Remember the `--cfg` syntax (`--cfg
   assistant.model.parameters.reasoning=auto`).
4. Remember the equivalent CLI shorthand, if one exists (`--reasoning auto`).

Steps 1 and 2 require consulting documentation or `--help`, step 3 requires
knowing the key-value syntax, and step 4 only exists for a subset of fields.
A user who wants to change the model without remembering the exact field path
has to stop, look up the documentation, construct the flag, and retype the
command.

The interactive config browser eliminates steps 1-3 by letting the user search
for a field, read its documentation and valid values, edit it inline, and
submit.
It provides an interactive UI for constructing `--cfg` flags.

## Design

### Trigger: bare `--cfg`

Passing `--cfg` (or `-c`) without a value triggers interactive mode:

```sh
jp query --cfg                    # interactive mode
jp query --cfg --model=opus       # interactive mode, model already set
jp query --cfg assistant.name=JP  # NOT interactive - has a value
```

A bare `--cfg` can be combined with valued `--cfg` flags.
Valued flags apply first, so the browser opens with those values already marked
as configured.

#### Clap integration

The `--cfg` argument uses `ArgAction::Append` with `KeyValueOrPath` values.
To accept a bare `--cfg`, the argument sets `num_args = 0..=1` and
`default_missing_value = ""`, and `KeyValueOrPath` adds an `Interactive`
variant:

```rust
enum KeyValueOrPath {
    KeyValue(KvAssignment),
    Path(Utf8PathBuf),
    Interactive,
}

impl FromStr for KeyValueOrPath {
    fn from_str(s: &str) -> Result<Self> {
        if s.is_empty() {
            return Ok(Self::Interactive);
        }
        // ... existing parsing ...
    }
}
```

In `run_inner()`, if the collected `config` vec contains `Interactive`, the
wizard runs after all other config layers are applied.

### Core loop

The interactive browser is a standalone function that takes the current
`PartialAppConfig` (resolved from files, env, conversation, and explicit `--cfg`
flags) and returns a `Vec<KvAssignment>`:

```rust
fn interactive_config_browser(
    current: &PartialAppConfig,
    schema: &Schema,
) -> Result<Vec<KvAssignment>>;
```

`run_inner()` calls `interactive_config_browser()`, and a future `jp config
edit` command can reuse it.

The browser runs in a loop:

1. Display the field selector, a filterable list of all `AppConfig::fields()`.
2. When the user selects a field, open a type-appropriate inline prompt.
3. Record the edited value as a `KvAssignment` keyed by field path, replacing
   any earlier assignment for the same field so only the final value appears in
   the returned assignments and dry-run output.
4. Return to the field selector.
5. On `^D` or "Apply and run", return the recorded assignments.
6. On `^C` or "Discard and exit", return an empty vec so the command does not
   run.

### Field selector

The field selector lists all config field paths.
Typing filters the list, and arrow keys navigate it.

#### Ordering

Fields are ordered by relevance:

1. **Already configured** (non-default) fields appear first with a visual
   indicator (such as a `●` prefix or bold text).
   Fields modified in the current wizard session use a distinct marker from
   fields set by earlier config layers.
2. **All remaining** fields follow in the order returned by
   `AppConfig::fields()`.

Once a CLI usage tracking system exists, the selector can also promote fields
the user frequently sets via `--cfg` or dedicated flags (see [Future
Extensions](#future-extensions)).

#### Documentation preview

Below the field list, a fixed-height preview pane (4-6 terminal lines) shows the
highlighted field's description from `schematic::Schema` (populated from `///`
doc comments on `AppConfig` fields).
Keeping the pane height fixed prevents the field list from shifting as the user
navigates.
If `--explain` provenance infrastructure is available, the preview also shows
the current value and where it was set.

Descriptions that exceed the pane height are truncated with `… (Ctrl+O to view
full docs in $EDITOR)`.
Inline scrolling of the preview pane is left as a future improvement.

#### Special entries

Two action entries appear at the top of the list, visually distinct from config
fields:

- **Apply and run** (`^D`): submit all wizard-configured values and run the
  command.
- **Discard and exit** (`^C`): cancel the wizard, discard all changes, and exit.

### Inline editing

Selecting a field opens a prompt matched to its schema type, pre-populated with
the field's current resolved value (pre-selected in select prompts, pre-filled
in text inputs).
Two markers label the values shown:

- **`(current)`**: marks the pre-filled value that will be used if left
  unchanged.
- **`(default)`**: marks the schema default when the current value differs from
  it.
  When the current value equals the default, only `(current)` is shown.

| Schema Type                          | Prompt                             |
| ------------------------------------ | ---------------------------------- |
| `Boolean`                            | Confirm prompt (y/n)               |
| `Enum` (≤8 variants)                 | Select prompt with variant list    |
| `Union` (bool + enum, like `Enable`) | Select prompt with all options     |
| `String` (short, no multiline)       | Text input                         |
| `Integer`                            | Text input with integer validation |
| `Float`                              | Text input with float validation   |
| `String` (multiline, e.g.            | Opens `$EDITOR`                    |
| `system_prompt`)                     |                                    |
| `Array`                              | Text input with comma separation   |
| Unknown / complex                    | Opens `$EDITOR` with TOML template |

Walking the `AppConfig` schema tree with the same traversal as
`AppConfig::fields()` resolves the selected field's `SchemaType` (`Boolean`,
`Enum`, `String`, `Integer`, `Float`, `Union`, or `Array`).
For `SchemaType::Enum`, the variant list comes from its `LiteralValue` entries.

#### `$EDITOR` escape hatch

Pressing `Ctrl+O` on any field opens `$EDITOR` instead of the inline prompt,
using the same editor lifecycle as the query editor (`jp_editor::open`).
The editor opens a temporary TOML file containing:

- The field's documentation as comments
- The current value (uncommented)
- Alternative values (commented out, for enum types)

When the editor closes, JP parses the TOML file and extracts the value.

### Output: `Vec<KvAssignment>`

The wizard produces a `Vec<KvAssignment>`, the same type produced by `--cfg
KEY=VALUE` parsing, with one assignment per edited field:

- The output feeds directly into `load_cli_cfg_args()`.
- The assignments serialize to `--cfg` flags for the dry-run CLI output.
- A future `jp config edit` command can serialize them to TOML for file output.

### Confirmation step

After leaving the field selector, a confirmation prompt shows the configured
values, the equivalent CLI command, and the available actions:

```txt
The following options have been configured:

    assistant.tool_choice = "auto"
    style.reasoning.display = "full"

Equivalent command:
  jp query --model=opus --cfg assistant.tool_choice=auto --cfg style.reasoning.display=full

Action:
  [1] Run command with selected options
   2  Edit more options (go back)
   3  Discard and exit
```

Option 1 runs the command, option 2 returns to the field selector, and option 3
discards all wizard changes and exits.
The "Equivalent command" line teaches users the CLI syntax so they can pass the
flags directly on future runs.

#### Equivalent command normalization

The equivalent command normalizes wizard assignments to the shortest CLI form:

1. **Alias reverse lookup**: When a wizard-set value matches a model alias in
   `providers.llm.aliases`, the preview uses the alias (for example,
   `assistant.model.id = "anthropic/claude-opus-4-6"` becomes `--model=opus`).
   If multiple aliases resolve to the same model, the first alias in insertion
   order wins.

2. **CLI flag reverse mapping**: When a config field path maps to a dedicated
   CLI flag, the preview emits that flag instead of `--cfg`.
   In [RFD 060], each command's `apply_cli_config()` records `CliRecord { field,
   flag, raw_value, note }` entries via `CliRecorder`.
   The wizard queries those mappings by field path and emits the corresponding
   long-form flag, such as `--model` for `assistant.model.id` and `--reasoning`
   for `assistant.model.parameters.reasoning`.
   Short-form flags (such as `-m`) are omitted for readability, and fields
   without a dedicated flag fall back to `--cfg KEY=VALUE`.

Normalization is best-effort: if a field has no recorded flag mapping (for
example, when a new flag has not been added to `CliRecorder`), the output falls
back to `--cfg KEY=VALUE`.

### Integration point

The wizard intercepts execution in `run_inner()`, after config loading but
before `Commands::run()`:

```rust
// In run_inner(), after load_partial_config():
if has_interactive_cfg(&cli.globals.config) {
    let schema = SchemaBuilder::build_root::<AppConfig>();
    let assignments = interactive_config_browser(&partial, &schema)?;

    if assignments.is_empty() {
        // User discarded — exit
        return Ok(());
    }

    // Apply wizard assignments
    for kv in &assignments {
        partial.assign(kv.clone())?;
    }

    // Rebuild config
    let config = Arc::new(build(partial)?);

    // Show combined confirmation prompt (values + equivalent command + action)
    // ...
}
```

### Prompt widget

The field selector requires a custom `ConfigBrowser` prompt widget in
`jp_inquire` (alongside the existing `InlineSelect` prompt) with:

- Filterable flat list (~83 items)
- Per-item documentation preview below the list
- Visual markers for configured items
- `Enter` to select for inline editing
- `Ctrl+O` to open `$EDITOR` for the selected item
- `^D` to submit, `^C` to cancel
- `?` for help

`ConfigBrowser` will implement `inquire`'s traits if `inquire` supports per-item
dynamic help text and custom key bindings; otherwise, we will contribute those
features upstream first.
Writing a standalone terminal widget is a last resort so `inquire` remains our
sole prompt widget.

## Drawbacks

- **Prompt widget complexity**: The field selector with documentation preview,
  visual markers, and dual-mode editing (inline and editor) is the most complex
  interactive UI component in JP.
  It requires careful terminal handling and may need upstream contributions to
  `inquire`.

- **Schema type coverage**: Not every `AppConfig` field maps cleanly to a simple
  inline prompt.
  Fields like `conversation.tools.*` have dynamic keys, and
  `assistant.instructions` is a nested array.
  The `$EDITOR` TOML template fallback handles these fields, though with less
  polish than inline prompts.

- **TTY requirement**: The interactive browser requires a terminal and cannot
  run in piped or non-interactive contexts, where users should pass `--cfg
  KEY=VALUE` instead.

## Alternatives

### Dedicated `--wizard` flag

Instead of overloading bare `--cfg`, JP could expose a separate `--wizard` flag.
We rejected this because the wizard is interactive `--cfg`; reusing the same
flag communicates that relationship without adding another flag to the
namespace.

### TUI application

A full-screen TUI (like `lazygit`) for browsing and editing config requires much
more engineering effort, pulls users out of their shell, and does not compose
with existing commands (you cannot append `--tui` to a partially typed command).

### Web-based config editor

A `jp config serve` command that opens a browser-based editor is out of scope
and does not fit JP's terminal-native workflow.

## Non-Goals

- **Command-specific guidance**: The wizard shows all config fields rather than
  a curated subset per command.
  Command-specific guided workflows belong to a future `--guide` feature.

- **Config file writing**: The wizard produces runtime `--cfg` overrides and
  does not write to `config.toml` or any other file.
  A future `jp config edit` command can reuse the core loop to persist changes.

- **Value auto-detection**: The wizard does not detect available models, running
  services, or other dynamic context; environment detection belongs to `jp init`
  ([RFD 044]).

## Risks and Open Questions

- **`inquire` upstream contributions**: The field selector needs per-item
  dynamic help text and possibly custom key bindings in `inquire`'s `Select`
  prompt.
  If the `inquire` maintainers do not accept those contributions, we must
  maintain a custom widget in `jp_inquire`.

- **Schema type edge cases**: Some fields use custom `Schematic` implementations
  (for example, `Enable` is a `Union` of `Boolean` and `Enum`, and `ToolSource`
  is a custom string).
  Prompt routing must map each custom representation to a prompt type.
  The `$EDITOR` fallback keeps every field editable, but unusual types may lack
  tailored inline prompts in the first iteration.

- **Dynamic tool config fields**: `conversation.tools.*` uses dynamic tool-name
  keys that are absent from the static `AppConfig::fields()` list.
  The wizard may need to read configured tool names from the loaded config to
  list them.

## Implementation Plan

### Phase 1: Core loop and clap integration

1. Add `Interactive` variant to `KeyValueOrPath`.
2. Handle bare `--cfg` in clap with `num_args = 0..=1` and
   `default_missing_value`.
3. Implement `interactive_config_browser()` backed by `AppConfig::fields()` and
   a basic `inquire::Select` prompt (without documentation preview or visual
   markers).
4. Implement type-appropriate inline prompts using schema introspection.
5. Wire into `run_inner()` with confirmation step and dry-run output.

### Phase 2: `$EDITOR` integration

1. Implement `Ctrl+O` to open `$EDITOR` for the selected field.
2. Generate TOML template with documentation and alternatives from schema.
3. Parse editor output back into `KvAssignment`.

### Phase 3: Polished field selector

1. Implement documentation preview below the field list (may require `inquire`
   upstream contribution or custom widget).
2. Add visual markers for configured items.
3. Order fields with configured items before remaining items.

### Phase 4: Equivalent command output

1. Implement alias reverse lookup (scan `providers.llm.aliases` for matches,
   first match wins).
2. Implement CLI flag reverse mapping using `CliRecord`/`CliRecorder` from [RFD
   060].
3. Serialize wizard assignments to the simplest CLI form.
4. Integrate into the combined confirmation prompt.

Phases 1-2 deliver a functional wizard, and phases 3-4 polish the UI and command
preview.
Each phase can be merged independently.

## Future Extensions

### Usage-based field ordering

The field selector orders configured fields ahead of remaining fields in schema
order.
A follow-up RFD could introduce a CLI usage tracking system that inserts a
recently or frequently used tier between configured and remaining fields.

## References

- [RFD 059]: Shell completions and man pages
- [RFD 060]: Config explain (`--explain`)
- [RFD 044]: Workspace initialization (model detection, config generation)
- `AppConfig::fields()` in `jp_config/src/lib.rs`: schema-driven field
  enumeration
- `SchemaBuilder::build_root::<AppConfig>()`: schema introspection with type
  info, descriptions, defaults, and enum variants
- `KvAssignment` in `jp_config/src/assignment.rs`: the assignment type the
  wizard produces
- `KeyValueOrPath` in `jp_cli/src/lib.rs`: the `--cfg` argument parser
- `InlineSelect` in `jp_inquire`: existing custom prompt widget
- `editor::open()` in `jp_cli/src/editor.rs`: editor lifecycle management
- `CliRecord` / `CliRecorder` in [RFD 060]: CLI flag reverse mapping
  infrastructure

[RFD 044]: 044-workspace-initialization.md
[RFD 059]: 059-shell-completions-and-man-pages.md
[RFD 060]: 060-config-explain.md
[RFD 063]: 063-usage-based-wizard-field-ordering.md
