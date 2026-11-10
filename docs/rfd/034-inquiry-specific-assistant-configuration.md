# RFD 034: Inquiry-Specific Assistant Configuration

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-03-07
- **Supersedes**: [RFD 033]
- **Extends**: [RFD 028]
- **Summary**: Adds dedicated assistant configuration and cache policy overrides
  for tool inquiries so questions can route to cheaper models.

## Summary

This RFD extends the inquiry system ([RFD 028]) with dedicated assistant
configuration (model, system prompt, and request settings) for tool inquiries.
Global defaults live at `conversation.inquiry.assistant`, and individual
questions can override them via
`QuestionTarget::Assistant(PartialAssistantConfig)`.
Together with stable output schemas and a configurable `CachePolicy`, this lets
inquiries run on smaller models with their own prompt cache.

## Motivation

The inquiry system ([RFD 028]) makes a separate LLM call when a tool needs input
from the assistant.
Every inquiry triggers a **complete prompt cache miss** on Anthropic for two
reasons:

1. **Empty tool list.** Inquiry requests send `tools: vec![]`, whereas normal
   requests include the full tool definitions.
   Because tools are the first component of the cache prefix (`tools → system
   → messages`), omitting them invalidates the entire cache.
   Passing the same tool definitions fixes this mismatch, but the second issue
   still invalidates the cache.

2. **Structured output.** Inquiry requests set `output_config.format`, which
   causes Anthropic to inject an additional system prompt.
   As noted in the [Anthropic docs][structured-docs]:

   > Changing the output_config.format parameter will invalidate any prompt
   > cache for that conversation thread.

   Even when the tool definitions match, the injected system content changes the
   system prompt and invalidates the cache for the system prompt and all
   subsequent messages.

At a base input cost of $5/MTok (Claude Opus 4.6) and a cache-write rate of 125%
of base, a 100k-token conversation costs $0.625 per inquiry.
A turn with three inquiries, which is common during multi-file edits, adds
$1.875 in avoidable cache writes.

### Why cache preservation falls short

[RFD 033] proposes replacing structured output with a built-in `answer_inquiry`
tool to keep the cache prefix intact.
That avoids the system prompt injection from `output_config.format`, but
changing `tool_choice` from `Auto` to `Function("answer_inquiry")` still
invalidates the **messages** cache, which accounts for roughly 80% of the prompt
tokens.
The savings are marginal:

| Approach                                       | Cost per inquiry (Opus 4.6, ~100k Ctx) |
| ---------------------------------------------- | -------------------------------------- |
| Current (broken cache)                         | ~$0.63                                 |
| RFD 033 (tool use)                             | ~$0.60                                 |
| **This RFD (Haiku 4.5, uncached)**             | **~$0.10**                             |
| **This RFD (Haiku 4.5, cached, 2nd+ inquiry)** | **~$0.02**                             |
| **This RFD (Haiku 3, uncached)**               | **~$0.025**                            |

Routing inquiries to a smaller model with its own cache cuts the cost far more
than trying to preserve the main model's cache.

## Design

### Overview

The inquiry backend routes inquiry requests to a dedicated model instead of
reusing the parent conversation's model, building its own provider, model, and
system prompt from configuration.
Because inquiries run against a separate model, they do not affect the main
conversation's prompt cache.
With stable output schemas, multiple inquiries within the same turn also share a
prompt cache with one another.

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

The `assistant` field is a `PartialAssistantConfig`, and unset fields fall back
to the parent `assistant` config.

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

`QuestionTarget::Assistant` carries an inline `PartialAssistantConfig`:

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
`Assistant(PartialAssistantConfig::default())`, where all fields are `None` and
resolution falls back to the global inquiry defaults and then the parent
`assistant` config.

#### Resolution order

When an inquiry runs, its effective configuration is resolved by merging three
layers:

```
per-question PartialAssistantConfig   (from QuestionTarget::Assistant)
  ↓ fills gaps from
conversation.inquiry.assistant        (global inquiry defaults)
  ↓ fills gaps from
assistant                             (main model config)
```

This uses the same partial-config merge pattern as the rest of the configuration
system.

### `CachePolicy` on `RequestConfig`

A `cache` field on `RequestConfig` controls prompt caching:

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

When `cache` is `Off`, the Anthropic provider omits the top-level
`cache_control` field and all explicit cache breakpoints; other providers
respect the same setting.
On Anthropic, `Short` and `Long` map to `ephemeral` with the default TTL and a
`1h` TTL, respectively.
Providers that support arbitrary TTLs (such as Google) receive `Custom`
durations directly; providers with fixed tiers round to the nearest supported
duration.

### Stable inquiry schemas

In [RFD 028], each inquiry schema embeds the unique inquiry ID in a `const`
field:

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

Embedding the ID changes the schema on every inquiry, which has two costs:

- It invalidates the prompt cache because `output_config.format` changes.
- It forces the provider to recompile the grammar artifact on each inquiry
  (Anthropic caches compiled grammars by schema, adding latency the first time a
  schema is seen).

Removing `inquiry_id` from the schema makes the schema stable for each answer
type:

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

The inquiry ID moves to the prompt text ("The inquiry ID is
`call_abc.apply_changes`.").
The `const` field in the schema was only a convenience for stateless matching;
`ActiveInquiry` already tracks the ID, and the coordinator's `spawn_inquiry`
knows which tool call owns each inquiry.

With stable schemas, multiple inquiries of the same answer type within a turn
share the same `output_config.format`.
Combined with an empty tool list and a shared system prompt, subsequent
inquiries hit the prompt cache on the shared prefix:

| Inquiry     | Schema           | Prefix match                     | Cache behavior       |
| ----------- | ---------------- | -------------------------------- | -------------------- |
| 1 (boolean) | `{answer: bool}` | None                             | Write 100% tokens    |
| 2 (boolean) | `{answer: bool}` | tools + system + shared messages | Read ~95%, write ~5% |
| 3 (boolean) | `{answer: bool}` | tools + system + shared messages | Read ~95%, write ~5% |

`Select` questions still produce distinct schemas when their option sets (`enum`
values) differ.
Two approaches were considered:

1. Move the options into the prompt text and use `{"answer": {"type":
   "string"}}` for all select questions.
   This stabilizes the schema across option sets but drops schema-level
   validation.
2. Keep the `enum` constraint in the schema and accept that select inquiries
   with different option sets do not share a cache entry.

Because boolean and free-text inquiries are the common cases, keeping schema
validation (option 2) outweighs cache sharing across distinct `Select` prompts.

### Inquiry backend changes

`LlmInquiryBackend` merges `conversation.inquiry.assistant` onto the parent
`assistant` config at construction time and stores the resolved values directly
as its defaults:

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

When `inquire` is called, it receives the per-question `PartialAssistantConfig`.
When that partial is empty (all fields `None`), the backend uses its stored
defaults directly.
When the partial specifies overrides, the backend merges them onto its defaults
and retrieves the target provider from the `Ctx` provider registry.

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

Every inquiry request:

- Sends `tools: vec![]` and `ToolChoice::None`.
- Uses a stable per-answer-type schema without `inquiry_id`.
- Uses structured output for type-safe answers.

### Provider registry on `Ctx`

To avoid constructing duplicate providers and to support per-question model
overrides efficiently, `Ctx` caches providers in a registry:

```rust
pub struct Ctx {
    // ...existing fields...
    providers: IndexMap<ProviderId, Arc<dyn Provider>>,
}
```

Providers are constructed on first use and cached for the lifetime of the
process.
The inquiry backend retrieves providers from this registry instead of
constructing its own instances, and other multi-model features can share the
same cache.

### Structured output support on `ModelDetails`

`ModelDetails` exposes a helper to check whether a model supports structured
output:

```rust
impl ModelDetails {
    pub fn supports_structured_output(&self) -> bool {
        self.structured_output.is_some_and(|v| v)
    }
}
```

At startup, JP validates that every configured inquiry model supports structured
output.
If `structured_output` is `Some(false)` (a known model without structured output
support), startup fails with an error before any work begins.
If it is `None` (an unknown model, such as a custom deployment), JP logs a
warning and proceeds.

### Default behavior

When `conversation.inquiry` is unset and a question uses `target = "assistant"`,
inquiries inherit the parent conversation's model, system prompt, and cache
policy, preserving backward compatibility with [RFD 028].

Set `conversation.inquiry.assistant` to route all assistant inquiries to a
smaller model by default:

```toml
[conversation.inquiry.assistant]
model.id = "anthropic/claude-haiku-4-5"
```

Set `target` on an individual question to override the model for that question
alone:

```toml
[tools.fs_modify_file.questions.apply_changes]
target.model.id = "anthropic/claude-haiku-3"
```

## Drawbacks

- **Smaller models may give worse answers.** Haiku is less capable than Opus at
  reasoning about complex tool interactions.
  That rarely matters for simple boolean prompts such as "Create backup?", but
  it can degrade answers to open-ended text questions.
  Per-question overrides let users route demanding questions to a stronger
  model.

- **Configuration layering.** Merging three layers (per-question override,
  global inquiry default, and parent assistant config) adds mental overhead.
  Documentation should lead with the common setup: a single global default in
  `conversation.inquiry.assistant` paired with `target = "assistant"` on each
  question.

- **Provider registry lifetime.** Providers cached on `Ctx` live for the entire
  process.
  If credentials rotate during a long-running session, the cached provider keeps
  using the old credentials.
  This is acceptable for short-lived CLI invocations but may need revisiting for
  long-lived server processes.

## Alternatives

### Cache-preserving tool use (RFD 033)

Replacing structured output with a built-in `answer_inquiry` tool ([RFD 033])
preserves the tool and system cache prefix, but changing `tool_choice` still
invalidates the messages cache (~95k tokens).
That saves only about $0.03 per inquiry on Opus 4.6, whereas routing inquiries
to a smaller model saves more than $0.50 per inquiry.

### Subset config (`InquiryAssistantConfig`)

Defining a narrower `InquiryAssistantConfig` struct with only inquiry-specific
fields was rejected because every part of `AssistantConfig` (system prompt,
instructions, model parameters, and request settings) can be useful for an
inquiry.
Reusing `PartialAssistantConfig` is more flexible and avoids maintaining a
parallel config type.

### Context compaction

Sending only recent conversation context (such as the last one or two turns) to
the inquiry model could reduce the cost per inquiry to ~$0.001 on Haiku 3.
However, deciding which context an inquiry needs requires compaction heuristics
that merit their own RFD.
Sending the full conversation stream is safe and accurate, so compaction is
deferred.

## Known Limitations

### Context truncation for smaller inquiry models

When the inquiry model has a smaller context window than the main model, the
inquiry backend truncates older conversation events to fit.
The token count is estimated from character length (~3 characters per token)
with a 20% margin for the system prompt, tool definitions, and provider
metadata.

Re-truncating to the exact budget on every inquiry would shift the message
prefix whenever the conversation grows, invalidating the prompt cache.
Instead, the backend drops events from the start of the stream in increments of
10% of the target token budget, keeping the cutoff point stable across
consecutive inquiries in the same turn.

### Cross-inquiry prompt cache misses

Despite the stable truncation prefix, Anthropic's prompt caching does not
produce cross-inquiry cache hits on conversation messages.
The system prompt and tool definitions (~27k tokens in production) hit the cache
through explicit breakpoints, but the conversation messages (~47k tokens after
truncation) are rewritten in full on each inquiry.

The root cause is that Anthropic's automatic caching uses a 20-block lookback
window from the last message.
Between inquiries, new tool responses and inquiry results are appended to the
conversation, shifting block indices.
When the lookback compares blocks by position, it hits those shifted indices and
stops before reaching the stable prefix.

Fixing this requires an explicit cache breakpoint at the boundary between the
stable conversation prefix and the per-inquiry tail.
However, Anthropic limits requests to four explicit breakpoints, and all four
slots are already allocated (tools, system prompt, attachments, and the
automatic breakpoint on the last message).

One potential fix is to introduce provider-agnostic **cache hint events** in
`ConversationStream`.
The inquiry backend would insert a hint event at the end of the stable prefix,
right before the synthetic "Tool paused" response.
Providers that support explicit cache control (such as Anthropic) would
translate the hint into a `cache_control` breakpoint on the nearest message
block, while other providers would ignore it.
That breakpoint could reuse the slot currently taken by the automatic
last-message breakpoint, which is wasted on inquiries because the final message
changes every time.

This optimization is deferred to a future RFD on conversation compaction and
cache management.

> [!TIP]
> [RFD 036] introduces conversation compaction strategies, including
> LLM-assisted summarization, that shrink long conversation contexts and reduce
> the token volume affected by these cache misses.

## Non-Goals

- **Context compaction.** The inquiry backend truncates the oldest events when a
  conversation exceeds the inquiry model's context window, but selective
  compaction (such as summarization or middle-out trimming) is out of scope.
- **Inquiry batching.** Combining multiple tool questions into a single LLM
  request is out of scope.
- **Provider credential validation at startup.** Verifying credentials for all
  configured providers before starting work is useful on its own, but separate
  from this RFD.

## Implementation Plan

### Phase 1: `CachePolicy` on `RequestConfig`

Add the `CachePolicy` enum and the `cache` field on `RequestConfig`, with
deserialization from booleans and strings (including `humantime` durations for
`Custom`).
When `cache` is `Off`, skip all `cache_control` annotations in the Anthropic
provider.
Add configuration and provider tests.

This phase can be merged independently.

### Phase 2: Stable inquiry schemas

Remove `inquiry_id` from the structured output schema in `create_inquiry_schema`
and include the ID in the prompt text instead.
Update `ActiveInquiry::extract_answer` to read only the `answer` field without
validating `inquiry_id` in the response, and update the existing tests.

This phase can be merged independently.

### Phase 3: Provider registry on `Ctx`

Add an `IndexMap<ProviderId, Arc<dyn Provider>>` registry to `Ctx`, populated
lazily on first access.
Refactor `query.rs` to fetch providers from the registry instead of constructing
them inline.

This phase can be merged independently.

### Phase 4: `InquiryConfig` and `QuestionTarget` config changes

Add `InquiryConfig` (with `assistant: PartialAssistantConfig`) to
`ConversationConfig`, and update `QuestionTarget::Assistant` to carry a
`PartialAssistantConfig` with a custom deserializer that accepts both string and
map forms.
Add configuration tests.

This phase can be merged independently.

### Phase 5: `ModelDetails::supports_structured_output`

Add `ModelDetails::supports_structured_output` and validate configured inquiry
models at startup, failing when structured output is known to be unsupported and
warning when support is unknown.

This phase can be merged independently.

### Phase 6: Inquiry backend with resolved config

Update `LlmInquiryBackend` to merge the three configuration layers (per-question
override, global inquiry config, and parent assistant config) and store the
resolved provider, model, and system prompt as defaults.
Update `inquire` to accept the per-question `PartialAssistantConfig`, retrieve
overridden providers from the `Ctx` registry, omit tool definitions, and apply
`CachePolicy`.

Depends on Phases 1 through 5.

### Phase 7: Default tool configs

Update the default JP tool configurations (such as `fs_modify_file`) in
`.jp/config.toml` to set `conversation.inquiry.assistant.model.id` to a
recommended smaller model.

Depends on Phase 6.

## References

- [RFD 028: Structured Inquiry System for Tool Questions][RFD 028]: Base inquiry
  architecture.
- [RFD 033: Cache-Preserving Inquiry via Tool Use][RFD 033]: Alternative
  tool-based approach superseded by this RFD.
- [Anthropic prompt caching docs][cache-docs]: Cache prefix hierarchy and
  invalidation rules.
- [Anthropic structured outputs docs][structured-docs]: System prompt injection
  behavior for `output_config.format`.

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 033]: 033-cache-preserving-inquiry-via-tool-use.md
[RFD 036]: 036-conversation-compaction.md
[cache-docs]: https://platform.claude.com/docs/en/build-with-claude/prompt-caching
[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
[structured-docs]: https://platform.claude.com/docs/en/build-with-claude/structured-outputs
