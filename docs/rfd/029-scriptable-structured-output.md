# RFD 029: Scriptable Structured Output

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-05

## Summary

Make JP useful as a scriptable tool for structured LLM output. Today, getting
clean JSON from JP requires too many flags and produces too much noise. This
RFD captures the goals, analyzes prior art, and defines an incremental plan
to make `jp query --schema 'summary' "summarize this" | jq .` work cleanly.

## Motivation

JP is built for interactive pair-programming sessions. When used in scripts or
pipelines, the experience degrades:

```sh
# Current: 8 flags to get structured JSON
jp -! q --format=json --no-tools --no-stream --new \
  --schema '{"type":"object","properties":{"summary":{"type":"string"}},"required":["summary"]}' \
  --no-reasoning --attachment docs/rfd/005.md --model haiku \
  "summarize this document in two sentences"
```

This produces three lines of output — the chat request event, the chat response
event, and the actual structured data — when only the last line is wanted.

Three problems:

1. **Too much output.** JSON format mode emits all conversation events as
   NDJSON, not just the structured result. The `JsonEmitter` in
   `TurnCoordinator` unconditionally emits every event, and then `query.rs`
   separately prints the structured data via `print_json`.

2. **Too many flags.** Most flags exist to turn off JP's interactive defaults
   (streaming chrome, tool calls, reasoning display, conversation persistence).
   These are the wrong defaults for scripting but the right defaults for
   interactive use.

3. **No concise schema syntax.** Writing full JSON Schema for a single string
   field is painful. Other tools solve this with a DSL.

## Prior Art

### simonw/llm

The closest precedent. Default behavior is script-friendly — `llm "prompt"`
emits only the response text to stdout.

- `llm --schema 'name, age int, bio' "invent a dog"` → just the JSON object
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
jp -! q -s 'summary' "summarize this" -a doc.md -m haiku | jq .
```

### Inference from `--schema`

When `--schema` is present, JP can infer scripting-friendly defaults. These are
defaults, not hard overrides — explicit flags always win.

Precedence: explicit flag > `--schema` inference > config file > hardcoded
default.

| Inference | Condition | Rationale |
|-----------|-----------|-----------|
| Only emit structured JSON to stdout | `--format json` or stdout is not a TTY | NDJSON event stream is noise |
| Suppress chrome on stdout | stdout is not a TTY | Piped output should be clean |
| Hide reasoning display | Always (unless `-r` is passed) | Structured output; reasoning display adds nothing |

What `--schema` should NOT imply:

| Flag | Why not infer |
|------|---------------|
| `--no-persist` | User might want a record of the extraction |
| `--new` | Schema queries within an ongoing conversation are valid |
| `--no-tools` | Tool-assisted extraction is useful |

### Concise Schema DSL

Adopt a syntax inspired by `llm`:

```
summary                              → single required string field
summary, key_points                  → two required string fields
age int, name                        → integer + string
summary: brief two-sentence summary  → description as hint for the model
```

Rules:

- Comma-separated fields (or newline-separated)
- Default type is `string`
- `int`, `float`, `bool` type suffixes
- Text after `:` is a description
- All fields are `required` by default

The DSL produces a `schemars::Schema`. It works alongside full JSON Schema
input — the `--schema` flag accepts either format.

### Remove Dead Flags

The `--stream` (`-s`) and `--no-stream` (`-S`) flags on `query` are parsed but
never read (destructured as `_`). Removing them frees `-s` for `--schema`.

### Future: `--one-shot` / `-1`

If the pattern `jp -! q -n -s 'schema' ...` becomes common enough, a
`-1` / `--one-shot` flag could preset: `--no-persist`, `--new`, `--no-tools`,
`--no-reasoning`, `--no-stream`, and `--format json` when `--schema` is
present. Each can still be overridden.

This is not planned for initial implementation. The inference from `--schema`
combined with output channel separation (RFD 019) covers the 90% case.

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

- **Schema DSL is a new syntax to learn.** Mitigated by its simplicity and by
  accepting full JSON Schema as a fallback.

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
  non-streamable (the JSON must be complete). The streaming infrastructure
  still runs, but display is suppressed in script mode.

## Risks and Open Questions

### Dependency on RFD 019

The output inference (suppress chrome when piping) is cleanly solved by
[RFD 019]'s output channel separation (stdout for data, stderr for chrome).
Without it, suppressing intermediate output requires threading ad-hoc flags
through the turn coordinator. The concise schema DSL and dead flag removal
are independent and can proceed now.

### Schema DSL scope

Starting with flat objects (no nesting, no arrays). A `--schema-multi` flag
for the `{"items": [...]}` pattern (like `llm`) can be added later. Nesting
support is deferred until real use cases justify the parser complexity.

## Implementation Plan

### Phase 1: Schema DSL and flag cleanup (independent)

- Implement the concise schema DSL parser as a pure function in a suitable
  crate. No side effects, easily testable.
- Remove dead `--stream` / `-s` and `--no-stream` / `-S` flags from `query`.
- Reassign `-s` as the short form of `--schema`.
- Wire the DSL parser into the `--schema` flag's value parser so it accepts
  both concise syntax and full JSON Schema.

No dependency on other RFDs.

### Phase 2: Output channel separation (RFD 019)

Implement [RFD 019]'s stdout/stderr split. Once chrome goes to stderr, piped
structured output is automatically clean. This is the architectural foundation
for the schema inference behavior.

### Phase 3: Schema output inference

With output channels separated, add the inference logic:

- When `--schema` is present and stdout is not a TTY (or `--format json`):
  only emit the structured JSON object to stdout.
- When `--schema` is present: default reasoning display to `Hidden` unless
  the user explicitly passes `-r` / `--reasoning`.
- Suppress `JsonEmitter` NDJSON when `--schema` is present in JSON format
  mode.

Depends on Phase 2.

### Phase 4: Templates with schemas (RFD 013)

Add a `schema` field to named templates. Depends on [RFD 013].

## References

- [RFD 013: Named Query Templates](013-named-query-templates.md)
- [RFD 019: Non-Interactive Mode](019-non-interactive-mode.md)
- [simonw/llm schemas documentation](https://llm.datasette.io/en/stable/schemas.html)
- [charmbracelet/mods](https://github.com/charmbracelet/mods) (sunset)
- [sigoden/aichat](https://github.com/sigoden/aichat)

[RFD 013]: 013-named-query-templates.md
[RFD 019]: 019-non-interactive-mode.md
