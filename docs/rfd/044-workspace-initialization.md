# RFD 044: Workspace Initialization

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-14

## Summary

This RFD redesigns `jp init` to produce a working workspace with a
schema-driven, auto-generated `config.toml`. The generated config includes
documentation comments derived from `AppConfig`'s Rust doc comments, a curated
whitelist of the most useful fields, and user-selected values for the model ID
and tool run mode. The command supports both interactive and non-interactive
use.

## Motivation

`jp init` is the first thing a new user runs. It should guide them to a working
setup quickly — detect available providers, ask the minimum necessary questions,
and produce a `config.toml` that works out of the box.

Beyond the initial setup, the generated config should serve as a discovery tool.
JP has a large configuration surface — model parameters, tool run modes, style
options, editor integration, provider aliases — and most users will never read
the documentation page end to end. A well-structured config file with curated
options and inline documentation lets users browse what is available, uncomment
what looks interesting, and experiment.

Power-users run `jp init` regularly — every new project, every new repo. They
already know their preferred model and settings. For them, init should be a
one-liner (`jp init --model anthropic/claude-sonnet-4-5`) that finishes
instantly with no prompts. The interactive wizard is for newcomers; the flags
are for everyone else.

The goal is a config that is useful on day one and still useful on day thirty:
minimal above the fold, rich below it. And an init command that is fast enough
to never be annoying.

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
   running tools (`ask`, the safe default) or run them unattended. This is a
   security-sensitive choice - unattended mode lets the assistant modify files,
   run commands, and make network requests without human approval. A `?` option
   prints a detailed explanation.

2. **Model selection.** Which LLM to use. The wizard auto-detects available
   providers by checking environment variables (`ANTHROPIC_API_KEY`,
   `OPENAI_API_KEY`, `GOOGLE_API_KEY`) and running `ollama list`. The user picks
   from the detected models or enters one manually.

Two questions, one file, working workspace. The `config.toml` is generated with
the selected model and run mode as the only uncommented active values.
Everything else — style, editor, providers — uses safe defaults and is visible
as commented-out options in the generated file.

#### Non-interactive mode

```sh
$ jp init --model anthropic/claude-sonnet-4-5
Initialized workspace at current directory
```

When `--model` is provided, the command skips all prompts and generates the
config directly. Tool run mode defaults to `ask` when not specified.

```sh
jp init --model anthropic/claude-sonnet-4-5 --tools-run unattended
```

Both flags can be combined. This is the CI/scripting path.

```sh
$ jp init /path/to/project --model anthropic/claude-sonnet-4-5
Initialized workspace at /path/to/project
```

An optional positional argument specifies the workspace root (defaults to `.`).

#### User defaults

Users who run `jp init` frequently can store their preferred init values in
`$XDG_CONFIG_HOME/jp/init-defaults.toml`. This file uses the standard
`AppConfig` TOML layout, so any config field can be set as a default:

```toml
# ~/.config/jp/init-defaults.toml

[assistant.model]
id = "anthropic/claude-sonnet-4-5"

[conversation.tools.'*']
run = "ask"

[style.reasoning]
display = "full"
```

Both the `--defaults` flag and the `JP_INIT_DEFAULTS` env var accept the same
values:

- `true`, `1`, or bare flag (`--defaults`) — read from the default path
  (`$XDG_CONFIG_HOME/jp/init-defaults.toml`)
- `false` or `0` — disable defaults (same as not passing the flag)
- a file path — read from that path
- an `https://` URL — fetch the file over HTTPS

The URL form lets teams publish a shared defaults file (e.g. in a GitHub repo)
without requiring users to clone it locally:

```sh
jp init --defaults=https://raw.githubusercontent.com/myorg/jp-config/main/defaults.toml

JP_INIT_DEFAULTS=https://raw.gh.com/myorg/jp-config/main/defaults.toml jp init
```

When defaults are active, init uses the file's values as the active
(uncommented) fields in the generated `config.toml`, skipping all prompts:

```sh
$ jp init --defaults
Initialized workspace at current directory
```

This is the fast path for power-users. Set the env var in your shell profile and
`jp init` becomes a zero-argument command that always does the right thing.

The defaults file is parsed as a `PartialAppConfig`. Fields present in the file
are written as active values; fields absent fall back to the commented-out
whitelist as usual. This means the defaults file controls not just model and run
mode, but any field — aliases, style preferences, editor config, provider
settings.

CLI flags override defaults file values. If `--defaults` is active but `--model`
is also passed, the flag wins. The `--cfg` flag (same syntax as `jp query
--cfg`) can also be used to set arbitrary config values:

```sh
$ jp init --defaults --cfg assistant.name="My Assistant"
$ jp init --model anthropic/claude-sonnet-4-5 --cfg style.reasoning.display=full
```

Resolution order: hard-coded defaults < defaults file < `--cfg` overrides < CLI
flags (`--model`, `--tools-run`) .

If `--defaults` is passed and the file does not exist or is missing the required
`assistant.model.id` field, init errors with a message pointing to the expected
file path.

#### Re-initialization

If `.jp/` already exists, `jp init` errors:

```sh
$ jp init
Error: Workspace already initialized. Use `jp config` to modify.
```

A `--force` flag overwrites the existing config.

### Config generation

The config file is auto-generated from `AppConfig`'s schema using schematic's
`SchemaGenerator` and `TomlTemplateRenderer` (or a custom renderer if the output
needs more control).

The generation is entirely schema-driven. Documentation comments come from `///`
doc comments on `AppConfig`'s Rust struct fields. Default values come from the
schema's defaults. No manual TOML strings, no handwritten comments.

#### Three tiers of fields

Fields are classified into three tiers using `TemplateOptions`:

1. **Active** — uncommented, with user-chosen values injected via
   `custom_values`. These are the fields the init wizard asked about:
   `assistant.model.id` and `conversation.tools.'*'.run`.

2. **Commented-out** — rendered as TOML comments with their default values and
   doc comments, via `comment_fields`. The user sees these exist and can
   uncomment them. This is the "below the fold" content — the most commonly
   tweaked options.

3. **Hidden** — not rendered at all. Internal plumbing and deeply nested fields
   that newcomers don't need to see.

#### Field whitelist

The generated config uses `only_fields` (whitelist) rather than `hide_fields`
(blacklist). This means new fields added to `AppConfig` are invisible in the
init output until explicitly curated. The whitelist is safer for a first-run
experience: the generated config never grows unexpectedly.

The exact whitelist needs tuning during implementation (it depends on what the
actual rendered output looks like), but the initial set is roughly:

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

This list is curated, not computed. It represents the options a user is most
likely to want to change in their first week of using JP.

#### Injecting user choices

The user's choices are injected into the schema via
`TemplateOptions::custom_values`, which overrides the default value for specific
fields at render time. The renderer writes the user's chosen values as the
uncommented defaults:

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

The exact format depends on what `TomlTemplateRenderer` produces. If the default
renderer's output is hard to read (bad table nesting, missing enum variant
documentation, etc.), a custom `SchemaRenderer` specific to JP replaces it. The
interface is the same — only the rendering logic changes.

### Workspace directory structure

`jp init` creates:

```sh
.jp/
  .id           # workspace ID (5-char base36 timestamp)
  config.toml   # generated config
```

Init also creates a default conversation so that `jp query` works immediately
without requiring `--new`. The conversation is created using the generated
config's defaults (model, run mode, etc.).

### Internal architecture

The init command has two layers:

**Shell** (I/O, prompts, filesystem):
- CLI argument parsing (`--model`, `--tools-run`, `--defaults`, `--cfg`,
  `--force`, path)
- Loading defaults file when `--defaults` or `JP_INIT_DEFAULTS`
- Model auto-detection (env var checks, `ollama list`)
- Interactive prompts via `inquire`
- Directory creation and file writing

**Core** (pure, testable):
- `InitPlan` struct holding resolved values (model ID, run mode, cfg overrides,
  workspace root)
- Config generation: `InitPlan` → `TemplateOptions` → `SchemaGenerator` → TOML
  string
- Validation (model ID parsing, path resolution)

The core is a pure function: given an `InitPlan`, produce a `String` of TOML. No
I/O. This makes it testable with snapshot tests — the generated config for a
given set of inputs is deterministic and can be asserted against a stored
snapshot.

## Drawbacks

**Whitelist maintenance.** The `only_fields` whitelist must be manually updated
when new user-facing fields are added to `AppConfig`. If a field is added but
not whitelisted, it is invisible in the init output. This is intentional
(curated experience), but it is a maintenance cost. A CI check that flags new
`AppConfig` fields not present in any whitelist would catch this.

**Schematic renderer limitations.** The `TomlTemplateRenderer` may not produce
output that matches the desired format. Deeply nested TOML tables, enum variant
documentation, and map types (like `providers.llm.aliases`) may render poorly.
If so, writing a custom renderer is additional work. The fork of schematic
mitigates this — missing features can be added to the renderer.

## Alternatives

### Handwritten TOML template

Build the `config.toml` as a format string with placeholders. Simple to
implement, but comments and field names drift out of sync with the Rust types.
Every config change requires updating two places. Rejected because the
schema-driven approach eliminates this class of drift.

### `PartialAppConfig` → `toml::to_string_pretty`

Serialize a `PartialAppConfig` containing only the user's choices. The TOML
output is type-safe and round-trips, but it has no comments, no documentation,
and the nested table structure is verbose and unfriendly. Rejected because the
generated file is meant to be read and edited by humans.

### Full schema dump (all fields, all commented out)

Render every field in `AppConfig` as a commented-out TOML entry. This is what
the unfinished code was heading toward. Shows everything, but is overwhelming —
the full schema is hundreds of lines. A newcomer opening this file does not know
where to start. Rejected in favor of the three-tier whitelist approach, which
curates the first-run experience.

## Non-Goals

- **Config migration.** If the config schema changes, `jp init` does not update
  existing `config.toml` files. That is a separate concern (a future `jp config
  migrate` or automatic migration on load).
- **Multi-format generation.** Generating config in JSON or YAML (in addition to
  TOML) is a future enhancement. The architecture supports it — schematic has
  `YamlTemplateRenderer` and the custom renderer approach generalizes — but v1
  is TOML only.
- **WASM init hooks.** Plugins extending the init flow (per [RFD 016]) is future
  work. The design accommodates it — the config generation step is modular and
  additional config fragments could be merged before writing — but v1 does not
  implement it.
- **Extended interactive wizard.** Asking about editor preferences, provider API
  URLs, persona selection, etc. The two questions (run mode and model) are the
  MVP. More prompts can be added incrementally without architectural changes.

## Risks and Open Questions

### Renderer output quality

The `TomlTemplateRenderer` has not been tested against `AppConfig`'s full
schema. The output may have formatting issues with deeply nested structs, enum
types, or map fields. A spike (Phase 1) validates this before committing to the
approach. If the output is inadequate, a custom renderer is written. The
fallback is well-scoped — schematic's `SchemaRenderer` trait is straightforward
to implement, and the fork allows upstream fixes.

### Enum variant documentation

Fields like `RunMode` (ask/unattended/edit/skip) and `ProviderId`
(anthropic/openai/google/...) should show their allowed values in the TOML
comments. Schematic has a feature for rendering enum variants as part of the
field description, which may be disabled in our fork. The spike should verify
this works and re-enable it if needed.

### Whitelist curation

The initial `only_fields` list is a guess. The right set of fields depends on
actual user feedback and what looks good in the rendered output. The list should
be treated as living — adjusted based on what users actually change after init.

### Global config interaction

If the user already has `~/.config/jp/config.toml` with a model configured, `jp
init` still asks for a model and writes it to the workspace config. The
workspace config overrides the global one, which is correct behavior, but the
init wizard does not inform the user that a global config exists. Whether to
detect and skip the model prompt in this case is deferred to a future
enhancement.

## Implementation Plan

### Phase 1: Spike — renderer output

Build a test that calls `SchemaGenerator::add::<AppConfig>()` with
`TomlTemplateRenderer` and various `TemplateOptions` configurations. Snapshot
the output. Evaluate:

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

If Phase 1 determined the built-in renderer is insufficient, implement a custom
`SchemaRenderer` here.

**Dependency:** Phase 1 findings.
**Mergeable:** Yes (library code + tests, no CLI changes).

### Phase 3: Init command

Wire the config generation into `jp init`:

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

Reuse the existing model detection and interactive prompt code (it works).

**Dependency:** Phase 2.
**Mergeable:** Yes.

### Phase 4: Documentation

- Update `docs/getting-started.md` with init instructions
- Update `docs/configuration.md` to reference the generated config
- Add `jp init --help` examples

**Dependency:** Phase 3.
**Mergeable:** Yes.

## References

- [RFD 016: Wasm Plugin Architecture][RFD 016] — future init hooks via WASM
  plugins.
- [Configuration documentation](../configuration.md) — config loading order
  and file locations.
- [schematic
  `TomlTemplateRenderer`](https://docs.rs/schematic/latest/schematic/schema/struct.TomlTemplateRenderer.html)
  — the template rendering API.
- [schematic
  `TemplateOptions`](https://docs.rs/schematic/latest/schematic/schema/struct.TemplateOptions.html)
  — controls for field visibility, comments, and custom values.

[RFD 016]: 016-wasm-plugin-architecture.md
