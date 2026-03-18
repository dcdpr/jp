# RFD 029: Scriptable Structured Output

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05

## Summary

Make JP useful as a scriptable tool for structured LLM output. This RFD captures
the goals, analyzes prior art, and defines an incremental plan to make `jp query
--schema 'summary' "summarize this" | jq .` work cleanly.

## Motivation

JP is built for interactive pair-programming sessions. When used in scripts or
pipelines, the experience degrades:

```sh
# Current: still too many flags for clean scripted output
jp -! query --format=json --no-tools --new --schema 'summary' \
  --no-reasoning --attachment docs/rfd/005.md --model haiku \
  "summarize this document in two sentences"
```

Two problems:

1. **Too much output.** JSON format mode emits all conversation events as
   NDJSON, not just the structured result. The `JsonEmitter` in
   `TurnCoordinator` unconditionally emits every event, and then `query.rs`
   separately prints the structured data via `print_json`.

2. **Too many flags.** Most flags exist to turn off JP's interactive defaults
   (tool calls, reasoning display, conversation persistence). These are the
   wrong defaults for scripting but the right defaults for interactive use.

## Prior Art

### simonw/llm

The closest precedent. Default behavior is script-friendly — `llm "prompt"`
emits only the response text to stdout.

- `llm --schema 'name, age int, bio' "invent a dog"` – just the JSON object
- Concise schema DSL: `name, age int, bio: a short bio` expands to JSON Schema
- `--schema-multi` wraps in `{"items": [...]}`  for arrays
- `--no-log` disables persistence
- Templates (`--save dog`, then `llm -t dog "prompt"`) for reuse
- Schemas referenceable by hash ID or template name

### charmbracelet/mods (sunset, replaced by Crush)

Pipe-oriented: `cat file | mods "explain"`.

- `-q` quiet mode — only the response
- `--no-cache` for no persistence
- `-f json` asks the LLM to format as JSON (prompt-level, not schema-level)

### sigoden/aichat

- Single-shot CMD mode is the default; chat/REPL is opt-in
- Roles (system prompts) for reuse
- No native structured output / schema support

### Key observation

`llm` and `mods` default to "script-friendly" — response to stdout, everything
else to stderr or suppressed. JP defaults to "interactive-friendly." The
challenge is bridging the gap without breaking the interactive experience.

## Design

### Goals

The target experience:

```sh
# Piped — clean JSON automatically
jp q -s 'summary' "summarize this" -a doc.md -m haiku | jq .summary

# At a terminal — shows formatted response as today
jp q -s 'summary' "summarize this" -a doc.md -m haiku
```

No special flags needed to switch between the two modes. The pipe triggers
script-friendly behavior. The concise schema DSL keeps it short.

If the user also doesn't want to persist:

```sh
jp q -! -s 'summary' "summarize this" -a doc.md -m haiku | jq .
```

### Inference from `--schema`

When `--schema` is present, JP can infer scripting-friendly defaults. These are
defaults, not hard overrides — explicit flags always win.

Precedence: explicit flag > `--schema` inference > config file > hardcoded
default.

| Inference                           | Condition                              | Rationale                              |
|-------------------------------------|----------------------------------------|----------------------------------------|
| Only emit structured JSON to stdout | `--format json` or stdout is not a TTY | NDJSON event stream is noise           |
| Suppress chrome on stderr           | stdout is not a TTY                    | Progress and tool headers are noise in |
|                                     |                                        | scripted contexts                      |
| Hide reasoning display              | Always (unless `-r` is passed)         | Structured output; reasoning display   |
|                                     |                                        | adds nothing                           |

[RFD 019] routes chrome to stderr unconditionally, so stdout is always clean for
piping. The "suppress chrome on stderr" inference goes further: when the user is
scripting (stdout is not a TTY), chrome on stderr is also noise — progress
indicators and tool call headers appearing on the terminal alongside a `$(jp q
-s ...)` subshell add no value. This inference silences stderr chrome entirely
in that case.

What `--schema` should NOT imply:

| Flag           | Why not infer                            |
|----------------|------------------------------------------|
| `--no-persist` | User might want a record of the          |
|                | extraction                               |
| `--new`        | Schema queries within an ongoing         |
|                | conversation are valid                   |
| `--no-tools`   | Tool-assisted extraction is useful       |

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
and literal values. Full JSON Schema is accepted as a passthrough when the DSL
is insufficient.

### Remove Dead Flags

> [!TIP]
> **Status: Implemented.**
>
> The `--stream` / `-s` and `--no-stream` / `-S` flags have been removed. `-s`
> is now the short form of `--schema`.

### Future: `--one-shot` / `-1`

If the pattern `jp -! q -n -s 'schema' ...` becomes common enough, a `-1` /
`--one-shot` flag could preset: `--no-persist`, `--new`, `--no-reasoning`, and
`--format json` when `--schema` is present. Each can still be overridden.

This is not planned for initial implementation. The inference from `--schema`
combined with output channel separation ([RFD 019]) covers the 90% case.

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

This would be a small addition to the RFD-013 schema (a `schema` field on
`NamedTemplate`).

## Drawbacks

- **Implicit behavior based on TTY detection.** Users who redirect stdout but
  still want verbose output may be surprised. This is the same trade-off `git`,
  `cargo`, and `ls` make.

## Alternatives

### Dedicated `jp gen` subcommand

A separate subcommand with scripting-friendly defaults baked in. Rejected
because supporting optional tool calls means `gen` needs the full turn loop
infrastructure, making it effectively `query` with different defaults. A flag
(`-1`) or inference from `--schema` achieves the same result without a new
subcommand.

### `--quiet` flag to suppress non-data output

Works but requires the user to remember to pass it every time. Inference from
`--schema` + TTY detection is more ergonomic and matches how other CLI tools
behave.

## Non-Goals

- **Multi-turn scripting.** This RFD focuses on single-turn structured
  generation. Multi-turn scripting (continuing conversations in scripts) is
  separate.
- **Streaming structured output.** Structured responses are inherently
  non-streamable (the JSON must be complete). The streaming infrastructure still
  runs, but display is suppressed in script mode.

## Risks and Open Questions

### Dependency on RFD 048

The output inference depends on [RFD 048]'s output channel separation (stdout
for assistant data, stderr for chrome). Once chrome is on stderr, stdout is
automatically clean for piping. The remaining inference — suppressing stderr
chrome in scripted contexts and suppressing NDJSON event noise — builds on that
foundation.

## Implementation Plan

### Phase 1: Output channel separation (RFD 048)

Implement [RFD 048]'s stdout/stderr split. Once chrome goes to stderr, piped
structured output is automatically clean on stdout. This is the architectural
foundation for the schema inference behavior.

Also adds the `PrintTarget::Tty` variant and `/dev/tty` integration for
interactive prompts, and moves tracing logs from stderr to a log file.

### Phase 2: Schema output inference

With output channels separated, add the inference logic:

- When `--schema` is present and stdout is not a TTY (or `--format json`): only
  emit the structured JSON object to stdout. Suppress `JsonEmitter` NDJSON event
  stream.
- When `--schema` is present and stdout is not a TTY: suppress chrome on stderr
  (progress indicators, tool call headers). In scripted contexts these add no
  value.
- When `--schema` is present: default reasoning display to `Hidden` unless the
  user explicitly passes `-r` / `--reasoning`.

Depends on Phase 1.

## References

- [RFD 013: Named Query Templates][RFD 013]
- [RFD 048: Four-Channel Output Model][RFD 048]
- [simonw/llm schemas documentation](https://llm.datasette.io/en/stable/schemas.html)
- [charmbracelet/mods](https://github.com/charmbracelet/mods) (sunset)
- [sigoden/aichat](https://github.com/sigoden/aichat)

[RFD 013]: 013-named-query-templates.md
[RFD 030]: 030-schema-dsl.md
[RFD 048]: 048-four-channel-output-model.md
