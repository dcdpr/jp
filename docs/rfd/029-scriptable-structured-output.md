# RFD 029: Scriptable Structured Output

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05
- **Summary**: Adds a concise `--schema` DSL and clean stdout JSON defaults so
  `jp query` works directly in shell pipelines.

## Summary

This RFD makes `jp query` scriptable for structured LLM output by adding a
concise `--schema` DSL and emitting only the structured JSON result when
`--schema` is set or stdout is piped.
With these defaults, `jp query --schema 'summary' "summarize this" | jq .` works
without extra flags to suppress interactive chrome or persistence.

## Motivation

JP is built for interactive pair-programming sessions.
In scripts or shell pipelines, extracting structured output requires too many
flags:

```sh
# Current: still too many flags for clean scripted output
jp -! query --format=json --no-tools --new --schema 'summary' \
  --no-reasoning --attachment docs/rfd/005.md --model haiku \
  "summarize this document in two sentences"
```

Two problems make this awkward:

1. **Too much output.** JSON format mode emits all conversation events as
   NDJSON, not just the structured result.
   `JsonEmitter` in `TurnCoordinator` emits every event unconditionally, and
   `query.rs` then prints the structured data separately via `print_json`.

2. **Too many flags.** Most of the flags above disable JP's interactive defaults
   (tool calls, reasoning display, and conversation persistence), which suit
   interactive sessions but get in the way in scripts.

## Prior Art

### simonw/llm

`llm` is the closest precedent.
Its default behavior is script-friendly: `llm "prompt"` writes only the response
text to stdout.

- `llm --schema 'name, age int, bio' "invent a dog"` emits only the JSON object.
- A concise schema DSL (`name, age int, bio: a short bio`) expands to JSON
  Schema, and `--schema-multi` wraps the schema in `{"items": [...]}` for
  arrays.
- `--no-log` disables persistence.
- Templates (`--save dog`, then `llm -t dog "prompt"`) save prompts and schemas
  for reuse, and schemas can be referenced by hash ID or template name.

### charmbracelet/mods (sunset, replaced by Crush)

`mods` is designed for shell pipelines (`cat file | mods "explain"`):

- `-q` enables quiet mode and emits only the response.
- `--no-cache` disables persistence.
- `-f json` instructs the LLM to format its response as JSON in the prompt
  rather than enforcing a schema.

### sigoden/aichat

- Single-shot CMD mode is the default; chat/REPL mode is opt-in.
- Roles define reusable system prompts.
- Structured output and schemas are not supported natively.

### Comparison

`llm` and `mods` default to script-friendly output: the response goes to stdout,
and everything else goes to stderr or is suppressed.
JP defaults to interactive output.
The goal is to support scripted pipelines without degrading the interactive
experience.

## Design

### Goals

The same command should work in a pipeline and at an interactive terminal
without mode-switching flags:

```sh
# Piped — clean JSON automatically
jp q -s 'summary' "summarize this" -a doc.md -m haiku | jq .summary

# At a terminal — shows formatted response as today
jp q -s 'summary' "summarize this" -a doc.md -m haiku
```

Piping stdout triggers script-friendly behavior, and the concise schema DSL
keeps the invocation short.
Pass `-!` to skip persistence as well:

```sh
jp q -! -s 'summary' "summarize this" -a doc.md -m haiku | jq .
```

### Inference from `--schema`

When `--schema` is set, JP infers script-friendly defaults.
Explicit flags always take precedence: explicit flag > `--schema` inference >
config file > hardcoded default.

| Inference                           | Condition                              | Rationale                           |
| ----------------------------------- | -------------------------------------- | ----------------------------------- |
| Emit only structured JSON to stdout | `--format json` or stdout is not a TTY | NDJSON event stream is noise        |
| Suppress chrome on stderr           | stdout is not a TTY                    | Progress and tool headers clutter   |
|                                     |                                        | scripted runs                       |
| Hide reasoning display              | Always (unless `-r` is passed)         | Reasoning output is not part of the |
|                                     |                                        | structured result                   |

[RFD 048] routes chrome to stderr unconditionally, so piped stdout stays clean.
When `--schema` is combined with non-TTY stdout, JP also silences stderr chrome
so progress indicators and tool call headers do not clutter the terminal during
`$(jp q -s ...)` subshell calls.

`--schema` does not infer the following flags:

| Flag           | Why not infer                                |
| -------------- | -------------------------------------------- |
| `--no-persist` | The user may want a record of the extraction |
| `--new`        | Schema queries are valid within an ongoing   |
|                | conversation                                 |
| `--no-tools`   | Tool-assisted extraction is useful           |

### Concise Schema DSL

> [!TIP]
> **Status: Implemented.**
>
> See [RFD 030] for the full syntax reference and `crates/jp_cli/src/schema.rs`
> for the implementation.

The `--schema` / `-s` flag accepts a concise DSL inspired by `llm`:

```txt
summary                              -> single required string field
summary, key_points                  -> two required string fields
age int, name                        -> integer + string
summary: brief two-sentence summary  -> description as hint for the model
```

The DSL supports flat objects, nested objects, arrays, unions, optional fields,
and literal values.
Full JSON Schema is accepted as a passthrough when the DSL is insufficient.

### Remove Dead Flags

> [!TIP]
> **Status: Implemented.**
>
> The `--stream` / `-s` and `--no-stream` / `-S` flags have been removed.
> `-s` is now the short form of `--schema`.

### Future: `--one-shot` / `-1`

If the pattern `jp -! q -n -s 'schema' ...` remains common, a `-1` /
`--one-shot` flag could preset `--no-persist`, `--new`, `--no-reasoning`, and
(when `--schema` is set) `--format json`, with each setting still overridable by
explicit flags.

This flag is deferred.
`--schema` inference combined with output channel separation ([RFD 048]) covers
most scripting use cases.

### Future: Schema in Named Templates

When [RFD 013] lands, templates could bundle a schema alongside the prompt:

```toml
[templates.summarize]
title = "Summarize Document"
content = "Summarize this document in {{ sentences }} sentences."
schema = "summary: a concise summary of the document"
submit = "unattended"
```

```sh
jp q -% summarize -a docs/rfd/005.md
```

This adds a `schema` field to `NamedTemplate` in RFD-013.

## Drawbacks

- **Implicit behavior based on TTY detection.** Users who redirect stdout but
  still want verbose output may be surprised.
  This is the same trade-off `git`, `cargo`, and `ls` make.

## Alternatives

### Dedicated `jp gen` subcommand

A separate subcommand could bake in script-friendly defaults.
However, supporting optional tool calls requires the full turn loop
infrastructure, so `gen` would duplicate `query` with different defaults.
`--schema` inference (or a future `-1` flag) achieves the same result without a
new subcommand.

### `--quiet` flag to suppress non-data output

A `--quiet` flag works, but users must remember to pass it on every scripted
invocation.
Inferring quiet output from `--schema` and TTY detection is more ergonomic and
matches how other CLI tools behave.

## Non-Goals

- **Multi-turn scripting.** This RFD covers single-turn structured generation;
  continuing conversations across turns in scripts is out of scope.
- **Streaming structured output.** Structured JSON must be complete before it
  can be parsed, so responses cannot be streamed incrementally.
  The streaming infrastructure still runs, but terminal display is suppressed in
  script mode.

## Risks and Open Questions

### Remaining scope

This RFD originally opened with a phase implementing [RFD 048]'s output channel
separation, because clean piped stdout is a precondition for scripted output.
Because RFD 048 is now Implemented, only the inference layer remains, which is a
narrower change than the document's framing suggests.

## Implementation Plan

Output channel separation ([RFD 048]) and the schema DSL ([RFD 030]) are both
Implemented, so implementation consists of a single phase for schema output
inference:

- When `--schema` is present and stdout is not a TTY (or `--format json`), emit
  only the structured JSON object to stdout and suppress the `JsonEmitter`
  NDJSON event stream.
- When `--schema` is present and stdout is not a TTY, suppress chrome on stderr
  (progress indicators and tool call headers).
- When `--schema` is present, default reasoning display to `Hidden` unless the
  user explicitly passes `-r` / `--reasoning`.

## References

- [RFD 013: Named Query Templates][RFD 013]
- [RFD 048: Four-Channel Output Model][RFD 048]
- [simonw/llm schemas documentation]
- [charmbracelet/mods] (sunset)
- [sigoden/aichat]

[RFD 013]: 013-named-query-templates.md
[RFD 030]: 030-schema-dsl.md
[RFD 048]: 048-four-channel-output-model.md
[charmbracelet/mods]: https://github.com/charmbracelet/mods
[sigoden/aichat]: https://github.com/sigoden/aichat
[simonw/llm schemas documentation]: https://llm.datasette.io/en/stable/schemas.html
