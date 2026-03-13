# RFD 034: Inquiry-Specific Assistant Configuration

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07

## Summary

This RFD extends the inquiry system ([RFD 028]) to support a dedicated assistant
configuration — model, system prompt, request settings — for inquiry requests. A
global default lives at `conversation.inquiry.assistant`, with per-question
overrides via `QuestionTarget::Assistant(PartialAssistantConfig)`. Combined with
stable schemas for cross-inquiry cache reuse and a new `CachePolicy` setting on
`RequestConfig`, this reduces inquiry costs by 5-25x compared to the current
approach.

## Motivation

The inquiry system ([RFD 028]) makes a separate LLM call when a tool needs input
from the assistant. Every inquiry triggers a **complete prompt cache miss** on
Anthropic for two reasons:

1. **Empty tool list.** The inquiry sends `tools: vec![]` while normal requests
   include full tool definitions. Since tools are the first component of the
   cache prefix hierarchy (`tools → system → messages`), this invalidates
   everything. (Fixed separately by passing the same tools — but insufficient
   alone.)

2. **Structured output.** The inquiry uses `output_config.format`, which causes
   Anthropic to inject an additional system prompt. From the [Anthropic
   docs][structured-docs]:

   > Changing the output_config.format parameter will invalidate any prompt
   > cache for that conversation thread.

   Even with matching tools, the system-level cache misses because the injected
   system content differs.

If we take a base cost of $5/MTok (Claude Opus 4.6), and a median conversation
of 100k tokens at a cache-write cost (125% of base), paying $0.625 per inquiry.
For turns with 3 inquiries (common with multi-file modifications), that is
$1,875 in avoidable cost.

### Why cache preservation doesn't work

An alternative approach ([RFD 033]) proposes replacing structured output with a
built-in `answer_inquiry` tool to preserve the cache prefix. This avoids the
structured output injection, but `tool_choice` changes (from `Auto` to
`Function("answer_inquiry")`) still invalidate the **messages** cache — which is
the bulk of the tokens (~80%). The savings are therefore marginal.

| Approach                            | Cost per inquiry (Opus 4.6, ~100k Ctx) |
|-------------------------------------|----------------------------------------|
| Current (broken cache)              | ~$0.63                                 |
| RFD 033 (tool use)                  | ~$0.60                                 |
| **This RFD (Haiku 4.5, uncached)**  | **~$0.10**                             |
| **This RFD (Haiku 4.5, cached, 2nd+ | **~$0.02**                             |
| inquiry)**                          |                                        |
| **This RFD (Haiku 3, uncached)**    | **~$0.025**                            |

The right framing is not "preserve the main model's cache" but "use a cheap
model and build its own cache."

[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs

## Design

### Overview

Route inquiry requests to a dedicated, cheap model instead of reusing the parent
conversation's model. The inquiry backend builds its own provider, model, and
system prompt from configuration. Since the inquiry goes to a different model,
it has zero impact on the main conversation's cache — and with stable schemas,
multiple inquiries within a turn can reuse each other's cache.

```txt
Main conversation (Opus 4.6):
  User → Assistant → ToolCall → Tool executes → NeedsInput
                                                    ↓
Inquiry (Haiku 4.5):
  [conversation context] + question → cheap model → structured answer
                                                    ↓
Main conversation (Opus 4.6):
  Tool re-executes with answer → ToolCallResponse → continues
```

### Configuration

#### Global default: `conversation.inquiry.assistant`

A new `InquiryConfig` on `ConversationConfig` provides the global default for
all assistant-targeted inquiries:

```toml
[conversation.inquiry.assistant]
model.id = "anthropic/claude-haiku-4-5"
system_prompt = "Answer tool questions concisely based on the conversation context."
request.cache = "off"
```

The `assistant` field is a `PartialAssistantConfig`. Unset fields fall back to
the parent assistant config.

```rust
/// Inquiry-specific configuration.
pub struct InquiryConfig {
    /// Assistant configuration for inquiry requests.
    ///
    /// Overrides the parent assistant config for inquiry LLM calls.
    /// Unset fields fall back to the parent assistant config.
    pub assistant: PartialAssistantConfig,
}
```

#### Per-question override: `QuestionTarget::Assistant`

The existing `QuestionTarget::Assistant` variant gains an inline
`PartialAssistantConfig`:

```rust
pub enum QuestionTarget {
    /// Ask the question to the user.
    User,

    /// Ask the question to the assistant.
    ///
    /// The partial config overrides the global inquiry config, which in turn
    /// overrides the parent assistant config.
    Assistant(PartialAssistantConfig),
}
```

A custom deserializer accepts both string and map forms:

```toml
# String form: use global inquiry defaults (all fields None)
target = "assistant"

# Map form: per-question overrides
[tools.fs_modify_file.questions.apply_changes.target]
model.id = "anthropic/claude-haiku-3"
```

The string `"assistant"` deserializes to
`Assistant(PartialAssistantConfig::default())` — all fields `None`, meaning "use
global inquiry defaults, then main model defaults."

#### Resolution order

When an inquiry fires, the effective config is resolved by merging three layers:

```
per-question PartialAssistantConfig   (from QuestionTarget::Assistant)
  ↓ fills gaps from
conversation.inquiry.assistant        (global inquiry defaults)
  ↓ fills gaps from
assistant                             (main model config)
```

This follows the same merge-partial pattern used throughout the config system.

### CachePolicy on RequestConfig

A new `cache` field on `RequestConfig` controls prompt caching behavior:

```rust
pub enum CachePolicy {
    /// No caching. Equivalent to `false`.
    Off,

    /// 5-minute TTL, or whatever a reasonable short duration is per provider
    /// (default). Equivalent to `true`.
    Short,

    /// 1-hour TTL, or whatever a reasonable long duration is per provider.
    Long,

    /// Custom duration. Not all providers support arbitrary durations;
    /// unsupported values are rounded to the nearest available option.
    Custom(Duration),
}
```

Deserialization accepts booleans and strings:

```toml
cache = false # Off
cache = true # Short
cache = "off" # Off
cache = "short" # Short (5m)
cache = "long" # Long (1h)
cache = "10m" # Custom(10 minutes)
```

When `Off`, the Anthropic provider skips the top-level `cache_control` and all
explicit breakpoints. Other providers follow the same flag. The `Short` and
`Long` variants map to Anthropic's `ephemeral` with default and `1h` TTL
respectively. `Custom` durations are passed through where supported (e.g.,
Google) and rounded to the nearest available option otherwise.

### Stable inquiry schemas

Currently, each inquiry schema includes a `const` field with a unique inquiry
ID:

```json
{
  "properties": {
    "inquiry_id": {
      "type": "string",
      "const": "call_abc.apply_changes"
    },
    "answer": {
      "type": "boolean"
    }
  }
}
```

This changes the schema per inquiry, which:

- Invalidates the prompt cache (different `output_config.format`).
- Forces recompilation of the grammar artifact (Anthropic caches compiled
  grammars by schema, with additional latency on the first use of each schema).

The fix: remove `inquiry_id` from the schema. The schema becomes stable per
answer type:

```json
{
  "type": "object",
  "properties": {
    "answer": {
      "type": "boolean"
    }
  },
  "required": [
    "answer"
  ],
  "additionalProperties": false
}
```

The inquiry ID moves to the prompt text: "The inquiry ID is
`call_abc.apply_changes`." The `ActiveInquiry` struct already tracks the ID on
our side — the schema `const` was a statelessness convenience, not a
requirement. The coordinator's `spawn_inquiry` already knows which tool call
each inquiry belongs to.

With stable schemas, multiple inquiries of the same answer type within a turn
share the same `output_config.format`. Combined with matching tools (empty) and
system prompt, this enables prompt cache hits on the shared prefix:

| Inquiry     | Schema           | Prefix match                     | Cache behavior       |
|-------------|------------------|----------------------------------|----------------------|
| 1 (boolean) | `{answer: bool}` | —                                | Write 100% tokens    |
| 2 (boolean) | `{answer: bool}` | tools + system + shared messages | Read ~95%, write ~5% |
| 3 (boolean) | `{answer: bool}` | tools + system + shared messages | Read ~95%, write ~5% |

Select questions still vary by option set (`enum` values differ). Two
approaches:

1. Move options to prompt text and use `{"answer": {"type": "string"}}` for all
   selects. Loses schema-level validation but stabilizes the schema.
2. Accept that select inquiries with different option sets don't cache-share.
   Boolean and text are the common cases.

Approach 2 is strictly better, as it keeps the type safety of the schema.

### Inquiry backend changes

`LlmInquiryBackend` stores the resolved (merged) defaults from the global
inquiry config and parent assistant config. No "override" fields — the stored
values ARE the final defaults.

```rust
pub struct LlmInquiryBackend {
    // Resolved from: conversation.inquiry.assistant > parent assistant
    provider: Arc<dyn Provider>,
    model: ModelDetails,
    system_prompt: Option<String>,
    sections: Vec<SectionConfig>,
    attachments: Vec<Attachment>,
}
```

When `inquire` is called, it receives the per-question `PartialAssistantConfig`
(if any). If the partial is empty (all `None`), the backend uses its stored
defaults directly. If the partial has overrides, it merges them on top and
resolves a provider from the `Ctx` provider registry.

The `InquiryBackend` trait signature gains the per-question config:

```rust
#[async_trait]
pub trait InquiryBackend: Send + Sync {
    async fn inquire(
        &self,
        events: ConversationStream,
        inquiry_id: &str,
        question: &Question,
        config: &PartialAssistantConfig,
        cancellation_token: CancellationToken,
    ) -> Result<Value, InquiryError>;
}
```

In all cases, the inquiry:

- Sends `tools: vec![]` and `ToolChoice::None`.
- Uses a stable schema (no `inquiry_id` in the schema).
- Keeps structured output for type safety.

### Provider registry on `Ctx`

To avoid constructing duplicate providers and to support per-question model
overrides efficiently, `Ctx` gains a provider registry:

```rust
pub struct Ctx {
    // ...existing fields...
    providers: IndexMap<ProviderId, Arc<dyn Provider>>,
}
```

Providers are constructed on first use and cached for the process lifetime. The
inquiry backend grabs providers from this registry rather than constructing its
own. This also benefits future multi-model features.

### Structured output support on `ModelDetails`

Add a convenience method for checking structured output support:

```rust
impl ModelDetails {
    pub fn supports_structured_output(&self) -> bool {
        self.structured_output.is_some_and(|v| v)
    }
}
```

At startup, if the inquiry model is configured, validate that it supports
structured output. If `None` (unknown model, e.g. custom deployment), accept
with a warning. If explicitly unsupported (known model without the feature),
surface a clear error before starting any work.

### Default behavior

When no inquiry config is set (`conversation.inquiry` absent, question target
is `"assistant"` string), the inquiry uses the parent conversation's model,
system prompt, and caching. This preserves backward compatibility with
[RFD 028].

To use a cheap model, set the global default:

```toml
[conversation.inquiry.assistant]
model.id = "anthropic/claude-haiku-4-5"
```

To override for a specific question:

```toml
[tools.fs_modify_file.questions.apply_changes]
target.model.id = "anthropic/claude-haiku-3"
```

## Drawbacks

- **Cheaper model may make worse decisions.** Haiku is less capable than Opus at
  reasoning about complex tool interactions. For simple boolean questions
  ("Create backup?") this is unlikely to matter. For complex free-text questions
  it could. The per-question granularity lets users route complex questions to a
  more capable model.

- **Config complexity.** Three-layer merge (per-question → global inquiry →
  parent assistant) is powerful but adds cognitive overhead. The simple case
  (`target = "assistant"` with a global default) should be well-documented as
  the recommended starting point.

- **Provider registry lifecycle.** Storing providers on `Ctx` means they live
  for the process lifetime. If credentials rotate during a long session, the old
  provider persists. Acceptable for CLI usage; may need revisiting for
  long-lived server processes.

## Alternatives

### Cache-preserving tool use (RFD 033)

Replace structured output with a built-in `answer_inquiry` tool to preserve the
cache prefix. Saves ~$0.03 per inquiry because `tool_choice` changes still
invalidate the messages cache (~95k tokens). This RFD saves ~$0.50+ per inquiry
by using a cheap model.

### Subset config (`InquiryAssistantConfig`)

Define a focused subset of `AssistantConfig` with only the fields relevant to
inquiries. Rejected: there's nothing in `AssistantConfig` that can't be useful
for inquiries (system prompt, instructions, model parameters, request config).
Using the full `PartialAssistantConfig` is more flexible and avoids maintaining
a parallel config type.

### Context compaction

Reduce the conversation context sent to the inquiry model (e.g., last 1-2 turns
only). Could bring inquiry cost down to ~$0.001 even on Haiku 3. Left as a
future optimization — the current approach sends the full conversation stream,
which is safe and correct. Compaction requires heuristics about what context the
model needs, which first needs to be resolved in an RFD.

## Known Limitations

### Context truncation for smaller inquiry models

When the inquiry model has a smaller context window than the main model, the
inquiry backend truncates older conversation events to fit. The truncation
uses a character-based token estimate (~3 chars/token) with a 20% overhead
margin for system prompt, tools, and provider metadata.

To avoid re-truncating on every inquiry in a turn (which would shift the
message prefix and bust the prompt cache), the truncation drops events from
the start of the stream and rounds the drop amount to a coarse granularity
(10% of the target budget). This keeps the cutoff point stable when the stream
grows slightly between inquiries.

### Cross-inquiry prompt cache misses

Despite a stable truncation prefix, Anthropic's prompt caching does not
achieve cross-inquiry cache hits on the conversation messages. The system
prompt and tool definitions (~27k tokens in production) cache correctly via
explicit breakpoints, but the conversation messages (~47k tokens after
truncation) are fully rewritten on each inquiry.

The root cause: Anthropic's automatic caching uses a 20-block lookback window
from the last message. Between inquiries, new events are inserted into the
conversation (tool responses, inquiry results) which shift block indices. The
lookback compares blocks by position, finds mismatches due to the index shift,
and gives up before reaching the stable prefix.

The fix requires an explicit cache breakpoint at the boundary between the
stable conversation prefix and the per-inquiry tail. However, Anthropic limits
explicit breakpoints to 4 per request, and all slots are currently allocated
(automatic on last message, system prompt, attachments, tools).

A possible solution: introduce provider-agnostic **cache hint events** in the
`ConversationStream`. The inquiry backend would insert a hint event at the end
of the stable prefix (right before the synthetic "Tool paused" response).
Providers that support caching (Anthropic) translate the hint to a
`cache_control` breakpoint on the nearest message block; other providers
ignore it. This could potentially reuse one of the existing breakpoint slots
(e.g. the automatic last-message breakpoint, which is wasted on inquiries
since the last message always differs).

This optimization is deferred to a future RFD on conversation compaction and
cache management.

> [!TIP]
> [RFD 036](036-conversation-compaction.md) introduces conversation compaction
> strategies — including LLM-assisted summarization — that reduce long
> conversation context, addressing the underlying problem the cache miss issue
> here stems from.

## Non-Goals

- **Context compaction.** Reducing the conversation events sent to the inquiry
  model. A crude truncation is implemented for context window overflow, but
  smarter compaction (summarization, middle-out trimming) is orthogonal.
- **Inquiry batching.** Combining multiple questions into a single request.
- **Provider credential validation at startup.** Validating that all configured
  providers have working credentials before starting. Worth doing but separate
  from this RFD.

## Implementation Plan

### Phase 1: `CachePolicy` on `RequestConfig`

Add the `CachePolicy` enum and `cache` field to `RequestConfig`. Implement
deserialization (bool and string forms, including `humantime` parsing for
`Custom`). When `Off`, the Anthropic provider skips all `cache_control`
annotations. Add config and provider tests.

Can be merged independently.

### Phase 2: Stable inquiry schemas

Remove `inquiry_id` from the structured output schema in
`create_inquiry_schema`. Move the ID to the prompt text. Update
`ActiveInquiry::extract_answer` to only extract the `answer` field (no
`inquiry_id` validation from the response). Update tests.

Can be merged independently.

### Phase 3: Provider registry on `Ctx`

Add `IndexMap<ProviderId, Arc<dyn Provider>>` to `Ctx`. Populate it lazily on
first access. Refactor `query.rs` to use the registry instead of constructing
providers inline.

Can be merged independently.

### Phase 4: `InquiryConfig` and `QuestionTarget` config changes

Add `InquiryConfig` to `ConversationConfig` with `assistant:
PartialAssistantConfig`. Change `QuestionTarget::Assistant` to carry a
`PartialAssistantConfig`. Implement custom deserializer for the string/map
form. Add config tests.

Can be merged independently.

### Phase 5: `ModelDetails::supports_structured_output`

Add the convenience method. Add startup validation that checks all configured
inquiry models support structured output (error if known-unsupported, warn if
unknown).

Can be merged independently.

### Phase 6: Inquiry backend with resolved config

Rewrite `LlmInquiryBackend` construction to merge the three config layers
(per-question → global inquiry → parent assistant) into final values. The
backend stores resolved provider, model, system prompt. The `inquire` method
accepts the per-question `PartialAssistantConfig` for overrides, grabs
providers from the `Ctx` registry. Strip tools, apply `CachePolicy`.

Depends on Phases 1-5.

### Phase 7: Default tool configs

Update the default JP tool configs (e.g., `fs_modify_file`) to set
`conversation.inquiry.assistant.model.id` to a recommended cheap model in the
project's `.jp/config.toml`.

Depends on Phase 6.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028] — the
  current inquiry implementation.
- [RFD 033: Cache-Preserving Inquiry via Tool Use][RFD 033] — alternative
  approach (superseded by this RFD).
- [Anthropic prompt caching docs][cache-docs] — cache prefix hierarchy and
  invalidation rules.
- [Anthropic structured outputs docs][structured-docs] — system prompt
  injection behavior.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 033]: 033-cache-preserving-inquiry-via-tool-use.md
[cache-docs]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching
[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
