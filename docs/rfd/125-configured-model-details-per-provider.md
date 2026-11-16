# RFD 125: Configured Model Details per Provider

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-27
- **Summary**: Adds a per-provider `models` table that overrides model details
  from provider listings and built-in tables.

## Summary

Each LLM provider block gains a `models` table, keyed by the served model name,
that configures a model's context window, output limit, reasoning support,
structured output, and prefill support.
Configured values override both the provider's model listing and JP's built-in
tables on every request to that model.

## Motivation

JP shapes each request around the target model's `ModelDetails`: it clamps
reasoning effort to the levels the model supports, converts a token-count effort
to a level using the output limit, decides whether to prefill, and truncates
inquiry and title input to the context window.
JP obtains these details from two sources: the provider's model listing and
tables built into JP for providers whose listings omit those fields.

For a self-hosted server, neither source is sufficient. vLLM's `/v1/models`
endpoint reports only a model's ID and its served context window
(`max_model_len`).
No vLLM endpoint exposes the reasoning parser, output limit, or structured
output support; `/tokenizer_info` is the closest, but it is disabled by default
and vLLM's [security guide][vllm-security] advises against enabling it.
Because operators swap models on the server without rebuilding JP, a built-in
table cannot cover them either.
JP treats every model on the server as unknown and falls back to permissive
defaults for missing fields.
Reasoning efforts pass through unclamped, and as vLLM
[documents][vllm-reasoning], some chat templates silently fall back to maximum
reasoning when given an unrecognized effort.

Cloud providers have the same gap when a model launches after a JP release: the
model receives empty details (for example, Cerebras logs `Unknown Cerebras
model, using empty details.`) until a later JP release adds it to the built-in
table.

Without configuration support, supplying missing model details requires a code
change in JP, even when the user already knows the model's capabilities from
deploying it or reading its model card.

## Design

### Configuration

Every provider block accepts a `models` table keyed by the exact model name the
provider serves (the name that follows `/` in a model ID).
Names containing `.` or `/` must be quoted in TOML:

```toml
[providers.llm.vllm.models."deepseek-v4.1-flash"]
max_output_tokens = 65536
structured_output = true
reasoning = { mode = "leveled", levels = ["low", "high"] }

[providers.llm.vllm.models."Qwen/Qwen3.8-27B-FP8"]
reasoning = { mode = "budgetted", max_tokens = 32768 }

[providers.llm.cerebras.models."zai-glm-5"]
context_window = 131072
```

Each model entry accepts the following optional fields:

| Field               | Meaning                                                   |
| ------------------- | --------------------------------------------------------- |
| `context_window`    | Maximum combined prompt and output tokens.                |
| `max_output_tokens` | Maximum tokens generated per response.                    |
| `structured_output` | Whether the model supports JSON schema response formats.  |
| `prefill`           | Whether the model continues a trailing assistant message. |
| `reasoning`         | Reasoning capability configuration (see below).           |

`reasoning` accepts `"unsupported"` or a table with a `mode` field:

- `budgetted`: Token-budget reasoning, with optional `min_tokens` and
  `max_tokens` bounds.
- `leveled`: Discrete effort levels, specified in `levels` as a subset of
  `xlow`, `low`, `medium`, `high`, `xhigh`, and `max`.
- `adaptive`: Model-scheduled reasoning.
  Supports `low`, `medium`, and `high` by default; `levels` may additionally
  include `xhigh` and `max`.

A reasoning table also accepts `can_disable` (default `true`).
Set `can_disable = false` when the model rejects requests that disable
reasoning.

These fields mirror `ModelDetails` in `jp_llm`, omitting `subscription`,
`features`, `display_name`, `knowledge_cutoff`, and `deprecated`.
`subscription` and `features` are internal to provider implementations, and
`display_name`, `knowledge_cutoff`, and `deprecated` do not affect request
construction.
In particular, `features` holds provider-private tags such as
`sampling-parameters` and `between-tools-thinking`; exposing it directly would
turn every tag rename into a config migration.
If users later need to configure one of those capabilities, JP can expose it as
a dedicated field.

Every provider block also accepts `fetch_model_details` (default `true`).
Setting `fetch_model_details = false` skips querying the provider for model
details before each turn, inquiry, and title query, saving a network round trip
on slow connections or on servers without a model-details endpoint:

```toml
[providers.llm.vllm]
fetch_model_details = false
```

If a request is rejected for an unknown model, JP still fetches the model list
for the interactive model picker.

### Precedence and Request Behavior

Each field on `ModelDetails` resolves from the first source that sets it:

1. The model's `models` config entry.
2. The provider's API response for the model.
3. JP's built-in table for the provider.
4. The provider's fallback default for an unknown model.

Because resolution happens per field, a vLLM `models` entry can declare
`reasoning` levels while retaining the `max_model_len` reported by `/v1/models`,
and a provider-reported value overrides a stale built-in table entry.

Each entry matches a single model by exact served name; glob or regex patterns
are not supported.
Configuring a model that the provider omits from its listing does not add the
model to the listing, but the entry still applies when a conversation requests
that model directly.

Fetching model details from the provider never fails a query.
If the HTTP lookup fails or times out after three seconds, JP logs the failure
and falls back to the built-in table and defaults for any unconfigured fields.
A `models` entry can therefore supply accurate limits for gateways without a
model-listing endpoint, such as Synthetic's Anthropic-compatible endpoint.
Pre-request checks outside the HTTP lookup, such as missing credentials or
OpenAI's check that a ChatGPT subscription supports the requested model, still
fail the query.

`models` entries merge across config layers by model name and then by field,
matching `providers.llm.aliases`.
For example, a user-global config can set a model's `context_window` while a
workspace config sets its `reasoning`.

On vLLM, declaring `reasoning` in `models` does not enable reasoning by default.
When `assistant.model.parameters.reasoning` is unset, JP omits reasoning fields
from the request so the model's chat template and the server's
`--default-chat-template-kwargs` control the default behavior.
Once reasoning is configured, JP consults the `models` entry to clamp effort
levels to the ladder, bound token budgets, and determine how to send `off`.

Other providers treat configured capabilities identically to entries in their
built-in tables.
A model configured with `reasoning` receives the provider's default reasoning
effort when `assistant.model.parameters.reasoning` is unset, matching how Ollama
and Cerebras handle models in their built-in tables.

### Provider Overlay

Applying a configured entry to a `ModelDetails` is a pure function in `jp_llm`
that overwrites each field set in the entry.

`get_provider` builds each provider from its config block and wraps it in a
decorator holding that block's `models` table and `fetch_model_details` flag.
The decorator applies the overlay to the results of `model_details` and
`models`.
When `fetch_model_details` is `false`, the decorator bypasses `model_details`
and calls a new `Provider` method that returns details from the built-in table
without making a network request.
That method defaults to returning empty details, so only providers with a
built-in table override it.
Because `get_provider` wraps the provider directly, every caller (the query
turn, inquiries, title generation, summarization, and the unknown-model picker)
sees the same overlaid details.

Individual provider implementations do not read `models` config directly.
Each request builder consumes the overlaid `ModelDetails` fields it already
reads, and this RFD does not require every builder to support every field.
Phase 2 updates the vLLM request builder to respect `min_tokens`, `max_tokens`,
and `can_disable`, which the motivating use case depends on.

Provider-agnostic code accesses the tables through a single accessor on
`LlmProviderConfig`, keyed by `ProviderId`.
If JP later supports multiple instances of the same provider (such as two vLLM
servers), the `models` table stays inside its provider block and only that
accessor's lookup key changes.

### Limitation: `--cfg KEY=VALUE`

Because `--cfg KEY=VALUE` splits dotted keys on every `.`, it cannot address a
model entry whose name contains a `.`:
`providers.llm.vllm.models.deepseek-v4.1-flash.max_output_tokens=65536` splits
`deepseek-v4.1-flash` into two path segments.
Use a config file (`-c models.toml`) or a JSON `--cfg` argument instead:

```sh
jp q --cfg '{"providers":{"llm":{"vllm":{"models":{
  "deepseek-v4.1-flash":{"max_output_tokens":65536}}}}}}'
```

## Drawbacks

Misconfigured entries can break requests that would otherwise succeed.
For example, setting `prefill = true` for a model that rejects prefill causes
the provider to reject the request, and the resulting error does not point to
the config entry.

Because config overrides JP's built-in tables, a workaround entry in a user's
config continues to take precedence even after a later release updates the
built-in table.

The mirrored `ModelDetails` field names become part of the configuration surface
and the exported JSON Schema.
`ModelDetails` can still evolve internally, but renaming a mirrored field
requires a config migration.

## Alternatives

**A single map at `providers.llm.models`, keyed by `provider/name`.** This
places all overrides in one table alongside `aliases` instead of repeating
`models` on each provider block.
Rejected because the `provider/name` key format differs from every other key
under `providers.llm`, and supporting multiple instances of the same provider
would require re-keying the map.
A table inside each provider block scopes naturally to that provider instance.

**A `models` field on the vLLM block only.** Self-hosted servers need overrides
most often, but built-in tables for cloud providers also lag behind new model
releases, and adding the field to every provider block costs only one delegating
line per provider.

**Infer details from the chat template.** vLLM's `/tokenizer_info` endpoint
returns the chat template when enabled.
Rejected because the endpoint is disabled by default, vLLM advises against
enabling it, and inferring model capabilities from a Jinja template is
unreliable.

**A separate models file, like [pi's `models.json`][pi-models].** JP already
layers config files and loads additional files with `-c`.
Introducing a dedicated file format would require a second loader without adding
new capability.

## Non-Goals

- **Adding models to a provider's listing.** The model listing reflects what the
  server actually serves, and the unknown-model picker relies on that list.
- **Arbitrary request body fields**, such as custom chat template kwargs.
  Custom request fields shape provider-specific payloads rather than describing
  model capabilities.
  A vLLM operator can set server-wide kwargs with
  `--default-chat-template-kwargs`, whereas per-model custom request fields
  across providers require a separate design.
- **Configuring multiple instances of the same provider.** Scoping `models`
  inside each provider block avoids a config migration if multi-instance support
  is added later, but this RFD does not add multi-instance support.
- **Pricing and cost tracking.**

## Risks and Open Questions

The `reasoning` schema is the part of the configuration surface most likely to
need revision.
Each provider interprets `ReasoningDetails` differently (Anthropic's adaptive
thinking, Google's levels, Cerebras's efforts), and the config schema must
express every variant without exposing the Rust enum directly.

Some request builders ignore `ModelDetails` fields that their built-in tables
never populate.
Two such gaps exist in the vLLM request builder and are fixed in Phase 2: it
sends an absolute effort as `thinking_token_budget` without clamping to
`min_tokens` and `max_tokens`, and it translates `off` to `reasoning_effort:
"none"` without checking `can_disable`.
Other providers may have similar gaps when a user configures a field that the
provider's built-in table never sets.

It is an open question whether setting `context_window` below the server's
actual limit should also cap outbound requests, or only guide truncation and
compaction.
Under this RFD, a configured `context_window` only overrides the value on
`ModelDetails`, so every consumer of `context_window` behaves as if the provider
had reported that value.

## Implementation Plan

1. **Config type.** Add `ModelDetailsConfig` in `jp_config`, including its
   partial, delta, fill-defaults, to-partial, and key-assignment
   implementations.
   Add `models: MergeableMap<ModelDetailsConfig>` (merged with
   `map_with_strategy`) and `fetch_model_details` to each of the nine provider
   blocks, add the lookup accessor on `LlmProviderConfig`, and update the config
   schema snapshots.
   This phase does not change runtime behavior and can merge independently.
2. **Overlay.** Add the pure overlay function in `jp_llm`, the decorator applied
   in `get_provider`, the table-only `Provider` method used when
   `fetch_model_details` is `false`, and unit tests for each field, including a
   vLLM request whose effort is clamped to a configured ladder.
   Update the vLLM request builder with request-body tests for three cases:
   omitting reasoning fields when reasoning is unset (even when the model has a
   configured ladder), clamping an absolute effort to `min_tokens` and
   `max_tokens`, and handling `off` when `can_disable = false` by sending the
   lowest level for a `leveled` model or omitting the field otherwise.
   Depends on Phase 1.
3. **Documentation.** Update the provider section of the configuration guide
   with a vLLM example.
   Depends on Phase 2.

## References

- [vLLM reasoning outputs][vllm-reasoning], including `--reasoning-effort-map`
  and `--default-chat-template-kwargs`.
- [vLLM security guide][vllm-security], on `/tokenizer_info`.
- [pi's model configuration][pi-models], which separates added models from
  overrides of existing ones.

[pi-models]: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md
[vllm-reasoning]: https://docs.vllm.ai/en/latest/features/reasoning_outputs/
[vllm-security]: https://docs.vllm.ai/en/stable/usage/security/
