# RFD 013: Named Query Templates

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-25
- **Summary**: Adds reusable, config-defined query templates with interactive
  prompts that collect typed variables before rendering.

## Summary

This RFD introduces named query templates: reusable, config-defined Jinja
templates paired with interactive, type-validated prompts that collect template
variables.
Users invoke a template by name (`jp q -% feature`) or pick one from a
fuzzy-searchable list (`jp q -%`), then submit the rendered query directly or
open it in `$EDITOR` before sending.

## Motivation

`jp query --template` (`-%`) treats the query string itself as a Jinja-style
template and resolves variables from `template.values` in the config.
This works for simple interpolation, but has four limitations:

1. **No reuse.** The template content lives in the query argument or the editor,
   so users cannot save a template and invoke it by name.
2. **No interactivity.** Every variable must be pre-configured in
   `template.values`, with no runtime prompts for missing values.
3. **No structure.** Variables have no types, descriptions, defaults, or
   constrained choices, so users must already know which variables exist and
   what values are valid.
4. **No discoverability.** Because templates are inline strings, users cannot
   list or search available templates.

Users who want structured, repeatable queries (feature requests, code reviews,
bug reports, architecture analyses) end up maintaining shell scripts, aliases,
or scratch notes.
Named templates bring this workflow into JP and integrate it with the config
system and the editor.

## Design

### User Experience

#### Invoking a template by name

```sh
# Long form
jp query --template feature

# Short form
jp q -% feature
```

This loads the `feature` template from the configuration, prompts for each
question defined in the template, renders `content` with the collected answers,
and sends the result as the query.

#### Picking a template interactively

```sh
jp q -%
```

When `-%` is passed without a value, JP presents a fuzzy-searchable list of
loaded templates showing each template's `title`.
Selecting a template starts the same prompt and render flow.

#### Submit behavior

After rendering, the template's `submit` field controls how JP handles the
query:

- `ask` (default): Show the rendered template and prompt with an inline select
  menu: `[s]end / [e]dit / [c]ancel / ?`.
- `unattended`: Send the rendered template as the query immediately.
- `edit`: Open `$EDITOR` with the rendered template before sending.

The `--edit` (`-e`) and `--no-edit` (`-E`) CLI flags override `submit`: `--edit`
forces `edit` mode, and `--no-edit` forces `unattended` mode.

### Configuration Schema

Templates live under the top-level `templates` key, replacing `template`.
Like `conversation.tools`, the section pairs a `defaults` table (`templates.*`)
for global settings with a flattened `IndexMap` of named templates.

```toml
# Global defaults (accessed via templates.*)
[templates.'*']
values = { project = "jp" }
submit = "ask"

# Named template — "feature" is the template name
[templates.feature]
title = "Building A New Feature"
description = "Structured template for proposing a new feature."
submit = "edit"
content = """
I want to build a new feature for {{ project }}.

## Context

{{ context }}

## Requirements

{% for req in requirements %}  {# future: list-type questions #}
- {{ req }}
{% endfor %}
"""

[[templates.feature.questions]]
target = "project"
question = "What project is this for?"
type = "string"

[[templates.feature.questions]]
target = "context"
question = "Describe the context and motivation."
type = "string"
```

Defaults schema (`templates.*`):

| Field    | Type     | Req | Default | Description                         |
| -------- | -------- | :-: | ------- | ----------------------------------- |
| `values` | `object` |  n  | `{}`    | Global template variable values.    |
|          |          |     |         | Pre-fills answers for any template. |
| `submit` | `string` |  n  | `ask`   | Default submit mode: `ask`,         |
|          |          |     |         | `unattended`, or `edit`.            |

Named template schema (`templates.<name>`):

| Field         | Type     | Req | Default   | Description                              |
| ------------- | -------- | :-: | --------- | ---------------------------------------- |
| `title`       | `string` |  y  | -         | Human-readable name shown in the picker. |
| `description` | `string` |  n  | -         | Shown below the title in the picker.     |
| `submit`      | `string` |  n  | (default) | Submit mode after rendering: `ask`,      |
|               |          |     |           | `unattended`, or `edit`.                 |
| `content`     | `string` |  y  | -         | Minijinja template body.                 |
| `questions`   | `array`  |  n  | `[]`      | Questions that populate template         |
|               |          |     |           | variables.                               |

Question schema (`templates.<name>.questions[]`):

| Field      | Type     | Req | Default  | Description                              |
| ---------- | -------- | :-: | -------- | ---------------------------------------- |
| `target`   | `string` |  y  | -        | Variable name in the template content.   |
| `question` | `string` |  y  | -        | Prompt shown to the user.                |
| `type`     | `string` |  n  | `string` | Expected value type: `string`, `number`, |
|            |          |     |          | `bool`, or `text`.                       |
| `enum`     | `array`  |  n  | -        | Allowed values. Presents the prompt as a |
|            |          |     |          | selection menu.                          |
| `default`  | `any`    |  n  | -        | Default value if the user provides none. |

### CLI Changes

The existing `-% / --template` flag changes from a boolean to an optional
string:

```sh
# Before (current)
-% / --template              Boolean flag. Treats query as a template.

# After
-% / --template [NAME]       Optional value.
                             No value, no query:   show template picker.
                             With value:           load named template.
                             No value, with query: treat query input as
                                                   inline template.
```

Inline template mode (`-%` with a query) still reads the query from positional
arguments, arguments after `--`, or stdin:

```sh
# Inline template from positional arg
jp q -% -m sonnet "Hello {{ user }}"

# Inline template from stdin
echo "Hello {{ user }}" | jp q -%

# Inline template via -- separator
jp q -% -- "Hello {{ user }}"

# Named template (no query arg needed)
jp q -% feature

# Picker (no query arg needed)
jp q -%
```

### Interactive Q&A Flow

When a named template is loaded:

1. For each entry in `questions`, in order:
   - Skip the question if `templates.*.values` already defines `target`.
   - If `enum` is set, show a selection prompt with `inquire::Select`.
   - Otherwise, show a text prompt with `question`, pre-filled with `default` if
     set.
   - Validate the answer against `type` and re-prompt on failure.
   - Pressing **Esc** skips the question and leaves the variable unset, which
     fails rendering in strict mode if `content` references the variable.
2. Render the `content` template with the collected values.
3. Handle the rendered query according to `submit` mode:
   - `ask`: Show the rendered template and prompt `[s]end / [e]dit / [c]ancel`.
   - `unattended`: Send immediately.
   - `edit`: Open `$EDITOR`.

Entries in `templates.*.values` supply fixed answers without prompting, so users
can configure values they always use (such as `templates.*.values.project =
"jp"`) and only answer the remaining questions.

### Internal Design

#### Config Changes (`jp_config`)

`TemplatesConfig` replaces `TemplateConfig`, using the same `defaults` and
flattened `IndexMap` pattern as `ToolsConfig`:

```rust
/// Templates configuration.
#[derive(Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct TemplatesConfig {
    /// Global defaults, accessed via `templates.*`.
    #[setting(nested, rename = "*")]
    pub defaults: TemplateDefaults,

    /// Named templates (catch-all for unknown keys).
    #[setting(nested, flatten, merge = merge_nested_indexmap)]
    templates: IndexMap<String, NamedTemplate>,
}

/// Global template defaults.
pub struct TemplateDefaults {
    /// Template variable values used to render query templates.
    pub values: Map<String, Value>,

    /// Default submit behavior for all templates.
    #[setting(default = SubmitMode::Ask)]
    pub submit: SubmitMode,
}

pub struct NamedTemplate {
    pub title: String,
    pub description: Option<String>,
    pub submit: Option<SubmitMode>,
    pub content: String,
    pub questions: Vec<TemplateQuestion>,
}

pub struct TemplateQuestion {
    pub target: String,
    pub question: String,
    #[serde(default = "default_type")]
    pub r#type: QuestionType,
    #[serde(default)]
    pub r#enum: Option<Vec<String>>,
    pub default: Option<Value>,
}

pub enum QuestionType {
    String,
    Number,
    Bool,
}

pub enum SubmitMode {
    Ask,
    Unattended,
    Edit,
}
```

Renaming the top-level config key and `AppConfig` field from `template` to
`templates` is a breaking change: `template.values` moves to
`templates.*.values` (or `templates.values`).

#### Query Command Changes (`jp_cli`)

The `--template` flag changes from `bool` to `Option<Option<String>>`:

```rust
/// Use a named template, or treat the query as an inline template.
///
/// Without a value: show interactive template picker.
/// With "-": treat query input as an inline minijinja template.
/// With any other value: load the named template.
#[arg(short = '%', long)]
template: Option<Option<String>>,
```

`build_conversation` branches on `template`:

1. `Some(Some("-"))`: render the query input as an inline minijinja template
   using `templates.*.values`.
2. `Some(Some(name))`: load the named template, run the Q&A prompts, and render.
3. `Some(None)`: open the template picker, then run the named-template flow.
4. `None`: skip template processing.

The Q&A loop and template rendering live in a helper function in `jp_cli`, or in
a new `jp_template` crate if the module grows.

## Drawbacks

- **Config namespace.** Named templates share the `templates` namespace with
  default fields (`values`, `submit`), so template names cannot use those
  reserved keys.
  The reserved set is small and stable, and `schematic` reports a clear error on
  collision.
- **Complexity budget.** The Q&A flow, validation, and submit modes add surface
  area to the query command, which is already the largest module in the
  codebase.
- **Minijinja learning curve.** Users must learn minijinja syntax to author
  templates, though basic variable interpolation is just `{{ var }}`.
- **Breaking change.** Renaming `template` to `templates` breaks existing
  `template.values` config paths.
  Migrating only requires renaming the key, but the change must be documented.

## Alternatives

### Keep `template` (singular) as the top-level key

Keeping `template` avoids a breaking rename, but mixes `values` and named
templates in the same namespace without a clear convention.
Renaming the key to `templates` matches `conversation.tools` and makes the
`defaults` and `flatten` split explicit.

### Nested namespace: `[templates.definitions.feature]`

Nesting templates under `definitions` avoids key collisions with default fields,
but reads poorly in TOML and adds a redundant table level.
We prefer the flat namespace with reserved-name validation, following
`ToolsConfig`.

### External template files

Storing templates as separate `.md` or `.toml` files in a `templates/` directory
suits large templates, but adds file discovery complexity and separates
templates from the rest of the configuration.
External files can be added later as an optional field (such as `content_file =
"templates/feature.md"`).

### Prompt-driven template creation (no config)

Letting users create templates interactively with `jp template create` improves
discoverability, but makes templates harder to version control and share.
Defining templates in config files keeps them in the repository alongside the
rest of the project's configuration.

## Non-Goals

- **Template sharing/registry.** Publishing or installing templates from
  external sources.
- **Template versioning.** Templates are config values and version with their
  config file in Git.
- **Complex control flow in Q&A.** Questions run in a flat sequence without
  branching, conditions, or loops; each question is independent.
- **Custom template functions.** Exposing custom functions to templates is
  deferred to future work.
- **Non-query templates.** Templates apply only to `jp query`; other commands
  are out of scope.
- **Template-level config overrides.** A template does not carry its own model,
  tools, or other config settings.
  Instead, a template that needs specific options can live in a separate config
  file loaded via `jp -c my_template -%tmpl`, using existing config layering
  rather than a template-specific override mechanism.

## Risks and Open Questions

### Should templates be validated at config load time?

Yes.
If `content` references variables that have neither a matching entry in
`questions` nor a value in `templates.*.values`, JP logs a warning at config
load time.
This is a warning rather than an error because another config layer may provide
the variable in `templates.*.values` at runtime.

### How does `--template` interact with `--schema`?

They are independent and can be used together: `--template` constructs the
outgoing *query*, while `--schema` constrains the assistant's *response* format.

### How does `--template` interact with `--replay`?

They conflict: `--replay` re-sends the last message, while `--template`
constructs a new one.
The CLI enforces this with `conflicts_with`.

## Implementation Plan

### Phase 1: Config schema and named template loading

Replace `TemplateConfig` with `TemplatesConfig` using the `defaults` and
flattened `IndexMap` pattern, and update `AppConfig` to use the new type.
Add `NamedTemplate`, `TemplateQuestion`, `TemplateTarget`, and `SubmitMode`,
along with serialization tests and config validation (reserved-name check and
variable-coverage warning).
This phase touches only `jp_config` and can be merged independently.

### Phase 2: CLI flag change and template picker

Change `--template` from `bool` to `Option<Option<String>>`, implement the
fuzzy-searchable template picker with `inquire::Select` (showing `title` and
`description`), and wire named template loading and `-%-` inline mode into
`build_conversation`.
In this phase, selecting a template renders it directly with the values in
`templates.*.values` without running the Q&A prompts.

Depends on Phase 1 and can be merged independently.

### Phase 3: Interactive Q&A

Implement the question loop using `inquire` (already a dependency via
`jp_inquire`): text prompts, selection prompts for `enum` fields, default
values, type validation, Esc to skip one question, and Ctrl+D to skip all
remaining questions.
Pass the collected answers into the minijinja rendering context alongside
`templates.*.values`.

Depends on Phase 2 and can be merged independently.

### Phase 4: Submit behavior

Implement the `submit` modes (`ask`, `unattended`, and `edit`) and honor
`--edit` / `--no-edit` overrides.
`ask` reuses the `InlineSelect` component from `jp_inquire`, and `edit` reuses
`edit_message`.

Depends on Phase 3 and can be merged independently.

## Future Work

- **Assistant-populated variables.** A `target` field (`user` | `assistant`) on
  templates or individual questions could delegate unanswered questions to the
  assistant via structured output.
  JP would convert unanswered questions to a JSON schema, send a structured
  output request, and fill in the remaining template variables from the
  response.
  This depends on the structured output infrastructure and needs real use cases
  to justify the added complexity.

  > [!TIP]
  > [RFD 029] introduces the structured output infrastructure this feature
  > depends on.
  > [RFD 030] documents the schema DSL.

- **Custom template functions.** Expose Rust functions in the minijinja
  environment (such as `get_config()`, `model_id()`, `git_branch()`, and
  `env()`).

- **`answer` mode.** Add a field (`ask`, `unattended`, `edit`) controlling the
  Q&A process itself, such as skipping prompts when all defaults are present.

## References

- [Issue \#178: Add named templates with interactive prompts for `jp query`]
- `jp_config::template` (`crates/jp_config/src/template.rs`): current template
  config
- `jp_config::conversation::tool` (`crates/jp_config/src/conversation/tool.rs`):
  `ToolsConfig` pattern (defaults + flatten)
- `jp_cli::cmd::query` (`crates/jp_cli/src/cmd/query.rs`): current template
  rendering (line ~510)
- [GitHub issue template syntax]: prior art for template question schemas
- [minijinja documentation]

[GitHub issue template syntax]: https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema
[Issue \#178: Add named templates with interactive prompts for `jp query`]: https://github.com/dcdpr/jp/issues/178
[RFD 029]: 029-scriptable-structured-output.md
[RFD 030]: 030-schema-dsl.md
[minijinja documentation]: https://docs.rs/minijinja
