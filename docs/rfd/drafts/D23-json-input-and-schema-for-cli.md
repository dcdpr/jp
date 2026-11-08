# RFD D23: JSON Input and Schema for CLI

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-13
- **Requires**: [RFD D19]
- **Summary**: Adds `--args-schema` and `--args-json` flags derived from `clap`
  argument structs for programmatic CLI invocation.

## Summary

This RFD adds a machine-readable JSON interface to JP's CLI: `--args-schema`
prints a JSON Schema for the CLI surface, and `--args-json` runs any command
from a JSON payload.
The existing `clap` argument structs derive `serde::Deserialize` and
`schemars::JsonSchema` so the CLI, schema, and JSON input share a single source
of truth.

## Motivation

JP's CLI is designed for interactive use by humans, but two kinds of callers
need programmatic access:

1. **LLMs.** An LLM invoking JP (e.g. as a tool in an agent loop) needs a
   machine-readable schema of JP's interface and a structured way to call it,
   rather than constructing fragile shell command strings.

2. **Scripts and automation.** CI pipelines, editor integrations, and wrapper
   tools benefit from passing structured JSON instead of string-splicing shell
   arguments.

Without a structured interface, programmatic callers must parse help text to
discover flag names and value formats, then build `argv` arrays by hand, which
limits JP's usefulness as a building block.

## Design

### Guiding Principle

The CLI argument structs (`Cli`, `Globals`, `Query`, `Conversation`, etc.) are
the single source of truth.
They already define the CLI via `clap` derives; adding `serde::Deserialize` and
`schemars::JsonSchema` to those structs derives the JSON schema and
deserialization behavior from the same types rather than a parallel hierarchy.

Field types that already work with `clap`, `serde`, and `schemars` need no
changes beyond the struct-level derives.
Fields with custom `value_parser` functions also need no new bridging types:
`clap`'s `value_parser` translates CLI strings into the field's target type
(e.g. `schemars::Schema`, `DateTime<Utc>`, `String`), while `serde` deserializes
JSON directly into that target type, leaving `value_parser` functions invisible
to `serde`.
Where a type is `clap`-specific (manual `FromArgMatches` implementations or
`Option<Option<T>>` triple-state patterns), it is replaced or annotated to
support all three trait families.

### User-Facing Interface

Two global flags expose the JSON interface:

#### Schema Output (`--args-schema`)

```shell
jp --args-schema                    # full schema
jp --args-schema query              # schema for `jp query`
jp --args-schema conversation.edit  # schema for `jp conversation edit`
```

`--args-schema` prints the JSON Schema for the `Cli` struct (or a subcommand) to
stdout and exits.
The full schema is generated with `schemars::schema_for!(Cli)`.

The optional argument is a dot-delimited command path that prunes the command
tree to the path from the root to the target subcommand.
Globals, root options, and metadata are always included.
At each nesting level, pruning drops sibling variants while preserving the
structural nesting, so the output remains a valid `--args-json` schema for
invoking that subcommand.

For example, `jp --args-schema query` emits the top-level schema with only the
`query` variant under `command` (omitting `conversation`, `config`, etc.).
`jp --args-schema conversation.edit` keeps the `command.conversation.edit`
nesting while dropping `conversation`'s sibling commands (`query`, `config`,
...) and `edit`'s sibling subcommands (`ls`, `show`, `fork`, ...).
When the argument is omitted, all variants at every level are included.

Because `schemars` generates schemas using `$ref` and `definitions`, pruning
walks the generated schema AST, filters `oneOf` arrays at each command level,
and removes orphaned definitions.

The output includes JP metadata:

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "title": "jp",
  "$jp": {
    "schema_version": 1,
    "jp_version": "0.42.0"
  },
  "type": "object",
  "properties": {
    "...": "..."
  }
}
```

`schema_version` is a monotonic integer incremented only on breaking changes to
the JSON input contract.
`jp_version` is informational.

#### JSON Input (`--args-json`)

```shell
jp --args-json '{ ... }'
echo '{ ... }' | jp --args-json -
```

`--args-json` accepts a JSON object conforming to the schema, either inline or
from stdin (`-`).
It first deserializes the input into a thin envelope struct that extracts and
validates the optional `schema_version` metadata, then deserializes the inner
payload into `Cli` and passes it to the existing `run_inner` execution path.
Config loading, workspace resolution, and command dispatch run unchanged.

Both `--args-schema` and `--args-json` are detected by scanning raw `argv`
before `clap` parsing.
When stdout is not a TTY, errors are returned as structured JSON, matching
`--format json`.

`--args-json` does not imply non-interactive mode.
TTY detection still governs interactive behavior, so invoking `--args-json` from
an interactive terminal preserves editor prompts and confirmation dialogs.

**Stdin interaction.** Passing `--args-json -` consumes stdin to read the JSON
payload before the command runs, leaving stdin empty for commands that also read
piped input (such as `Query`).
Callers must embed all input in the JSON payload (e.g. the `query` field) rather
than piping it over stdin, and the CLI help text for `--args-json` documents
this requirement.

### Bridging Types

An audit of all ~30 command structs groups their field types into the following
categories.

#### Directly Supported Types

Primitives (`bool`, `String`, `Option<String>`, `u8`, `usize`, `Option<usize>`,
`Option<NonZeroUsize>`), `Option<Utf8PathBuf>`, and `schemars::Schema` already
implement or derive `Serialize`, `Deserialize`, and `JsonSchema`.
Containing structs only need the derive annotations.

#### Custom `value_parser` Fields

Four fields use custom `value_parser` functions to parse CLI strings into typed
values: `string_or_path` (`String`), `parse_schema` (`schemars::Schema`),
`parse_fork_turns` (`Option<usize>`), and `parse_duration` (`DateTime<Utc>`).

Because the target types already implement `Deserialize` and `JsonSchema`, these
fields need no bridging types.
For example, `Query::schema` has type `Option<schemars::Schema>`: on the CLI
path, `clap` applies `string_or_path.try_map(parse_schema)` to parse the DSL
string, while on the JSON path `serde` deserializes a JSON Schema object
directly.

#### Simple Enums

`CliFormat`, `LogFormat`, and the two `Sort` enums (in `conversation/ls` and
`conversation/grep`) derive `clap::ValueEnum` and only need `Serialize`,
`Deserialize`, and `JsonSchema` added to their derive lists.
`ReasoningConfig` already derives `Serialize` and `Deserialize` via
`schematic`'s `Config` macro and needs only `JsonSchema`.

#### `FromStr` Types

Seven types (`Editor`, `AttachmentUrlOrPath`, `WorkspaceIdOrPath`,
`ConversationTarget`, `KvAssignment`, `ExpirationDuration`, `KeyValueOrPath`)
implement `FromStr`.
Each needs a `Deserialize` implementation that delegates string input to
`FromStr` and a `JsonSchema` implementation that emits `{"type": "string"}` with
the appropriate description or enum values.
A shared helper trait or macro can generate these implementations.

#### Manual `clap` Types

Three types have hand-written `clap::Args` and `FromArgMatches` implementations:

- **`ToolDirectives`**: An ordered list of tool enable and disable operations,
  represented in JSON as an array of directive objects.

- **`FlagIds<S, M>` and `PositionalIds<S, M>`**: Lists of conversation targets.
  Because the distinction between positional arguments and flags is CLI-only,
  both collapse to `Vec<ConversationTarget>` in JSON.

All three types need manual `Deserialize` and `JsonSchema` implementations.
Because they serialize as JSON arrays rather than objects, their fields on
parent structs must be regular named fields and cannot use `#[serde(flatten)]`.

#### Triple-State Fields (`Option<Option<T>>`)

Nine fields across `Query`, `Edit`, `Fork`, and `Print` use `clap`'s `num_args =
0..=1` with `default_missing_value` to represent three states: absent,
flag-present-without-value, and flag-present-with-value.

In JSON, these three states map to: field absent (or `undefined`), `null` (or
`true`), and a concrete value.
Distinguishing an omitted field from `null` requires custom `serde` handling.
Because `clap`'s derive macro has built-in support for `Option<Option<T>>` tied
to `num_args`, Phase 1 will prototype whether to use a `TriState<T>` newtype
that replicates that `clap` behavior, keep `Option<Option<T>>` with per-field
`#[serde(default, deserialize_with)]` annotations, or combine both.

#### `Commands::External`

The `#[command(external_subcommand)]` variant holds `Vec<String>` to satisfy
`clap`'s `FromIterator<String>` requirement.
In JSON, a structured object with `name` and `args` fields is clearer than a raw
string vector, so the variant becomes `External(ExternalCommand)`, where
`ExternalCommand` implements `FromIterator<String>` for `clap` alongside
`Serialize`, `Deserialize`, and `JsonSchema` for JSON.

For `--args-schema`, installed plugins that implement [RFD D19]'s `Help`
protocol contribute their structured argument definitions as `oneOf` variants
under the `External` discriminator.
Plugins that do not implement `Help` fall back to an opaque `{"name": "string",
"args": ["string"]}` schema.
To emit accurate JSON Schema types for plugin arguments, [RFD D19]'s `PluginArg`
type gains an optional `value_type` field (`"string" | "integer" | "number" |
"boolean"`, defaulting to `"string"`).

### Schema Versioning

The schema version (`$jp.schema_version`) tracks breaking changes to the JSON
input contract, independent of JP's release version.
Renaming or removing fields, changing a field's type, or altering command
nesting increments `schema_version`; adding optional fields does not.

The `schema_version` field in the input JSON is optional.
When present, the envelope struct validates that it matches the expected version
and returns an error on mismatch before deserializing the inner payload into
`Cli`.

## Drawbacks

**Annotation overhead.** Every command struct gains three additional derives and
some fields need `#[serde(...)]` or `#[schemars(...)]` attributes.
In practice, most structs need only the derive line; the fields requiring
attributes are concentrated in `Query` and the conversation targeting types.

**Dual maintenance of manual implementations.** The three types with manual
`clap::Args` implementations (`ToolDirectives`, `FlagIds`, `PositionalIds`) also
require manual `Deserialize` and `JsonSchema` implementations kept in sync with
their `clap` behavior.
Round-trip tests between CLI arguments and JSON catch drift.

**`clap` constraint gaps.** `serde` deserialization does not enforce `clap`'s
`conflicts_with` and `requires` constraints, so invalid combinations (e.g. both
`edit` and `no_edit` set to `true`) are caught by the execution pipeline rather
than at parse time.
A post-deserialization `validate()` pass can be added incrementally if needed.

**Binary size.** Deriving `JsonSchema` adds generated code across all command
structs.
Because `schemars` is already a dependency (used by `Query::schema`), the size
increase should be small, but it should be measured.

## Alternatives

### Generate Schema from `clap` Introspection at Runtime

Walk `clap::Command` at runtime using its introspection API (`get_subcommands`,
`get_arguments`, etc.) to produce a JSON Schema.

`clap`'s introspection is lossy: value parsers are opaque, so the schema would
emit `"string"` for nearly every argument instead of exposing the precise type
information that `schemars::JsonSchema` provides to LLMs.

### Dedicated Parallel Schema Structs

Maintain a separate hierarchy of `schemars`-annotated structs that mirror the
CLI shape, with a conversion layer between them.

Maintaining two sources of truth requires updating both struct hierarchies on
every new command or flag.

### Parse JSON via Synthetic `argv`

Convert the `--args-json` object into a `Vec<String>` `argv` and feed it through
`clap`'s normal parser.

While simpler to implement, this conversion is lossy in the opposite direction:
structured JSON values (nested objects for `--cfg`, ordered arrays for
`ToolDirectives`) have no natural `argv` representation, and every JSON input
remains subject to `clap`'s string-parsing rules.
Deserializing directly into `Cli` preserves types end-to-end.

## Non-Goals

- **Full `clap` constraint enforcement in JSON mode.** Constraints such as
  `conflicts_with` and `requires` are not replicated in `Deserialize`; the
  execution pipeline catches conflicts at runtime.

- **Shell completions from the schema.** Generating shell completions is covered
  by [RFD 059].

- **Non-interactive mode.** `--args-json` does not imply non-interactive
  behavior; see [RFD 049] for the `--non-interactive` flag.

- **`--cfg` JSON objects.** Accepting JSON objects as `--cfg` values is an
  orthogonal config-pipeline extension tracked separately.

- **Long-running JSON API server.** A persistent JSON-over-HTTP or
  JSON-over-stdio server that executes JP commands without per-invocation
  process overhead is a separate design concern, though this RFD's schema is
  designed to be reusable as its request contract.

## Risks and Open Questions

1. **`schemars` compatibility with `schematic` types.** Several CLI fields use
   `jp_config` types derived via `schematic`'s `Config` macro, which provides
   `Serialize` and `Deserialize` but not always `JsonSchema`.
   Each type must be checked and may require `#[schemars(with = "...")]`
   annotations or wrapper types.

2. **Schema stability.** Breaking changes become costly once external consumers
   depend on the schema, even with `schema_version`.
   Starting with `Query` (the primary command for LLM callers) allows iterating
   on the schema design before covering the full CLI surface.

3. **`schemars` version.** The workspace uses `schemars` 1.0.0-alpha.17.
   API changes before 1.0 stable may require updating derives and custom
   implementations.

4. **Enum representation for `Commands`.** `serde`'s default externally tagged
   enum representation produces `{"variant_name": {fields}}`.
   The tag names and subcommand nesting (e.g. `{"conversation": {"ls":
   {fields}}}`) must be validated against `schemars` output and tested with LLM
   callers.

5. **Negative flags.** The CLI defines negated flags such as `--no-edit`,
   `--no-reasoning`, and `--no-tool-use`, which are more naturally expressed in
   JSON by setting the positive field to `false` (e.g. `"edit": false` instead
   of `"no_edit": true`).
   Deriving from the CLI structs preserves the CLI field names by default, so we
   must decide where `#[serde(rename)]` or custom deserialization should alias
   them for JSON ergonomics.

6. **Triple-state `clap` interaction.** `clap`'s derive macro special-cases the
   `Option<Option<T>>` type name, which a replacement type may not trigger
   automatically.
   Phase 1 will prototype the bridging strategy (newtype, `serde` attributes, or
   a hybrid).

## Implementation Plan

### Phase 1: Bridging Types

Create the types and annotations that command structs depend on:

- Triple-state handling for `Option<Option<T>>` fields (prototyping a
  `TriState<T>` newtype, per-field `#[serde(deserialize_with)]`, or a hybrid
  approach).
- `ExternalCommand` struct implementing `FromIterator<String>`, `Serialize`,
  `Deserialize`, and `JsonSchema`.
- `Deserialize` and `JsonSchema` implementations for `FromStr` types
  (`ConversationTarget`, `AttachmentUrlOrPath`, `WorkspaceIdOrPath`, `Editor`,
  `KvAssignment`, `ExpirationDuration`, `KeyValueOrPath`).
- `Deserialize` and `JsonSchema` implementations for manual `clap` types
  (`ToolDirectives`, `FlagIds`, `PositionalIds`).

This phase has no user-visible behavior change and can be reviewed and merged
independently.

### Phase 2: Derive Additions for `Query` and `Globals`

Add `Serialize`, `Deserialize`, and `JsonSchema` to `Cli`, `Globals`,
`RootOpts`, `Commands`, and `Query`, covering the primary use case of LLM
callers invoking `jp query`.

Validate that `schemars::schema_for!(Cli)` produces a usable schema, and write
round-trip tests verifying that a `Query` deserialized from JSON produces the
same `PartialAppConfig` as the equivalent CLI arguments.

### Phase 3: `--args-schema` and `--args-json`

Add pre-parse `argv` detection for both flags.
`--args-schema` generates and prints the schema with embedded `schema_version`
and `jp_version` metadata and optional command-path pruning of the schema AST.
`--args-json` deserializes the input via the envelope struct, validates
`schema_version` when present, and dispatches into `run_inner`.

Both flags ship together because the schema defines the contract that
`--args-json` consumes.

### Phase 4: Remaining Commands

Add derives incrementally to the remaining command structs: `Conversation` (and
all subcommands), `Config`, `Attachment`, `Plugin`, and `Init`.
Each subcommand tree can land in a separate pull request.

### Phase 5: External Plugin Schema (Depends on [RFD D19])

Extend `PluginArg` with `value_type`.
On `--args-schema`, query installed plugins via the `Help` protocol and inject
their schemas into the `External` variant.

## References

- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]
- [RFD 059: Shell Completions and Man Pages][RFD 059]
- [RFD 072: Command Plugin System][RFD 072]
- [RFD D19: Structured Plugin Help Protocol][RFD D19]

[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 059]: ../059-shell-completions-and-man-pages.md
[RFD 072]: ../072-command-plugin-system.md
[RFD D19]: D19-structured-plugin-help-protocol.md
