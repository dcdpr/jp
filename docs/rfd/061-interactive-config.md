# RFD 061: Interactive Config

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-21

## Summary

A bare `--cfg` flag (no value) opens an interactive configuration browser that
lets users search, inspect, and edit config fields using type-appropriate inline
prompts or `$EDITOR`. The wizard produces `KvAssignment` values that feed into
the existing `--cfg` pipeline, then runs the command with the merged config.

## Motivation

JP has ~83 configuration fields spread across 7 top-level sections. Users who
want to change a setting mid-command face a friction ladder:

1. Remember the field name (`assistant.model.parameters.reasoning`)
2. Remember the valid values (`off`, `auto`, or a custom object)
3. Remember the `--cfg` syntax (`--cfg assistant.model.parameters.reasoning=auto`)
4. Remember the equivalent CLI shorthand, if one exists (`--reasoning auto`)

Steps 1 and 2 require reading docs or `--help`. Step 3 requires knowing the
key-value syntax. Step 4 is only available for a subset of fields. For a user
who knows they want to "change the model" but doesn't remember the exact path,
the current workflow is: stop, read docs, construct the flag, re-type the
command.

The interactive config browser eliminates steps 1-3 by letting the user search
for a field, see its documentation and valid values, edit it inline, and submit.
It's conceptually equivalent to typing `--cfg` flags — it just provides a UI for
building them.

## Design

### Trigger: bare `--cfg`

The interactive mode is triggered by passing `--cfg` (or `-c`) with no value:

```sh
jp query --cfg                    # interactive mode
jp query --cfg --model=opus       # interactive mode, model already set
jp query --cfg assistant.name=JP  # NOT interactive - has a value
```

A bare `--cfg` can be mixed with valued `--cfg` flags. The valued ones are
applied first (as they are today), and the interactive browser opens afterward
with those values already shown as configured.

#### Clap integration

The current `--cfg` definition uses `ArgAction::Append` with `KeyValueOrPath`
values. To support bare `--cfg`, the arg gains `num_args = 0..=1` and
`default_missing_value = ""`. The `KeyValueOrPath` enum gains an `Interactive`
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

In `run_inner()`, if any element in the collected `config` vec is `Interactive`,
the wizard runs after all other config layers are applied.

### Core loop

The interactive browser is a standalone function that takes the current
`PartialAppConfig` (already resolved from files, env, conversation, and explicit
`--cfg` flags) and returns a `Vec<KvAssignment>`:

```rust
fn interactive_config_browser(
    current: &PartialAppConfig,
    schema: &Schema,
) -> Result<Vec<KvAssignment>>;
```

This function is the reusable core. `run_inner()` calls it; a future `jp config
edit` command can call it too.

The loop:

1. Show the field selector (filterable list of all `AppConfig::fields()`)
2. User selects a field → type-appropriate inline prompt
3. User edits the value → assignment is recorded (deduplicating by field path;
   re-editing a field replaces the previous assignment)
4. Return to step 1
5. User presses `^D` or selects "Apply and run" → return assignments
6. User selects "Discard and exit" → return empty vec (command doesn't run)

Deduplication keeps the output clean: if the user edits the same field twice,
only the final value appears in the dry-run output and in the returned
assignments.

### Field selector

A filterable, searchable list of all config field paths. The user types to
filter; arrow keys navigate.

#### Ordering

Fields are ordered by relevance:

1. **Already configured** (non-default) fields appear first, marked with a
   visual indicator (e.g., `●` prefix or bold text).
2. **All remaining** fields in their natural order (as returned by
   `AppConfig::fields()`).

Fields configured during the current wizard session are also marked, distinct
from fields configured by other layers.

A future extension could add usage-based ranking (promoting fields the user
frequently sets via `--cfg` or dedicated flags) once a CLI usage tracking
system exists. See [Future extensions](#future-extensions).

#### Documentation preview

Below the field list, a fixed-height documentation area (4-6 terminal lines)
shows the description of the currently highlighted field. The description comes
from the `schematic::Schema` (populated from `///` doc comments on `AppConfig`
fields). If `--explain` provenance infrastructure is available, the preview also
shows the current value and where it was set.

If the description exceeds the available height, it is truncated with `… (Ctrl+O
to view full docs in $EDITOR)`. The area height is fixed regardless of content
length — it does not resize dynamically. This prevents the field list from
jumping around as the user navigates, which would be disorienting.

A future improvement could add inline scrolling of the description area.

#### Special entries

Two entries appear at the top of the list, visually distinct from config fields:

- **Apply and run** — submit all wizard-configured values and run the command.
- **Discard and exit** — cancel the wizard, discard all changes, exit.

`^D` is equivalent to "Apply and run". `^C` is equivalent to "Discard and
exit".

### Inline editing

When the user selects a field, the prompt type depends on the field's schema
type. The prompt pre-fills with the field's current resolved value. Two markers
help the user understand what they're looking at:

- **`(current)`** — shown next to the pre-filled value when it differs from the
  schema default. This is the value that would be used if the user doesn't
  change it.
- **`(default)`** — shown next to the schema default value, for reference. If
  the current value *is* the default, only `(current)` is shown (no separate
  default line).

For select prompts (enums, booleans), the current value is the pre-selected
option. For text inputs, the current value is the pre-filled text.

| Schema Type                          | Prompt                             |
|--------------------------------------|------------------------------------|
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

The schema type is determined by walking the `AppConfig` schema tree to the
selected field, using the same traversal as `AppConfig::fields()`. The
`SchemaType` enum variants (`Boolean`, `Enum`, `String`, `Integer`, `Float`,
`Union`, `Array`) provide the type information needed to choose the prompt.

For enum fields, the variant list is extracted from `SchemaType::Enum` which
contains `LiteralValue` entries. The current value (if set) is shown as the
default selection.

#### $EDITOR escape hatch

For any field, the user can press `Ctrl+O` to open `$EDITOR` instead of using
the inline prompt. The editor opens a temporary TOML file containing:

- The field's documentation as comments
- The current value (uncommented)
- Alternative values (commented out, for enum types)

This uses the same editor lifecycle as the existing query editor
(`jp_editor::open`). When the editor closes, the file is parsed as TOML and the
value is extracted.

### Output: `Vec<KvAssignment>`

The wizard produces a `Vec<KvAssignment>` — the same type that `--cfg KEY=VALUE`
parsing produces. Each edited field becomes one assignment. This means:

- The wizard's output feeds directly into `load_cli_cfg_args()`.
- The assignments can be serialized to `--cfg` flag format for the dry-run CLI
  output.
- A future `jp config edit` can serialize them to TOML for file output.

### Confirmation step

After leaving the field selector, a single confirmation prompt shows the
configured values, the equivalent CLI command, and the available actions:

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

Option 1 runs the command. Option 2 returns to the field selector. Option 3
discards all wizard changes and exits.

The "Equivalent command" line teaches users the CLI syntax. After using the
wizard a few times, they learn the flags and stop needing it.

#### Equivalent command normalization

The equivalent command normalizes wizard assignments to the simplest CLI form:

1. **Alias reverse lookup**: If a wizard-set value matches a known model alias
   (from `providers.llm.aliases`), the alias is used. For example,
   `assistant.model.id = "anthropic/claude-opus-4-6"` becomes `--model=opus`. If
   multiple aliases resolve to the same model, the first alias in insertion
   order is used.

2. **CLI flag reverse mapping**: If a config field path corresponds to a
   dedicated CLI flag, the flag is used instead of `--cfg`. This uses the
   `CliRecord` infrastructure from [RFD 060]. Each command's
   `apply_cli_config()` already records `CliRecord { field, flag, raw_value,
   note }` entries via the `CliRecorder`. The wizard reverses this: given a
   field path, it looks up whether a `CliRecord` mapping exists for the current
   command and emits the corresponding flag.

   For example, `assistant.model.id` → `--model`, and
   `assistant.model.parameters.reasoning` → `--reasoning`.

   Fields without a dedicated flag fall back to `--cfg KEY=VALUE` syntax.

   Short-form flags (e.g. `-m`) are never shown, to improve flag readability.

This normalization is best-effort. If the reverse mapping doesn't exist for a
field (e.g., a new flag was added but the recorder wasn't updated), the output
falls back to the `--cfg` form, which is always correct.

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

The field selector requires a custom prompt widget built on `inquire`. The
existing `jp_inquire` crate already has `InlineSelect` as a custom prompt. The
new widget (`ConfigBrowser`) needs:

- Filterable flat list (~83 items)
- Per-item documentation preview below the list
- Visual markers for configured items
- `Enter` to select for inline editing
- `Ctrl+O` to open $EDITOR for the selected item
- `^D` to submit, `^C` to cancel
- `?` for help

If `inquire`'s current API surface supports adding per-item dynamic help text
and custom key bindings, the widget is built on `inquire`'s traits. If not, the
necessary features are contributed upstream to `inquire` first. Falling back to
a fully custom terminal widget is the last resort, and should ideally be avoided
to retain `inquire` as our sole prompt widget.

## Drawbacks

- **Prompt widget complexity**: The field selector with documentation preview,
  visual markers, and dual-mode editing (inline + editor) is the most complex
  interactive UI component in JP. It requires careful terminal handling and may
  need upstream contributions to `inquire`.

- **Schema type coverage**: Not every `AppConfig` field maps cleanly to a simple
  prompt type. Fields like `conversation.tools.*` have dynamic keys.
  `assistant.instructions` is a complex nested array. The "open in $EDITOR with
  TOML template" fallback handles these, but the experience is less polished
  than the inline prompts.

- **TTY requirement**: The interactive browser requires a terminal. It doesn't
  work in piped or non-interactive contexts. This is acceptable — the whole
  point is an interactive experience. Non-interactive users have `--cfg
  KEY=VALUE`.

## Alternatives

### Dedicated `--wizard` flag

A separate `--wizard` flag instead of overloading bare `--cfg`.

Rejected because: the wizard *is* interactive `--cfg`. Using the same flag
communicates this relationship. A separate flag adds to the flag namespace
without adding semantic clarity.

### TUI application

A full-screen TUI (like `lazygit`) for config browsing and editing.

Rejected because: it's a much larger engineering effort, it's a different UX
paradigm (users expect to stay in their shell), and it doesn't compose with
existing commands (you can't append `--tui` to a partially-typed command).

### Web-based config editor

`jp config serve` opens a browser-based editor.

Out of scope. Doesn't fit JP's terminal-native identity.

## Non-Goals

- **Command-specific guidance**: The wizard shows all config fields, not a
  curated subset per command. Command-specific guided experiences are the domain
  of a future `--guide` feature.

- **Config file writing**: The wizard produces runtime `--cfg` overrides. It
  does not write to `config.toml` or any other file. A future `jp config edit`
  command would reuse the core loop for that purpose.

- **Value auto-detection**: The wizard does not detect available models, running
  services, or other dynamic context. That's `jp init`'s domain ([RFD 044]).

## Risks and Open Questions

- **`inquire` upstream contributions**: The field selector needs per-item
  dynamic help text and possibly custom key binding support in `inquire`'s
  `Select` prompt. If the `inquire` maintainers don't accept these
  contributions, we need a custom widget. The `jp_inquire` crate already
  demonstrates this is feasible but adds maintenance burden.

- **Schema type edge cases**: Some fields use custom `Schematic` implementations
  (e.g., `Enable` is a `Union` of `Boolean` and `Enum`, `ToolSource` is a custom
  string). The prompt routing logic needs to handle these gracefully. The
  $EDITOR fallback ensures no field is un-editable, but the inline experience
  may be rough for unusual types in the first iteration.

- **Dynamic tool config fields**: `conversation.tools.*` uses dynamic keys (tool
  names) that aren't in the static `AppConfig::fields()` list. The wizard may
  need to discover available tool names from the loaded config to present them.

## Implementation Plan

### Phase 1: Core loop and clap integration

1. Add `Interactive` variant to `KeyValueOrPath`.
2. Handle bare `--cfg` in clap with `num_args = 0..=1` and
   `default_missing_value`.
3. Implement `interactive_config_browser()` with a basic `inquire::Select`
   prompt (no documentation preview, no visual markers). Uses
   `AppConfig::fields()` for the field list.
4. Implement type-appropriate inline prompts using schema introspection.
5. Wire into `run_inner()` with confirmation step and dry-run output.

### Phase 2: $EDITOR integration

1. Implement `Ctrl+O` to open `$EDITOR` for the selected field.
2. Generate TOML template with documentation and alternatives from schema.
3. Parse editor output back into `KvAssignment`.

### Phase 3: Polished field selector

1. Implement documentation preview below the field list (may require `inquire`
   upstream contribution or custom widget).
2. Add visual markers for configured items.
3. Add field ordering logic (configured → rest).

### Phase 4: Equivalent command output

1. Implement alias reverse lookup (scan `providers.llm.aliases` for matches,
   first match wins).
2. Implement CLI flag reverse mapping using `CliRecord`/`CliRecorder` from
   [RFD 060].
3. Serialize wizard assignments to the simplest CLI form.
4. Integrate into the combined confirmation prompt.

Phases 1-2 deliver a functional wizard. Phases 3-4 polish the experience. Each
phase can be merged independently.

## Future Extensions

### Usage-based field ordering

The field selector currently orders fields as: configured first, then natural
order. A follow-up RFD could introduce a CLI usage tracking system to add a
"recently/frequently used" tier between configured and remaining fields. This
would make the wizard increasingly personalized over time.

## References

- [RFD 059]: Shell completions and man pages
- [RFD 060]: Config explain (`--explain`)
- [RFD 044]: Workspace initialization (model detection, config generation)
- `AppConfig::fields()` in `jp_config/src/lib.rs` — schema-driven field
  enumeration
- `SchemaBuilder::build_root::<AppConfig>()` — schema introspection with type
  info, descriptions, defaults, and enum variants
- `KvAssignment` in `jp_config/src/assignment.rs` — the assignment type the
  wizard produces
- `KeyValueOrPath` in `jp_cli/src/lib.rs` — the `--cfg` argument parser
- `InlineSelect` in `jp_inquire` — existing custom prompt widget
- `editor::open()` in `jp_cli/src/editor.rs` — editor lifecycle management
- `CliRecord` / `CliRecorder` in [RFD 060] — CLI flag reverse mapping
  infrastructure

[RFD 044]: 044-workspace-initialization.md
[RFD 059]: 059-shell-completions-and-man-pages.md
[RFD 060]: 060-config-explain.md
