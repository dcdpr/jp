# RFD 033: Cache-Preserving Inquiry via Tool Use

- **Status**: Superseded
- **Superseded by**: [RFD 034](034-inquiry-specific-assistant-configuration.md)
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07

## Summary

This RFD replaces the structured output mechanism (`output_config.format`) used
by the inquiry system ([RFD 028]) with a strict tool use approach. A built-in
`answer_inquiry` tool is always present in the tool list and is invoked via
`tool_choice: Function("answer_inquiry")` during inquiries. This eliminates two
sources of prompt cache invalidation that currently cause complete cache misses
on every inquiry request.

## Motivation

The inquiry system ([RFD 028]) makes a separate LLM call when a tool needs
input from the assistant. These calls use structured output
(`output_config.format`) to guarantee the response matches a JSON schema.

In practice, every inquiry request triggers a **complete prompt cache miss**
on Anthropic, rewriting ~95k tokens at 125% cost instead of reading them at
10% cost. Two issues cause this:

### 1. Empty tool list

The inquiry backend sends `tools: vec![]` while normal requests include the full
tool definitions. Anthropic's prompt cache prefix follows the hierarchy
`tools → system → messages`. Removing the tools section changes the prefix at
the top level, invalidating everything.

This is a straightforward bug, fixed by passing the same tool definitions to the
inquiry backend (merged separately). But fixing it alone is not sufficient
because of issue 2.

### 2. Structured output invalidates the system cache

Anthropic's structured output feature injects an additional system prompt
describing the expected output format. From the [Anthropic docs][cache-docs]:

> When using structured outputs, Claude automatically receives an additional
> system prompt explaining the expected output format. [...] Changing the
> output_config.format parameter will invalidate any prompt cache for that
> conversation thread.

Even with matching tool definitions, the inquiry request gets a system-level
cache miss because the system content differs (injected structured output
prompt). With ~95k tokens of conversation context, each inquiry wastes roughly:

```
Cache write: 95,000 × $5.00/MTok × 1.25 = $0.59
Cache read:  95,000 × $5.00/MTok × 0.10 = $0.05
Waste per inquiry: ~$0.55
```

For turns with 3 inquiries (common with multi-file modifications), that is
~$1.65 in avoidable cost per turn, plus the latency of reprocessing the full
context.

### Cross-provider applicability

While the data above is from Anthropic, other providers with prefix-based prompt
caching (OpenAI, Google) are likely to exhibit the same behavior: structured
output changes the effective request shape, busting the cache. A solution that
works within the existing tool use mechanism avoids this class of problem across
all providers.

[cache-docs]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching
[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs

## Design

### Overview

Replace `output_config.format` with a built-in `answer_inquiry` tool that uses
`strict: true` for schema validation. The tool is always present in the tool
definitions — even when no inquiry is active — so it never changes the tools
prefix. During an inquiry, the backend sets
`tool_choice: Function("answer_inquiry")` to force the LLM to call it.

Cache impact per the [Anthropic docs][cache-invalidation]:

| Component | Changes during inquiry? | Cache impact |
|-----------|------------------------|--------------|
| Tool definitions | No (always present) | ✓ Cache hit |
| System prompt | No (`output_config` not used) | ✓ Cache hit |
| `tool_choice` | Yes (`Auto` → `Function(...)`) | Message blocks only |
| Messages | Yes (inquiry question appended) | Expected miss |

The only cache miss is on the new message content, which is unavoidable and
minimal.

[cache-invalidation]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching#what-invalidates-the-cache

### The `answer_inquiry` tool

A generic tool with a deliberately simple, stable schema:

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

The `answer` field is always a string. Type-specific constraints (boolean values,
select options) are communicated in the question prompt and validated after
extraction.

### Why `answer` is a string

A per-inquiry typed schema (boolean, enum, etc.) would require changing the tool
definition per inquiry, which invalidates the tools cache — the exact problem
we're solving. A stable, generic schema means the tool definition never changes.

The tradeoff is that validation moves from the provider (constrained decoding)
to our code (post-hoc parsing). For the three answer types:

| Type | Prompt instruction | Validation |
|------|-------------------|------------|
| Boolean | "Answer exactly `true` or `false`." | Parse as bool |
| Select | "Answer with one of: A, B, C." | Check membership |
| Text | "Answer with free-form text." | Accept as-is |

### Inquiry flow

The revised flow in `LlmInquiryBackend::inquire`:

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

The response is a `ToolCallRequest` instead of a structured text response.
Extraction pulls `inquiry_id` and `answer` from the tool call arguments:

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

Since the schema is generic (string answer), the LLM might return a malformed
answer — e.g., `"yes"` instead of `"true"` for a boolean. The backend retries
up to 2 times, appending the error as a user message:

```
Turn 1 (inquiry):
  User: "A tool requires input. Answer true or false: Create backup?"
  Assistant: answer_inquiry(id="...", answer="yes")  ← invalid

Turn 2 (retry):
  User: "Invalid answer 'yes'. Must be exactly 'true' or 'false'."
  Assistant: answer_inquiry(id="...", answer="true")  ← valid
```

Each retry reuses the same cached prefix (tools + system + prior messages), so
only the new error message is a cache write.

### Tool registration

`answer_inquiry` is registered as a `BuiltinTool` alongside `describe_tools`.
It is always included in the tool definitions when any tools are enabled. The
tool's `execute` method is never called during normal tool execution — it only
serves as a schema carrier for the LLM. If the LLM calls it outside an inquiry
context, the executor returns an error message.

```rust
// In BuiltinExecutors setup:
executors.register("answer_inquiry", AnswerInquiryTool);

// AnswerInquiryTool::execute always returns an error — it should
// only be called via the inquiry backend, not the normal tool loop.
```

The tool definition is constructed in `jp_llm::tool::builtin` and added to the
tool list by the query command alongside `describe_tools`.

### Changes to `LlmInquiryBackend`

The backend no longer needs to set a schema on the `ChatRequest`. Instead:

1. The `ChatRequest.schema` field is `None`.
2. The `ChatQuery.tool_choice` is `ToolChoice::Function("answer_inquiry")`.
3. The response is processed as tool call events instead of structured text
   events.

The `InquiryBackend` trait and `MockInquiryBackend` are unchanged — the trait
returns `Value` regardless of the underlying mechanism.

## Drawbacks

- **Weaker type safety.** Structured output guarantees the response matches the
  schema via constrained decoding. Tool use with a generic string answer relies
  on prompt instructions and post-hoc validation. In practice, boolean and
  select answers are simple enough that LLMs get them right on the first attempt
  the vast majority of the time, and the retry mechanism handles the rest.

- **Extra retries on malformed answers.** A structured output response never
  needs a retry for schema violations. The tool use approach may occasionally
  need 1 retry (estimated <5% of inquiries based on the simplicity of the
  answer types). Each retry is cheap (cache hit + small message delta).

- **Always-present tool.** `answer_inquiry` appears in every request even when
  no inquiry is happening. This adds a small constant to the tool definitions
  token count (~50-100 tokens). The LLM may occasionally try to call it
  unprompted, though this is mitigated by the description making its purpose
  clear and the executor returning an error.

## Alternatives

### Keep structured output, pass same tools

This is the partial fix already implemented: pass the same tool definitions to
the inquiry backend to avoid the empty-tools cache bust. However, the structured
output system prompt injection still causes a system-level cache miss on every
inquiry. For conversations with ~95k tokens of context and multiple inquiries
per turn, the cost is substantial.

### Provider-specific strategy selection

Choose between structured output and tool use based on provider caching
heuristics — e.g., use structured output for providers without prefix caching,
tool use for those with it.

Rejected for now: adds complexity with unclear benefit. All major providers
(Anthropic, OpenAI, Google) use some form of prefix-based caching, and
structured output is likely to bust the cache on all of them. If a provider
is found where structured output doesn't affect caching, this can be revisited.

### Per-inquiry dynamic tool schema

Define the `answer` parameter with the exact type for each inquiry (boolean,
enum, string) instead of a generic string.

Rejected: changing the tool schema per inquiry changes the tools prefix, which
invalidates the tools cache — the same problem as the current approach, just
at a different level. A stable, generic schema is the whole point.

## Non-Goals

- **Batching multiple inquiry questions into a single tool call.** The
  `answer_inquiry` tool answers one question at a time. Batching is orthogonal
  and can be layered on later.
- **Rendering inquiry tool calls in conversation output.** The inquiry remains
  invisible to the user (same as today).
- **Replacing structured output for non-inquiry uses.** The `schema` field on
  `ChatRequest` and `output_config.format` support remain for other features
  (e.g., scriptable structured output via `jp query --schema`).

## Risks and Open Questions

- **LLM calling `answer_inquiry` unprompted.** If the LLM calls the tool
  outside an inquiry context, the executor returns an error and the turn
  continues normally. The tool description should be clear enough to prevent
  this in practice, but it should be monitored.

- **Answer parsing edge cases.** The LLM might return `"True"` or `"TRUE"`
  instead of `"true"`. The parser should be case-insensitive for booleans.
  For selects, exact match is required (the prompt lists the exact options).

- **Interaction with `ToolChoice::Function` and reasoning.** Anthropic does not
  support extended thinking when `tool_choice` forces a specific tool. The
  inquiry backend currently does not enable reasoning, so this is not a problem
  today. If reasoning is later enabled for inquiries, this constraint will need
  to be handled (the existing soft-force fallback in `create_request` already
  covers this case).

## Implementation Plan

### Phase 1: `answer_inquiry` builtin tool

Add `AnswerInquiryTool` to `jp_llm::tool::builtin`. Implement the
`BuiltinTool` trait (executor returns an error — it's only called via the
inquiry backend). Add the `ToolDefinition` constructor.

Register it in the builtin executors alongside `describe_tools`.

Can be merged independently.

### Phase 2: Always include `answer_inquiry` in tool definitions

Wire the `answer_inquiry` definition into the tool list construction in
`jp_cli::cmd::query`. Ensure it's present whenever tools are enabled.

Verify that the tool appears in the API request and doesn't change token
counts unexpectedly.

Can be merged independently.

### Phase 3: Rewrite `LlmInquiryBackend` to use tool calls

- Remove `schema` from the inquiry `ChatRequest`.
- Set `tool_choice: ToolChoice::Function("answer_inquiry")`.
- Process the response as tool call events instead of structured text.
- Add answer parsing (string → bool/select/text) with validation.
- Add retry loop (up to 2 retries) with error feedback messages.
- Update unit tests.

Depends on Phase 2.

### Phase 4: Cleanup

- Remove the `tools: vec![]` workaround from the previous fix (tools are
  now always passed through and `answer_inquiry` is always present).
- Update [RFD 028] status to Superseded by this RFD.
- Verify cache behavior with Anthropic API logs: inquiry requests should show
  cache reads matching normal requests.

Depends on Phase 3.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028] — the
  current implementation this RFD supersedes.
- [Anthropic prompt caching docs][cache-docs] — cache prefix hierarchy and
  invalidation rules.
- [Anthropic structured outputs docs][structured-docs] — documents the system
  prompt injection that causes cache invalidation.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
