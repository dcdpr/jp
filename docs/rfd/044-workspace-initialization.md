# RFD 044: Workspace Initialization

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-14
- **Summary**: Redesigns `jp init` to generate a schema-documented `config.toml`
  with interactive or flag-driven model and tool-mode selection.

## Summary

This RFD redesigns `jp init` to produce a working workspace with a
schema-driven, auto-generated `config.toml`.
The generated config includes documentation comments derived from `AppConfig`'s
Rust doc comments, a curated whitelist of the most useful fields, and
user-selected values for the model ID and tool run mode.
The command supports both interactive and non-interactive use.

## Motivation

`jp init` is the first command a new user runs.
It needs to guide them to a working setup quickly: detect available providers,
ask only the necessary questions, and produce a `config.toml` that works out of
the box.

Beyond initial setup, the generated config doubles as documentation.
JP has a large configuration surface (model parameters, tool run modes, style
options, editor integration, and provider aliases), and most users do not read
the configuration reference end to end.
A config file with curated options and inline comments lets users browse
available settings, uncomment what they need, and experiment.

Experienced users run `jp init` in every new project and repository, and they
already know their preferred model and settings.
The interactive wizard guides newcomers, while CLI flags let experienced users
initialize a workspace in a single non-interactive command (`jp init --model
anthropic/claude-sonnet-4-5`) with no prompts.

The goal is a fast `jp init` command and a generated config that stays useful
past day one: minimal active settings at the top, with common options documented
and commented out below.

## Design

### User experience

#### Interactive mode (default)

```sh
$ $ jp init
? Confirm before running tools? [Y/n/?] Y
? Select the default model to use:
  > anthropic/claude-sonnet-4-6   (detected ANTHROPIC_API_KEY)
    openai/gpt-5.3                (detected OPENAI_API_KEY)
    ollama/llama3                 (detected via "ollama list")
    Other (enter manually)

Initialized workspace at current directory
```

The wizard asks two questions:

1. **Tool run mode.** Whether the assistant should ask for confirmation before
   running tools (`ask`, the safe default) or run them unattended.
   This is a security-sensitive choice: unattended mode lets the assistant
   modify files, run commands, and make network requests without human approval.
   Entering `?` prints a detailed explanation.

2. **Model selection.** Which LLM to use.
   The wizard auto-detects available providers by checking environment variables
   (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GOOGLE_API_KEY`) and running `ollama
   list`.
   The user picks from the detected models or enters one manually.

`jp init` writes `config.toml` with the selected model and run mode as the only
active (uncommented) values.
All other curated settings (style, editor, providers) keep their defaults and
appear as commented-out options in the generated file.

#### Non-interactive mode

Passing `--model` skips all prompts and generates the config directly, with tool
run mode defaulting to `ask`:

```sh
$ jp init --model anthropic/claude-sonnet-4-5
Initialized workspace at current directory
```

Combine `--model` with `--tools-run` for CI and scripting:

```sh
jp init --model anthropic/claude-sonnet-4-5 --tools-run unattended
```

An optional positional argument specifies the workspace root (defaults to `.`):

```sh
$ jp init /path/to/project --model anthropic/claude-sonnet-4-5
Initialized workspace at /path/to/project
```

#### User defaults

Users who run `jp init` frequently can store preferred initialization values in
`$XDG_CONFIG_HOME/jp/init-defaults.toml`.
The file uses the standard `AppConfig` TOML layout, so any config field can be
set as a default:

```toml
# ~/.config/jp/init-defaults.toml

[assistant.model]
id = "anthropic/claude-sonnet-4-5"

[conversation.tools.'*']
run = "ask"

[style.reasoning]
display = "full"
```

The `--defaults` flag and the `JP_INIT_DEFAULTS` environment variable accept the
same values:

- `true`, `1`, or bare `--defaults`: read from the default path
  (`$XDG_CONFIG_HOME/jp/init-defaults.toml`).
- `false` or `0`: disable defaults (same as omitting the flag).
- A file path: read from that path.
- An `https://` URL: fetch the file over HTTPS.

The URL form lets teams publish a shared defaults file (for example, in a GitHub
repository) without requiring users to clone it locally:

```sh
jp init --defaults=https://raw.githubusercontent.com/myorg/jp-config/main/defaults.toml

JP_INIT_DEFAULTS=https://raw.gh.com/myorg/jp-config/main/defaults.toml jp init
```

When defaults are active, `jp init` skips all prompts.
Setting `JP_INIT_DEFAULTS=1` in a shell profile makes `jp init` non-interactive
without extra arguments:

```sh
$ jp init --defaults
Initialized workspace at current directory
```

The defaults file is parsed as a `PartialAppConfig`.
Fields present in the file (including model, run mode, aliases, style
preferences, editor config, and provider settings) are written as active
(uncommented) values in the generated `config.toml`; absent fields fall back to
the commented-out whitelist.
If `--defaults` is passed and the file does not exist or omits the required
`assistant.model.id` field, `jp init` exits with an error pointing to the
expected file path.

CLI flags override values from the defaults file, so `--model` takes precedence
over `--defaults`.
The `--cfg` flag (using the same syntax as `jp query --cfg`) sets arbitrary
config values:

```sh
$ jp init --defaults --cfg assistant.name="My Assistant"
$ jp init --model anthropic/claude-sonnet-4-5 --cfg style.reasoning.display=full
```

Resolution order from lowest to highest precedence: hard-coded defaults <
defaults file < `--cfg` overrides < CLI flags (`--model`, `--tools-run`).

#### Re-initialization

If `.jp/` already exists, `jp init` exits with an error unless `--force` is
passed to overwrite the existing config:

```sh
$ jp init
Error: Workspace already initialized. Use `jp config` to modify.
```

### Config generation

The config file is auto-generated from `AppConfig`'s schema using schematic's
`SchemaGenerator` and `TomlTemplateRenderer` (or a custom renderer if the output
needs more control).
Generation is completely schema-driven: documentation comments come from `///`
doc comments on `AppConfig`'s Rust struct fields, and default values come from
the schema defaults, without manual TOML templates or handwritten comments.

#### Field tiers

`TemplateOptions` classifies fields into three tiers:

1. **Active**: uncommented, with user-chosen values injected via
   `custom_values`.
   By default, these are the fields the wizard prompts for: `assistant.model.id`
   and `conversation.tools.'*'.run`.

2. **Commented-out**: rendered as TOML comments with their default values and
   doc comments via `comment_fields`.
   This tier exposes the most commonly adjusted options below the active
   settings so users can discover and uncomment them.

3. **Hidden**: omitted from the generated file.
   This tier hides internal plumbing and deeply nested fields that newcomers do
   not need.

#### Field whitelist

The generated config uses `only_fields` (whitelist) rather than `hide_fields`
(blacklist).
New fields added to `AppConfig` stay out of the `jp init` output until
explicitly curated, preventing the generated config from growing unexpectedly.

The whitelist is curated around the options a user is most likely to change in
their first week with JP.
The exact set will be tuned against the rendered output during implementation,
starting from:

**Active (uncommented)**:

- `assistant.model.id`
- `conversation.tools.'*'.run`

**Commented-out (visible but inactive)**:

- `assistant.model.parameters.reasoning`
- `conversation.title.generate.auto`
- `style.code.copy_link`
- `style.reasoning.display`
- `editor.command`
- `providers.llm.aliases` (with an expand example)

#### Injecting user choices

`TemplateOptions::custom_values` overrides the default values of specific fields
at render time, so the renderer writes the user's choices as uncommented
settings:

```toml
[assistant.model]
# The model to use for the assistant.
id = "anthropic/claude-sonnet-4-5"

[conversation.tools.'*']
# How to run tools. Options: ask, unattended, edit, skip
run = "ask"
```

#### Example output

```toml
# JP workspace configuration.
# See: https://jp.computer/configuration

[assistant.model]
# The model to use for the assistant.
#
# Format: "<provider>/<model-name>"
# Providers: anthropic, openai, google, ollama, llamacpp, openrouter
id = "anthropic/claude-sonnet-4-5"

# Maximum number of tokens to generate.
# max_tokens = 8192

# Sampling temperature (0.0 to 2.0).
# temperature = 1.0

[conversation.tools.'*']
# How to run tools. Options: ask, unattended, edit, skip
run = "ask"

# [style.reasoning]
# How to display model reasoning. Options: full, summary, off
# display = "full"

# [style.code]
# How to display code block links. Options: full, osc8, off
# copy_link = "full"
```

The exact formatting depends on `TomlTemplateRenderer`.
If the default renderer produces hard-to-read output (such as poor table nesting
or missing enum variant documentation), a JP-specific `SchemaRenderer` will
replace it behind the same interface.

### Workspace directory structure

`jp init` creates:

```sh
.jp/
  .id           # workspace ID (5-char base36 timestamp)
  config.toml   # generated config
```

`jp init` also creates a default conversation initialized with the generated
config's defaults (model, run mode, and any overrides) so that `jp query` works
immediately without `--new`.

### Internal architecture

The `init` command is split into two layers:

**Shell** (I/O, prompts, filesystem):

- CLI argument parsing (`--model`, `--tools-run`, `--defaults`, `--cfg`,
  `--force`, path)
- Loading the defaults file when `--defaults` or `JP_INIT_DEFAULTS` is set
- Model auto-detection (environment variable checks and `ollama list`)
- Interactive prompts via `inquire`
- Directory creation and file writing

**Core** (pure, testable):

- `InitPlan` struct holding resolved values (model ID, run mode, `--cfg`
  overrides, workspace root)
- Config generation: `InitPlan` → `TemplateOptions` → `SchemaGenerator` →
  TOML string
- Validation (model ID parsing and path resolution)

The core is a pure function from `InitPlan` to a TOML `String` with no I/O.
Because the generated config for a given `InitPlan` is deterministic, it can be
tested directly with snapshot tests.

## Drawbacks

**Whitelist maintenance.** Adding a user-facing field to `AppConfig` requires
updating the `only_fields` whitelist manually, or the field stays omitted from
the `jp init` output.
A CI check that flags `AppConfig` fields not listed in any whitelist can catch
omissions.

**Schematic renderer limitations.** `TomlTemplateRenderer` may render deeply
nested TOML tables, enum variant documentation, or map types (such as
`providers.llm.aliases`) poorly.
If its output is inadequate, we will need to either extend the renderer in our
`schematic` fork or write a custom `SchemaRenderer`.

## Alternatives

### Handwritten TOML template

Build `config.toml` as a format string with placeholders.
This is simple to implement, but comments and field names can drift out of sync
with the Rust types, requiring every config change to be updated in two places.
Rejected because schema-driven generation prevents this drift.

### `PartialAppConfig` → `toml::to_string_pretty`

Serialize a `PartialAppConfig` containing only the user's choices.
The TOML output is type-safe and round-trips, but it omits documentation
comments and produces verbose nested tables.
Rejected because the generated file is meant to be read and edited by humans.

### Full schema dump

Render every field in `AppConfig` as a commented-out TOML entry, as the earlier
unfinished code did.
The full schema spans hundreds of lines and overwhelms newcomers who do not know
where to start.
Rejected in favor of the three-tier whitelist.

## Non-Goals

- **Config migration.** `jp init` does not update existing `config.toml` files
  when the config schema changes.
  Migration belongs in a separate `jp config migrate` command or automatic
  migration on load.
- **Multi-format generation.** Generating JSON or YAML in addition to TOML is
  left to future work.
  The architecture supports it (`schematic` provides `YamlTemplateRenderer`, and
  a custom renderer can target other formats), but v1 generates TOML only.
- **WASM init hooks.** Allowing plugins to extend the initialization flow (per
  [RFD 016]) is future work.
  Because config generation is modular, plugin-provided config fragments can be
  merged before writing in a later version.
- **Extended interactive wizard.** Prompting for editor preferences, provider
  API URLs, or persona selection is out of scope for MVP.
  The wizard asks only for tool run mode and model ID; more prompts can be added
  later without architectural changes.

## Risks and Open Questions

### Renderer output quality

`TomlTemplateRenderer` has not been tested against `AppConfig`'s full schema and
may format deeply nested structs, enum types, or map fields poorly.
Phase 1 validates the output before committing to the approach.
If the output is inadequate, implementing schematic's `SchemaRenderer` trait or
patching our `schematic` fork is a well-scoped fallback.

### Enum variant documentation

Fields like `RunMode` (`ask`, `unattended`, `edit`, `skip`) and `ProviderId`
(`anthropic`, `openai`, `google`, ...) should list their allowed values in the
TOML comments.
Schematic supports rendering enum variants in field descriptions, which may be
disabled in our fork.
The Phase 1 spike will verify this and re-enable it if needed.

### Whitelist curation

The initial `only_fields` list is an estimate.
We will adjust the whitelist based on the rendered output and which settings
users actually change after initialization.

### Global config interaction

If the user already has `~/.config/jp/config.toml` with a model configured, `jp
init` still prompts for a model and writes it to the workspace config.
Having the workspace config override the global config is expected, but the
wizard does not inform the user that a global config exists.
Detecting a global model and skipping the prompt is deferred to a future
enhancement.

## Implementation Plan

### Phase 1: Renderer output spike

Build a snapshot test that calls `SchemaGenerator::add::<AppConfig>()` with
`TomlTemplateRenderer` across `TemplateOptions` configurations and evaluates:

- Table nesting readability
- Enum variant rendering
- Comment formatting
- `comment_fields` behavior
- `only_fields` vs `hide_fields` behavior

Determine whether the built-in renderer is sufficient or a custom renderer is
needed.

**Dependency:** None.
**Mergeable:** Yes (test-only, no behavioral changes).

### Phase 2: Config generation core

Implement the pure config generation function:

- Define `InitPlan` struct (model ID, workspace root)
- Build `TemplateOptions` with the curated whitelist
- Inject user values via `custom_values`
- Generate TOML string
- Snapshot tests for the generated output

If Phase 1 shows the built-in renderer is insufficient, implement a custom
`SchemaRenderer` here.

**Dependency:** Phase 1 findings.
**Mergeable:** Yes (library code + tests, no CLI changes).

### Phase 3: Init command

Wire config generation into `jp init`:

- Fix the run-mode logic bug (swap `Ask`/`Unattended`)
- Add `--model` and `--tools-run` flags for non-interactive use
- Add `--defaults` flag and `JP_INIT_DEFAULTS` env var support
- Add `--cfg` flag (reuse `KeyValueOrPath` from `jp query`)
- Add `--force` flag for re-initialization
- Remove the commented-out `SchemaGenerator` code and the `schema.json` debug
  write
- Detect existing `.jp/` directory and error without `--force`
- Write `.jp/.id` and `.jp/config.toml`
- Create a default conversation so `jp query` works immediately
- Print success message

Reuse the existing model detection and interactive prompt code.

**Dependency:** Phase 2.
**Mergeable:** Yes.

### Phase 4: Documentation

- Update `docs/getting-started.md` with init instructions
- Update `docs/configuration.md` to reference the generated config
- Add `jp init --help` examples

**Dependency:** Phase 3.
**Mergeable:** Yes.

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016]: future init hooks via WASM
  plugins.
- [Configuration documentation]: config loading order and file locations.
- [schematic `TomlTemplateRenderer`]: the template rendering API.
- [schematic `TemplateOptions`]: controls for field visibility, comments, and
  custom values.

[Configuration documentation]: ../configuration.md
[RFD 016]: 016-wasm-plugin-architecture.md
[schematic `TemplateOptions`]: https://docs.rs/schematic/latest/schematic/schema/struct.TemplateOptions.html
[schematic `TomlTemplateRenderer`]: https://docs.rs/schematic/latest/schematic/schema/struct.TomlTemplateRenderer.html
