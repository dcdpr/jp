# RFD D14: Inquiry-Driven File Creation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02

## Summary

This RFD removes the `content` parameter from `fs_create_file` and delivers
file content through a schema-typed inquiry instead. This structurally prevents
LLMs from wasting tokens by dumping file contents into tool call arguments, and
moves file content out of the provider-visible conversation stream entirely.

## Motivation

LLMs frequently misuse `fs_create_file` to overwrite existing files instead of
using `fs_modify_file` for incremental edits. The system prompt tells them not
to, but they often ignore it. The current flow when an LLM overwrites an
existing file:

```
1. LLM generates fs_create_file(path, content="<500 lines>")  ← tokens generated
2. Tool detects file exists → NeedsInput("Overwrite?")
3. Inquiry answers "true" or "false"
4. If false: 500 lines of content wasted
```

The problem is structural: by the time we detect the file exists, the LLM has
already generated and transmitted the full file content. Those tokens are stored
in `ToolCallRequest.arguments`, which is provider-visible — they persist in the
conversation's context window for all future turns.

The system prompt reminder ("DO NOT re-create files that already exist") is a
soft guardrail. LLMs comply inconsistently. A hard guardrail — making it
physically impossible to send content in the tool call — is the only reliable
fix.

### Token economics

`ToolCallRequest` events are provider-visible: they're sent to the LLM on every
subsequent turn. `InquiryResponse` events are NOT provider-visible: they're
stored in the conversation stream but filtered out before reaching providers.

Moving file content from tool call arguments to inquiry answers has a direct
context window benefit:

| Scenario | Current (content in args) | Proposed (content in inquiry) |
|----------|--------------------------|-------------------------------|
| New file | Content in context forever | Content NOT in context |
| Existing, LLM overwrites | Content in context forever | Content NOT in context |
| Existing, LLM cancels | Content in context forever (**wasted**) | Near-zero tokens |

The third row is where the savings are largest. Today, when an LLM generates 500
lines for an existing file and then decides not to overwrite, those 500 lines
live in the context window permanently. With this change, the LLM generates only
a small `fs_create_file(path)` call, learns the file exists through the inquiry
question text, and makes an informed decision before generating any content.

## Design

### Overview

`fs_create_file` loses its `content` parameter and always returns `NeedsInput`
with an `AnswerType::Schema` question (see [RFD D13]). The inquiry system
routes this to the assistant, which provides file content through a structured
output response. The tool then creates or overwrites the file with the provided
content.

```
New file:
  LLM: fs_create_file(path="src/new.rs")      ← tiny tool call
  Tool: NeedsInput(schema={content: string})
  Inquiry LLM: {content: "fn main() {}"}       ← content in inquiry, not args
  Tool: creates file

Existing file:
  LLM: fs_create_file(path="src/lib.rs")       ← tiny tool call
  Tool: NeedsInput(schema={overwrite: bool, content?: string})
  Inquiry LLM: {overwrite: false}              ← LLM decides to cancel
  Tool: returns "use fs_modify_file instead"
```

The inquiry is invisible to the user. From the user's perspective, the tool
call appears, the tool runs, and the result appears — same as today. The extra
LLM round-trip happens behind the scenes.

### Tool parameter changes

The tool's parameter schema changes from:

```
fs_create_file(path: string, content?: string)
```

To:

```
fs_create_file(path: string)
```

The tool always returns `NeedsInput` after path validation. The question ID is
`file_content` in both cases, with different schemas and question text depending
on whether the file exists.

### New file path

Schema:

```json
{
  "type": "object",
  "properties": {
    "content": {
      "type": "string",
      "description": "The content to write to the new file."
    }
  },
  "required": ["content"],
  "additionalProperties": false
}
```

Question text:

> File '{path}' does not exist and will be created. Provide the file content.

On answer: create parent directories, create the file, write `content`.

### Existing file path

Schema:

```json
{
  "type": "object",
  "properties": {
    "overwrite": {
      "type": "boolean",
      "description": "Set to true to replace the existing file with new content.
        Set to false to cancel. If your intent is to modify part of the file
        rather than replace it entirely, set this to false and use
        fs_modify_file instead — it is more token-efficient and keeps the
        original content in place."
    },
    "content": {
      "type": "string",
      "description": "The new file content. Required when overwrite is true.
        Ignored when overwrite is false."
    }
  },
  "required": ["overwrite"],
  "additionalProperties": false
}
```

Question text:

> File '{path}' already exists ({size} bytes). To overwrite it, set `overwrite`
> to true and provide the full new content. If you only need to change part of
> the file, set `overwrite` to false and use `fs_modify_file` instead —
> `fs_modify_file` is more token-efficient because it only transmits the changed
> portions. Overwriting regenerates the entire file content, which consumes
> tokens and context window space.

On answer:
- `overwrite: true` + `content`: truncate file, write content.
- `overwrite: true` + no `content`: truncate file (empty).
- `overwrite: false`: return message suggesting `fs_modify_file`.

### Inquiry model routing

The `file_content` inquiry generates file content — a task that requires the
main assistant model. The question is configured with the `inherit` model alias
([RFD D13]) so the inquiry goes to the same model as the main conversation:

```toml
[conversation.tools.fs_create_file.questions.file_content.target]
model.id = "inherit"
```

### Configuration changes

The existing `overwrite_file` question config becomes obsolete. All config files
that reference it are updated:

| File | Old | New |
|------|-----|-----|
| `personas/dev.toml` | `fs_create_file.questions.overwrite_file.answer = true` | `fs_create_file.questions.file_content.target.model.id = "inherit"` |
| `skill/rfd.toml` | `fs_create_file.questions.overwrite_file.answer = true` | `fs_create_file.questions.file_content.target.model.id = "inherit"` |
| `skill/edit-files.toml` | Describes `content` param | Updated description |
| `create_file.toml` | Has `content` parameter | Parameter removed |

The skill description in `edit-files.toml` is updated:

> - fs_create_file: Create a new file. Content is provided via a follow-up
>   prompt, not inline.

### Format arguments mode

The `FormatArguments` action (used for rendering tool calls in the terminal)
is simplified. It no longer needs to syntax-highlight a `content` parameter
because there is none. The output shows only the file path.

## Drawbacks

- **Extra round-trip for every file creation.** Every `fs_create_file` call now
  requires an inquiry round-trip, even for new files where there's no conflict.
  This adds latency (one structured output LLM call). The latency is offset by
  context window savings and the inquiry being invisible to the user.

- **`inherit` model means inquiry cost equals main model cost.** Routing the
  inquiry to the main model (e.g. Opus) instead of a cheap model (Haiku) is
  more expensive per call. This is unavoidable — generating file content
  requires the main model's capabilities. The cost is comparable to what the
  LLM would have spent generating content inline in the tool call arguments.

- **`fs_modify_file` on empty files is unintuitive.** If an LLM creates an
  empty file via `fs_create_file` (by answering `overwrite: true` with no
  content) and later wants to populate it, it must call `fs_modify_file` with
  `old=""`. This works but is not obvious. In practice, this path should be
  rare — the inquiry provides content directly.

## Alternatives

### Keep `content` parameter, discard it for existing files

Keep `content` on `fs_create_file` but ignore it when the file exists. Return
a `NeedsInput` with a select offering "overwrite" or "cancel."

Rejected: the LLM still generates and transmits the full file content before
we can intervene. Those tokens are stored in `ToolCallRequest.arguments`
(provider-visible) and persist in the context window. The token waste is the
core problem and this approach doesn't address it.

### Remove `content`, use `fs_modify_file` for all content

The tool only creates empty files. Content is always provided via a separate
`fs_modify_file` call with `old=""`, `new="<content>"`.

Rejected: requires two tool calls for every new file (create + modify). The
`fs_modify_file` call puts content in `ToolCallRequest.arguments`, which is
provider-visible — so there is no context window benefit over the current
design. The inquiry approach moves content to `InquiryResponse`, which is NOT
provider-visible.

## Non-Goals

- **Changing `fs_modify_file` behavior.** The modify tool is unaffected by
  this change.
- **Applying this pattern to other tools.** Other tools that accept large
  content parameters (e.g. hypothetical `fs_write_file`) could benefit from the
  same pattern, but that is separate work.
- **Context compaction.** Reducing existing conversation context through
  summarization or trimming. This RFD prevents content from entering the
  context in the first place; compaction addresses content that's already there.

## Risks and Open Questions

- **LLM compliance with schema-based overwrite decisions.** The existing-file
  schema asks the LLM to set `overwrite: false` when it should use
  `fs_modify_file`. LLMs might still set `overwrite: true` habitually. The
  question text is explicit about the trade-off, but real-world compliance needs
  validation. If LLMs consistently overwrite, the schema description may need
  tuning.

- **Optional `content` field across providers.** The existing-file schema uses
  `required: ["overwrite"]` without requiring `content`. This is valid JSON
  Schema but providers' structured output implementations may handle optional
  fields differently. If a provider always generates all properties regardless
  of `required`, the LLM may produce an empty `content` string even when
  `overwrite` is false — wasting tokens on an empty field. This is minor
  (a few tokens) but worth monitoring.

## Implementation Plan

### Phase 1: Rewrite `fs_create_file` tool

Remove the `content` parameter from `create_file.rs`. Implement the two
inquiry paths (new file, existing file) using `AnswerType::Schema`. Update
the `FormatArguments` rendering. Update unit tests.

Depends on [RFD D13] Phase 1 (`AnswerType::Schema`).

### Phase 2: Configuration updates

Update tool definitions (`create_file.toml`), skill descriptions
(`edit-files.toml`), and persona configs (`dev.toml`, `rfd.toml`). Remove
the old `overwrite_file` question configs. Add `file_content` question with
`inherit` model targeting.

Depends on Phase 1 and [RFD D13] Phase 3 (`inherit` alias).

## References

- [RFD D13: Schema Answer Type and Inherit Model Alias][RFD D13] — the
  infrastructure this RFD depends on.
- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028] — the
  inquiry system used for content delivery.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034] — per-question
  model targeting.

[RFD D13]: D13-schema-answer-type-and-inherit-model-alias.md
[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
