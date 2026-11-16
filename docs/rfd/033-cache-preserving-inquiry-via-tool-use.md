# RFD 033: Cache-Preserving Inquiry via Tool Use

- **Status**: Superseded
- **Superseded by**: [RFD 034]
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07
- **Summary**: Replaces structured-output inquiries with a forced
  `answer_inquiry` tool call to avoid invalidating the provider's prompt cache.

## Summary

This RFD replaces structured output (`output_config.format`) in the inquiry
system ([RFD 028]) with a forced tool call.
A built-in `answer_inquiry` tool stays in the tool list across all requests, and
inquiries invoke it via `tool_choice: Function("answer_inquiry")` to preserve
the system prompt cache.

## Motivation

The inquiry system ([RFD 028]) makes a separate LLM call when a tool needs input
from the assistant.
These calls use structured output (`output_config.format`) to guarantee that the
response matches a JSON schema.

In practice, every inquiry request triggers a **complete prompt cache miss** on
Anthropic, rewriting ~95k tokens at 125% cost instead of reading them at 10%
cost.
Two issues cause this:

### 1. Empty tool list

The inquiry backend sends `tools: vec![]` while normal requests include the full
tool definitions.
Anthropic's prompt cache prefix follows the hierarchy `tools → system →
messages`.
Omitting the tools section changes the top-level prefix and invalidates the
entire cache.

Passing the same tool definitions to the inquiry backend fixes this bug (merged
separately), but structured output still invalidates the cache.

### 2. Structured output invalidates the system cache

Anthropic's structured output feature injects an additional system prompt
describing the expected output format.
From the [Anthropic docs][cache-docs]:

> When using structured outputs, Claude automatically receives an additional
> system prompt explaining the expected output format.
> Changing the output_config.format parameter will invalidate any prompt cache
> for that conversation thread.

Even with matching tool definitions, the injected prompt causes a system-level
cache miss on every inquiry.
With ~95k tokens of conversation context, each inquiry wastes roughly:

```
Cache write: 95,000 × $5.00/MTok × 1.25 = $0.59
Cache read:  95,000 × $5.00/MTok × 0.10 = $0.05
Waste per inquiry: ~$0.55
```

For turns with 3 inquiries (common with multi-file modifications), that adds
~$1.65 in avoidable cost per turn plus the latency of reprocessing the full
context.

### Cross-provider applicability

Other providers with prefix-based prompt caching (OpenAI, Google) face the same
issue when structured output alters the request prefix.
Handling inquiries through standard tool use preserves the prompt cache across
all providers.

## Design

### Overview

Replace `output_config.format` with a built-in `answer_inquiry` tool that uses
`strict: true` for schema validation.
The tool stays in the tool definitions even when no inquiry is active, keeping
the tools prefix constant.
During an inquiry, the backend sets `tool_choice: Function("answer_inquiry")` to
force the LLM to call the tool.

Cache impact per the [Anthropic docs][cache-invalidation]:

| Component        | Changes during inquiry?         | Cache impact        |
| ---------------- | ------------------------------- | ------------------- |
| Tool definitions | No (always present)             | ✓ Cache hit         |
| System prompt    | No (`output_config` not used)   | ✓ Cache hit         |
| `tool_choice`    | Yes (`Auto` → `Function(...)`)  | Message blocks only |
| Messages         | Yes (inquiry question appended) | Expected miss       |

Only the appended inquiry message misses the cache.

### The `answer_inquiry` tool

`answer_inquiry` uses a fixed schema across all inquiries:

```json
{
  "name": "answer_inquiry",
  "description": "Answer a question from a tool that needs additional input. Call this tool when the system indicates a tool requires your input. Use the inquiry_id from the question and provide your answer as a JSON string.",
  "strict": true,
  "input_schema": {
    "type": "object",
    "properties": {
      "inquiry_id": {
        "type": "string",
        "description": "The inquiry ID from the question prompt."
      },
      "answer": {
        "type": "string",
        "description": "Your answer, formatted as instructed in the question prompt."
      }
    },
    "required": ["inquiry_id", "answer"],
    "additionalProperties": false
  }
}
```

### Why `answer` is a string

Typing `answer` per inquiry (boolean, enum, string) would alter the tool
definition on each call and invalidate the tools cache.
Keeping `answer` as a string leaves the tool definition static.

This shifts type validation from provider-side constrained decoding to post-hoc
parsing in JP.
The question prompt specifies the expected format for each answer type:

| Type    | Prompt instruction                  | Validation       |
| ------- | ----------------------------------- | ---------------- |
| Boolean | "Answer exactly `true` or `false`." | Parse as bool    |
| Select  | "Answer with one of: A, B, C."      | Check membership |
| Text    | "Answer with free-form text."       | Accept as-is     |

### Inquiry flow

`LlmInquiryBackend::inquire` executes an inquiry in six steps:

```
1. Build ChatRequest with question text + formatting instructions
   (no schema attached — schema field is None)
2. Build ChatQuery with:
   - tools: self.tools (same as parent turn — includes answer_inquiry)
   - tool_choice: ToolChoice::Function("answer_inquiry")
3. Send to provider via collect_with_retry
4. Extract tool call arguments from response
5. Parse answer.answer as the expected type
6. On validation failure: retry with error feedback (up to N attempts)
```

### Answer extraction and validation

The provider returns a `ToolCallRequest` rather than structured text.
`extract_answer` reads `inquiry_id` and `answer` from the tool call arguments
and parses the answer into the expected type:

```rust
fn extract_answer(
    tool_call: &ToolCallRequest,
    expected_id: &str,
    answer_type: &AnswerType,
) -> Result<Value, InquiryError> {
    let args = &tool_call.arguments;

    // Validate inquiry_id
    let id = args.get("inquiry_id")
        .and_then(Value::as_str)
        .ok_or(InquiryError::AnswerExtraction {
            reason: "missing inquiry_id".into()
        })?;
    if id != expected_id {
        return Err(InquiryError::AnswerExtraction {
            reason: format!("id mismatch: expected '{expected_id}', got '{id}'")
        });
    }

    // Parse answer string into expected type
    let raw = args.get("answer")
        .and_then(Value::as_str)
        .ok_or(InquiryError::AnswerExtraction {
            reason: "missing answer field".into()
        })?;

    match answer_type {
        AnswerType::Boolean => match raw {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(/* retry-eligible error */)
        },
        AnswerType::Select { options } => {
            if options.contains(&raw.to_string()) {
                Ok(Value::String(raw.to_string()))
            } else {
                Err(/* retry-eligible error */)
            }
        },
        AnswerType::Text => Ok(Value::String(raw.to_string())),
    }
}
```

### Retry on validation failure

Because the schema accepts any string, the LLM can return a value that fails
type parsing, such as `"yes"` instead of `"true"` for a boolean.
On validation failure, the backend retries up to 2 times, appending the error as
a user message:

```
Turn 1 (inquiry):
  User: "A tool requires input. Answer true or false: Create backup?"
  Assistant: answer_inquiry(id="...", answer="yes")  ← invalid

Turn 2 (retry):
  User: "Invalid answer 'yes'. Must be exactly 'true' or 'false'."
  Assistant: answer_inquiry(id="...", answer="true")  ← valid
```

Each retry reuses the cached prefix (tools, system prompt, and prior messages),
writing only the new error message to the cache.

### Tool registration

`jp_llm::tool::builtin` defines `answer_inquiry` as a `BuiltinTool`, and the
query command includes it in the tool definitions alongside `describe_tools`
whenever tools are enabled.

The tool exists only to expose its schema to the LLM; the inquiry backend
extracts the tool call arguments directly without running the tool executor.
If the LLM calls `answer_inquiry` during the normal tool loop, `execute` returns
an error:

```rust
// In BuiltinExecutors setup:
executors.register("answer_inquiry", AnswerInquiryTool);

// AnswerInquiryTool::execute always returns an error — it should
// only be called via the inquiry backend, not the normal tool loop.
```

### Changes to `LlmInquiryBackend`

`LlmInquiryBackend` changes as follows:

1. `ChatRequest.schema` is `None`.
2. `ChatQuery.tool_choice` is `ToolChoice::Function("answer_inquiry")`.
3. The backend processes the response as tool call events rather than structured
   text events.

The `InquiryBackend` trait and `MockInquiryBackend` are unchanged, as the trait
returns `Value` regardless of the underlying mechanism.

## Drawbacks

- **Weaker type safety.** Structured output guarantees schema compliance via
  constrained decoding, whereas a string-typed tool parameter relies on prompt
  instructions and post-hoc validation.
  In practice, boolean and select formats are simple enough that LLMs rarely
  misformat them, and retries handle the remaining cases.

- **Extra retries on malformed answers.** Structured output never retries for
  schema violations.
  Tool use may require a retry on an estimated <5% of inquiries, though each
  retry hits the prompt cache and adds only a short error message.

- **Always-present tool.** Including `answer_inquiry` in every request adds
  ~50-100 tokens to the tool definitions even when no inquiry runs.
  The LLM might also call the tool unprompted, though the tool description and
  the error returned by the executor mitigate this.

## Alternatives

### Keep structured output, pass same tools

Passing the full tool definitions to the inquiry backend (already implemented)
fixes the empty-tools cache invalidation, but structured output still injects a
system prompt and causes a system-level cache miss on every inquiry.
With ~95k tokens of context and multiple inquiries per turn, the remaining cache
misses are still expensive.

### Provider-specific strategy selection

Select structured output or tool use per provider based on caching behavior (for
example, using structured output on providers without prefix caching and tool
use on providers with it).

Rejected because it adds branching complexity without a clear benefit.
All major providers (Anthropic, OpenAI, Google) use prefix-based caching that
structured output invalidates.
We can revisit this if we add a provider where structured output preserves the
cache.

### Per-inquiry dynamic tool schema

Define the `answer` parameter with the exact type for each inquiry (boolean,
enum, string) instead of a generic string.

Rejected because changing the tool schema per inquiry alters the tools prefix
and invalidates the tools cache, trading a system-level cache miss for a
tools-level cache miss.

## Non-Goals

- **Batching multiple inquiry questions into a single tool call.**
  `answer_inquiry` answers one question at a time.
  Batching is independent and can be added later.
- **Rendering inquiry tool calls in conversation output.** Inquiries remain
  hidden from user-facing output.
- **Replacing structured output for non-inquiry uses.** The `schema` field on
  `ChatRequest` and `output_config.format` remain available for other features,
  such as scriptable structured output via `jp query --schema`.

## Risks and Open Questions

- **Unprompted `answer_inquiry` calls.** If the LLM calls `answer_inquiry`
  outside an inquiry, the executor returns an error and the turn continues.
  The tool description discourages unprompted calls, but we should monitor how
  often they occur.

- **Answer parsing edge cases.** The LLM might return `"True"` or `"TRUE"`
  instead of `"true"`.
  Boolean parsing should be case-insensitive, while select parsing requires an
  exact match against the options listed in the prompt.

- **Interaction with `ToolChoice::Function` and reasoning.** Anthropic does not
  support extended thinking when `tool_choice` forces a specific tool.
  The inquiry backend does not enable reasoning, and if it does later, the
  existing soft-force fallback in `create_request` already handles this
  constraint.

## Implementation Plan

### Phase 1: `answer_inquiry` builtin tool

- Add `AnswerInquiryTool` and its `ToolDefinition` constructor to
  `jp_llm::tool::builtin`.
- Implement the `BuiltinTool` trait so the executor returns an error when called
  outside the inquiry backend.
- Register `answer_inquiry` in the builtin executors alongside `describe_tools`.

Can be merged independently.

### Phase 2: Always include `answer_inquiry` in tool definitions

- Wire the `answer_inquiry` definition into the tool list construction in
  `jp_cli::cmd::query` whenever tools are enabled.
- Verify that the tool appears in the API request without unexpected token count
  changes.

Can be merged independently.

### Phase 3: Rewrite `LlmInquiryBackend` to use tool calls

- Remove `schema` from the inquiry `ChatRequest`.
- Set `tool_choice: ToolChoice::Function("answer_inquiry")`.
- Process the response as tool call events instead of structured text.
- Add answer parsing (string → bool/select/text) with validation.
- Add a retry loop (up to 2 retries) with error feedback messages.
- Update unit tests.

Depends on Phase 2.

### Phase 4: Cleanup

- Remove the `tools: vec![]` workaround now that tools are always passed through
  and `answer_inquiry` is always present.
- Update [RFD 028] status to Superseded by this RFD.
- Verify cache behavior in Anthropic API logs: inquiry requests should show
  cache reads matching normal requests.

Depends on Phase 3.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028]: the
  structured-output inquiry implementation superseded by this RFD.
- [Anthropic prompt caching docs][cache-docs]: cache prefix hierarchy and
  invalidation rules.
- [Anthropic structured outputs docs][structured-docs]: system prompt injection
  that causes cache invalidation.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 034]: 034-inquiry-specific-assistant-configuration.md
[cache-docs]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching
[cache-invalidation]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching#what-invalidates-the-cache
[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
