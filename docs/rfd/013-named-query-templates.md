# RFD 013: Named Query Templates

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-25

## Summary

This RFD introduces named query templates: reusable, config-defined templates
that combine a Jinja-style content string with interactive questions to collect
template variables. Users invoke a template by name (`jp q -% feature`) or pick
one from a fuzzy-searchable list (`jp q -%`). The rendered template becomes the
query sent to the assistant. Templates support configurable submit behavior and
interactive question prompts with type validation.

## Motivation

Today, `jp query --template` (`-%`) treats the query string itself as a
Jinja-style template and resolves variables from `template.values` in the
config. This works for simple interpolation, but has several limitations:

1. **No reuse.** The template content lives in the query argument or the
   editor. There is no way to save a template and invoke it by name.
2. **No interactivity.** All variables must be pre-configured in
   `template.values`. There is no way to prompt the user for missing values at
   runtime.
3. **No structure.** Variables are untyped, have no descriptions, no defaults,
   no constrained choices. The user must know what variables exist and what
   values are valid.
4. **No discoverability.** Without named templates, there is no way to list
   available templates or search for one.

The result is that users who want structured, repeatable queries (feature
requests, code reviews, bug reports, architecture analyses) resort to shell
scripts, aliases, or copy-pasting from notes. Named templates bring this
workflow into JP, where it can integrate with the config system and the editor.

## Design

### User Experience

#### Invoking a template by name

```sh
# Long form
jp query --template feature

# Short form (the `%` flag accepts an optional value)
jp q -% feature
```

This loads the template named `feature` from the configuration, prompts the
user for each question defined in the template, renders the content with the
collected answers, and sends the result as the query.

#### Picking a template interactively

```sh
jp q -%
```

When `-%` is passed without a value, JP presents a fuzzy-searchable list of all
loaded templates (showing each template's `title`). The user selects one, then
proceeds through the Q&A flow as above.

#### Submit behavior

After the Q&A, the rendered template is handled according to the template's
`submit` field:

- `ask` (default): Show the rendered template and prompt with an inline
  select menu: `[s]end / [e]dit / [c]ancel / ?`.
- `unattended`: Send the rendered template as the query immediately.
- `edit`: Open `$EDITOR` with the rendered template before sending.

The `--edit` (`-e`) and `--no-edit` (`-E`) CLI flags override the template's
`submit` setting — `--edit` forces `edit` mode, `--no-edit` forces
`unattended` mode.

### Configuration Schema

Templates live under a top-level `templates` key. This replaces the current
`template` key. The struct uses the same pattern as `conversation.tools`: a
`defaults` field (accessed via `templates.*`) for global settings, and a
flattened `IndexMap` for named templates.

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

Full schema for the defaults (`templates.*`):

| Field    | Type     | Req | Default | Description                         |
|----------|----------|:---:|---------|-------------------------------------|
| `values` | `object` |  n  | `{}`    | Global template variable values.    |
|          |          |     |         | Pre-fills answers for any template. |
| `submit` | `string` |  n  | `ask`   | Default submit mode: `ask`,         |
|          |          |     |         | `unattended`, or `edit`.            |

Full schema for a named template:

| Field         | Type     | Req | Default   | Description                              |
|---------------|----------|:---:|-----------|------------------------------------------|
| `title`       | `string` |  y  | —         | Human-readable name shown in the picker. |
| `description` | `string` |  n  | —         | Shown below the title in the picker.     |
| `submit`      | `string` |  n  | (default) | What happens after rendering: `ask`,     |
|               |          |     |           | `unattended`, or `edit`.                 |
| `content`     | `string` |  y  | —         | Minijinja template body.                 |
| `questions`   | `array`  |  n  | `[]`      | Questions that populate template         |
|               |          |     |           | variables.                               |

Full schema for a question:

| Field      | Type     | Req | Default  | Description                              |
|------------|----------|:---:|----------|------------------------------------------|
| `target`   | `string` |  y  | —        | Variable name in the template content.   |
| `question` | `string` |  y  | —        | Prompt shown to the user.                |
| `type`     | `string` |  n  | `string` | Expected type: `string`, `number`,       |
|            |          |     |          | `bool`, `text`.                          |
| `enum`     | `array`  |  n  | —        | Finite list of allowed values. Turns the |
|            |          |     |          | prompt into a selection menu.            |
| `default`  | `any`    |  n  | —        | Default value if the user provides none. |

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

The inline template mode (current `-% ` behavior) keeps its current behavior.
The query input comes from the usual sources: positional args, `--` args, or
stdin. For example:

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
   - If the variable already has a value in `templates.*.values`, skip.
   - If `enum` is set, show a selection prompt (using `inquire::Select`).
   - Otherwise, show a text prompt with the `question` string. If `default`
     is set, pre-fill it.
   - Validate the answer against `type`. On validation failure, re-prompt.
   - **Esc** skips the current question (leaves it blank, which will produce
     a template rendering error if the variable is used and minijinja is in
     strict mode).
2. Render the `content` template with the collected values.
3. Handle the result according to `submit` mode:
   - `ask`: Show rendered template and prompt `[s]end / [e]dit / [c]ancel`.
   - `unattended`: Send immediately.
   - `edit`: Open `$EDITOR`.

Values from `templates.*.values` in the config serve as pre-filled answers. This
lets users hardcode values they always use (e.g., `templates.*.values.project =
"jp"`) while still being prompted for the rest.

### Internal Design

#### Config Changes (`jp_config`)

The existing `TemplateConfig` is replaced with `TemplatesConfig`, using the same
`defaults` + flattened `IndexMap` pattern as `ToolsConfig`:

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

The top-level config key changes from `template` to `templates`. The
`AppConfig` field updates accordingly. This is a breaking change to the
existing `template.values` config path (now `templates.*.values` or just
`templates.values`).

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

The `build_conversation` method gains a branch:

1. `template == Some(Some("-"))` → inline template mode (query input is a
   minijinja template, rendered with `templates.*.values`).
2. `template == Some(Some(name))` → load named template, run Q&A, render.
3. `template == Some(None)` → show picker, then as above.
4. `template == None` → no template processing.

The Q&A loop and template rendering can be extracted into a function in
`jp_cli` (or a new `jp_template` crate if the logic grows).

## Drawbacks

- **Config namespace.** Named templates share the `templates` namespace with
  default fields (`values`, `submit`). Template names must not collide with
  these reserved names. The list of reserved names is small and stable, and
  schematic will produce a clear error on collision.
- **Complexity budget.** The Q&A flow, validation, and submit modes add
  surface area to the query command, which is already the largest module in
  the codebase.
- **Minijinja learning curve.** Users need to learn minijinja syntax to write
  templates. This is mitigated by the simple variable interpolation being
  trivial (`{{ var }}`).
- **Breaking change.** The rename from `template` to `templates` breaks
  existing `template.values` config paths. Migration is straightforward
  (rename the key) but must be documented.

## Alternatives

### Keep `template` (singular) as the top-level key

Avoids the breaking rename but creates a namespace where `values` coexists
with named templates in a less principled way. The `templates` rename aligns
with the pattern established by `conversation.tools` and makes the
`defaults`/`flatten` split explicit.

### Nested namespace: `[templates.definitions.feature]`

Avoids namespace collision entirely but reads poorly in TOML and adds a
redundant nesting level. Rejected in favor of the flat namespace with
reserved-name checking, following the `ToolsConfig` precedent.

### External template files

Store templates as separate `.md` or `.toml` files in a `templates/` directory.
More flexible for large templates, but adds file discovery complexity and
diverges from the config-centric approach. Could be added later as a
complement (e.g. `content_file = "templates/feature.md"`).

### Prompt-driven template creation (no config)

Instead of config-defined templates, let users create templates interactively
with `jp template create`. More discoverable, but harder to version control
and share. The config-based approach is preferred because templates are part of
the project's configuration and travel with the repository.

## Non-Goals

- **Template sharing/registry.** No mechanism for publishing or installing
  templates from external sources.
- **Template versioning.** Templates are config values; they version with the
  config file in Git.
- **Complex control flow in Q&A.** No conditional questions, branching, or
  loops in the question sequence. Each question is independent.
- **Custom template functions.** Noted as a future extension point but not
  designed here.
- **Non-query templates.** This is specifically for `jp query`. Other commands
  may benefit from templates in the future, but that's out of scope.
- **Template-level config overrides.** A template does not carry its own model,
  tools, or other config settings. Instead, templates can live in separate
  config files loaded via `jp -c my_template -%tmpl`, which lets the config
  file set any options alongside the template definition. This uses the
  existing config layering system rather than adding a template-specific
  override mechanism.

## Risks and Open Questions

### Should template names be validated at config load time?

Yes. If a template references variables in `content` that have no
corresponding question and no value in `templates.*.values`, this should produce a
warning at config load time (not an error, since a user may rely on
`templates.*.values` to provide the variable at runtime).

### How does `--template` interact with `--schema`?

They are independent. `--schema` constrains the assistant's *response* format.
`--template` constructs the *query* content. Both can be used together: a
template generates the query, and a schema constrains the response.

### How does `--template` interact with `--replay`?

`--replay` re-sends the last message. `--template` constructs a new message.
They conflict. The CLI should enforce this with `conflicts_with`.

## Implementation Plan

### Phase 1: Config schema and named template loading

Replace `TemplateConfig` with `TemplatesConfig` using the `defaults` +
flattened `IndexMap` pattern. Add `NamedTemplate`, `TemplateQuestion`,
`TemplateTarget`, `SubmitMode` types. Add serialization tests. Add config
validation (reserved name check, variable coverage warning). Update
`AppConfig` to use the new type. This touches `jp_config` only and can be
merged independently.

### Phase 2: CLI flag change and template picker

Change `--template` from `bool` to `Option<Option<String>>`. Implement the
fuzzy-searchable template picker using `inquire::Select` (showing `title`
and `description`). Wire up the named template loading path in
`build_conversation`, including the `-%-` inline mode. At this point,
selecting a template loads it but does not yet run the Q&A — it renders
with whatever values are available in `templates.*.values`.

Depends on Phase 1. Can be merged independently.

### Phase 3: Interactive Q&A

Implement the question loop: text prompts, selection prompts for `enum` fields,
default values, type validation, Esc (skip one), Ctrl+D (skip all). Integrate
with the `inquire` crate (already a dependency via `jp_inquire`). Wire the
collected answers into the minijinja rendering context alongside
`templates.*.values`.

Depends on Phase 2. Can be merged independently.

### Phase 4: Submit behavior

Implement the `submit` field: `ask` (inline select menu), `unattended`
(send immediately), `edit` (open `$EDITOR`). Honor `--edit` / `--no-edit`
overrides. The `ask` mode reuses the `InlineSelect` component from
`jp_inquire`. The `edit` mode reuses `edit_message`.

Depends on Phase 3. Can be merged independently.

## Future Work

- **Assistant-populated variables.** A `target` field (`user` | `assistant`)
  on templates and/or individual questions could allow delegating unanswered
  questions to the assistant via structured output. Unanswered questions would
  be converted to a JSON schema, sent as a structured output request, and the
  response would fill in the remaining template variables. This depends on the
  structured output infrastructure and needs real use cases to justify the
  added complexity.
- **Custom template functions.** Expose Rust functions in the minijinja
  environment (e.g. `get_config()`, `model_id()`, `git_branch()`, `env()`).
- **`answer` mode.** A field controlling the Q&A process itself (`ask`,
  `unattended`, `edit`) — e.g. skipping prompts when all defaults are present.

## References

- [Issue #178: Add named templates with interactive prompts for `jp query`](https://github.com/dcdpr/jp/issues/178)
- `jp_config::template` (`crates/jp_config/src/template.rs`) — current template config
- `jp_config::conversation::tool` (`crates/jp_config/src/conversation/tool.rs`) — `ToolsConfig` pattern (defaults + flatten)
- `jp_cli::cmd::query` (`crates/jp_cli/src/cmd/query.rs`) — current template rendering (line ~510)
- [GitHub issue template syntax](https://docs.github.com/en/communities/using-templates-to-encourage-useful-issues-and-pull-requests/syntax-for-githubs-form-schema) — prior art for template question schemas
- [minijinja documentation](https://docs.rs/minijinja)
