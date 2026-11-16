# RFD D14: Inquiry-Driven File Creation

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-04-02
- **Requires**: [RFD D13]
- **Summary**: Moves `fs_create_file` content from tool call arguments into a
  schema-typed inquiry to keep file bodies out of context.

## Summary

This RFD removes the `content` parameter from `fs_create_file` and delivers file
content through a schema-typed inquiry instead.
This prevents LLMs from wasting tokens in tool call arguments and keeps file
bodies out of the provider-visible conversation stream.

## Motivation

LLMs frequently misuse `fs_create_file` to overwrite existing files instead of
using `fs_modify_file` for incremental edits, despite the system prompt warning
("DO NOT re-create files that already exist").
When an LLM attempts to overwrite an existing file, the call proceeds as
follows:

```
1. LLM generates fs_create_file(path, content="<500 lines>")  ← tokens generated
2. Tool detects file exists → NeedsInput("Overwrite?")
3. Inquiry answers "true" or "false"
4. If false: 500 lines of content wasted
```

By the time the tool checks whether the file exists, the LLM has already
generated and transmitted the full file content.
Those tokens are stored in `ToolCallRequest.arguments`, which is
provider-visible, so they remain in the conversation's context window for every
subsequent turn.

A system prompt warning is a soft guardrail that LLMs follow inconsistently.
Removing `content` from the tool call is a hard guardrail: the LLM cannot send
file content before the tool checks the path.

### Token economics

`ToolCallRequest` events are sent to the provider on every subsequent turn,
whereas `InquiryResponse` events are stored in the conversation stream but
filtered out before provider requests are built.
Moving file content from `ToolCallRequest.arguments` to `InquiryResponse` keeps
it out of the context window across all three outcomes:

| Scenario                 | Current (content in args)               | Proposed (content in inquiry) |
| ------------------------ | --------------------------------------- | ----------------------------- |
| New file                 | Content in context forever              | Content not in context        |
| Existing, LLM overwrites | Content in context forever              | Content not in context        |
| Existing, LLM cancels    | Content in context forever (**wasted**) | Near-zero tokens              |

The largest savings occur when the LLM cancels an overwrite.
Instead of generating the full file upfront, the LLM sends a minimal
`fs_create_file(path)` call, learns from the inquiry prompt that the file
already exists, and decides whether to overwrite before generating any content.

## Design

### Overview

`fs_create_file` drops its `content` parameter and always returns `NeedsInput`
with an `AnswerType::Schema` question (see [RFD D13]).
The inquiry system routes the question to the assistant, which returns the file
content (or cancels the overwrite) as structured output.
The tool then writes the file or aborts.

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

The extra LLM round-trip is hidden from the user: the tool call, execution, and
result render in the terminal as a single step.

### Tool parameters

The tool's parameter schema changes from:

```
fs_create_file(path: string, content?: string)
```

To:

```
fs_create_file(path: string)
```

After validating `path`, the tool returns `NeedsInput` with question ID
`file_content`.
The schema and question text depend on whether the file already exists.

### New file

When the target file does not exist, the inquiry uses this schema:

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

> File '{path}' does not exist and will be created.
> Provide the file content.

When the inquiry returns, the tool creates any missing parent directories and
writes `content` to the new file.

### Existing file

When the target file already exists, the inquiry uses this schema:

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

> File '{path}' already exists ({size} bytes).
> To overwrite it, set `overwrite` to true and provide the full new content.
> If you only need to change part of the file, set `overwrite` to false and use
> `fs_modify_file` instead: `fs_modify_file` is more token-efficient because it
> only transmits the changed portions.
> Overwriting regenerates the entire file content, which consumes tokens and
> context window space.

When the inquiry returns:

- `overwrite: true` with `content`: truncate the file and write `content`.
- `overwrite: true` without `content`: truncate the file to zero bytes.
- `overwrite: false`: leave the file untouched and return a message directing
  the assistant to `fs_modify_file`.

### Inquiry model routing

Because the `file_content` inquiry writes the file body, it needs the capability
of the main conversation model rather than a smaller default inquiry model.
The question uses the `inherit` model alias ([RFD D13]) to route the inquiry to
the active conversation model:

```toml
[conversation.tools.fs_create_file.questions.file_content.target]
model.id = "inherit"
```

### Configuration

The `file_content` question replaces `overwrite_file`, and the tool and skill
definitions drop the `content` parameter:

| File                    | Old                                                     | New                                                                 |
| ----------------------- | ------------------------------------------------------- | ------------------------------------------------------------------- |
| `personas/dev.toml`     | `fs_create_file.questions.overwrite_file.answer = true` | `fs_create_file.questions.file_content.target.model.id = "inherit"` |
| `skill/rfd.toml`        | `fs_create_file.questions.overwrite_file.answer = true` | `fs_create_file.questions.file_content.target.model.id = "inherit"` |
| `skill/edit-files.toml` | Describes `content` parameter                           | Explains follow-up prompt                                           |
| `create_file.toml`      | Defines `content` parameter                             | Removes `content` parameter                                         |

`skill/edit-files.toml` describes the two-step flow:

> - fs_create_file: Create a new file.
>   Content is provided via a follow-up prompt, not inline.

### Formatting tool arguments

With `content` removed, the `FormatArguments` action no longer syntax-highlights
a file body when rendering tool calls in the terminal; it prints only the file
path.

## Drawbacks

- **Extra round-trip for every file creation.** Every `fs_create_file` call
  requires a structured-output inquiry call, even for new files with no
  conflict.
  The extra round-trip adds latency, traded off against lower context usage on
  subsequent turns.

- **Main-model cost for inquiries.** Routing `file_content` to `inherit` (e.g.
  Opus) costs more per inquiry than routing a simple boolean question to a
  smaller model (e.g. Haiku).
  Because the inquiry generates the file body, it needs the main model's
  capabilities, and the token cost matches what the model would have spent
  generating `content` inline in the tool call.

- **Populating empty files via `fs_modify_file` is unintuitive.** If the LLM
  truncates a file to zero bytes (`overwrite: true` without `content`) and later
  populates it through `fs_modify_file`, it must pass `old=""`.
  This is non-obvious, though uncommon because the inquiry collects the new
  content directly.

## Alternatives

### Keep `content` parameter, discard it for existing files

Keep `content` on `fs_create_file` but ignore it when the file exists, returning
`NeedsInput` with a select question offering "overwrite" or "cancel."

Rejected: the LLM still generates the full file body before the tool runs, and
those tokens remain in `ToolCallRequest.arguments` in the provider-visible
context window.

### Remove `content`, use `fs_modify_file` for all content

Have `fs_create_file` create only empty files, and require a follow-up
`fs_modify_file` call with `old=""` and `new="<content>"` to populate them.

Rejected: creating a file takes two tool calls, and `fs_modify_file` still
records the file body in `ToolCallRequest.arguments`, which is provider-visible.
Delivering content through `InquiryResponse` keeps it out of provider context.

## Non-Goals

- **Changing `fs_modify_file` behavior.** Incremental file edits continue to
  pass patterns inline in tool call arguments.
- **Applying this pattern to other tools.** Other tools that accept large
  content parameters (e.g. a hypothetical `fs_write_file`) are separate work.
- **Context compaction.** This RFD keeps file bodies out of the conversation
  context upfront; summarizing or trimming existing context is separate work.

## Risks and Open Questions

- **Habitual overwrites.** The existing-file prompt and schema instruct the LLM
  to set `overwrite: false` for partial edits, but models can still set
  `overwrite: true` out of habit.
  Real-world compliance needs testing, and the prompt or schema description can
  be tuned if models continue to overwrite.

- **Optional `content` field across providers.** The existing-file schema lists
  only `overwrite` in `required` and leaves `content` optional.
  Providers that require every schema property to appear in structured output
  will emit an empty `content` string even when `overwrite` is `false`, costing
  a few extra tokens.

## Implementation Plan

### Phase 1: Rewrite `fs_create_file` tool

Remove the `content` parameter from `create_file.rs`, implement the new-file and
existing-file inquiry flows using `AnswerType::Schema`, and update
`FormatArguments` rendering and unit tests.

Depends on [RFD D13] Phase 1 (`AnswerType::Schema`).

### Phase 2: Configuration updates

Update tool definitions (`create_file.toml`), skill descriptions
(`edit-files.toml`), and persona configs (`dev.toml`, `rfd.toml`) to remove the
`overwrite_file` question config and add the `file_content` question with
`inherit` model targeting.

Depends on Phase 1 and [RFD D13] Phase 3 (`inherit` alias).

## References

- [RFD D13: Schema Answer Type and Inherit Model Alias][RFD D13]: the
  infrastructure this RFD depends on.
- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028]: the inquiry
  system used for content delivery.
- [RFD 034: Inquiry-Specific Assistant Configuration][RFD 034]: per-question
  model targeting.

[RFD 028]: ../028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: ../034-inquiry-specific-assistant-configuration.md
[RFD D13]: D13-schema-answer-type-and-inherit-model-alias.md
