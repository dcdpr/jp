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

JP shapes each request around a `ModelDetails`: it clamps a reasoning effort to
the levels a model supports, converts a token-count effort to a level using the
output limit, decides whether to prefill, and truncates inquiry and title input
to the context window.
Those details come from two places: the provider's model listing, and tables
built into JP for providers whose listing omits those fields.

For a self-hosted server, neither source is enough. vLLM's `/v1/models` reports
a model's id and its served context window (`max_model_len`), and nothing else.
No vLLM endpoint reports the reasoning parser, the output limit, or whether
structured output works; the closest, `/tokenizer_info`, is off by default and
vLLM's [security guide][vllm-security] advises against enabling it.
The server swaps models without JP being rebuilt, so a built-in table cannot
keep up either.
Every model on such a server is unknown to JP, so JP falls back to permissive
defaults for any missing field.
For reasoning, that means efforts pass through unclamped, and vLLM
[documents][vllm-reasoning] that some chat templates silently fall back to
maximum reasoning for an effort they do not recognize.

Cloud providers have the same gap on a smaller scale.
A model released after the JP binary gets empty details (Cerebras logs `Unknown
Cerebras model, using empty details.`) until a JP release adds it to the table.

Without configuration support, supplying missing model details requires a code
change, even when the user who deployed the model or read its model card already
knows its capabilities.

## Design

### Configuration

Every provider block takes a `models` table.
The key is the model name exactly as the provider serves it, the same name that
follows the `/` in a model id; names with a `.` or `/` are quoted, as TOML
requires.

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

An entry has these fields, each optional:

| Field               | Meaning                                                   |
| ------------------- | --------------------------------------------------------- |
| `context_window`    | Maximum combined prompt and output tokens.                |
| `max_output_tokens` | Maximum tokens generated per response.                    |
| `structured_output` | Whether the model honours a JSON schema response format.  |
| `prefill`           | Whether the model continues a trailing assistant message. |
| `reasoning`         | Reasoning configuration; see below.                       |

`reasoning` is `"unsupported"`, or a table naming a `mode`:

- `budgetted`: reasoning measured in tokens, with optional `min_tokens` and
  `max_tokens`.
- `leveled`: discrete efforts, listed in `levels` from `xlow`, `low`, `medium`,
  `high`, `xhigh`, `max`.
- `adaptive`: the model schedules its own reasoning; `levels` may add `xhigh`
  and `max` to the base `low`, `medium`, `high`.

A reasoning table also takes `can_disable`, default `true`; set it to `false`
for a model that rejects a request to turn reasoning off.

The fields mirror `ModelDetails` in `jp_llm`.
`subscription`, `features`, `display_name`, `knowledge_cutoff`, and `deprecated`
are omitted: the first two are internal to the provider implementation, and the
last three do not affect request construction.
`features` in particular holds each provider's private tags
(`sampling-parameters`, `between-tools-thinking`), so exposing it would make
every rename of a tag a config migration.
If users later need to configure one of those properties, it can be exposed as a
dedicated field.

Every provider block also takes `fetch_model_details`, default `true`.
When `fetch_model_details` is `false`, JP skips fetching model details from the
provider before each turn, inquiry, and title query, which saves a round trip on
slow connections or on servers without a model-details endpoint:

```toml
[providers.llm.vllm]
fetch_model_details = false
```

If a request is rejected for an unknown model, JP still fetches the model list
for the interactive model picker.

### Behaviour

Each field on `ModelDetails` is resolved from the first source that sets it:

1. the model's `models` config entry;
2. the provider's API response for the model;
3. JP's built-in table for the provider;
4. the provider's fallback default for an unknown model.

An entry for a vLLM model can declare reasoning levels while keeping the
`max_model_len` reported by `/v1/models`, and a value reported by the provider
overrides a stale built-in table entry.

An entry applies to exactly one model: the one whose served name equals its key.
There are no patterns, and an entry for a model the provider does not list adds
nothing to the listing.
It still applies when a conversation requests that model, since a request can
name a model the listing omits.

Fetching model details from the provider never fails a query.
If the lookup fails or times out after three seconds, JP logs the failure and
falls back to the built-in table and defaults for any fields not set in config.
This allows a `models` entry to supply accurate limits for a gateway without a
model-listing endpoint, such as Synthetic's Anthropic-compatible endpoint.
Pre-request checks outside the HTTP lookup still fail the query, such as a
missing credential or OpenAI's check that a ChatGPT subscription supports the
requested model.

Entries merge across config layers like `providers.llm.aliases`: by key, and
within an entry by field.
A user-global config can set a model's `context_window` while a workspace config
sets its `reasoning`.

On vLLM, declaring `reasoning` in `models` does not enable reasoning by default:
when `assistant.model.parameters.reasoning` is unset, JP omits reasoning fields
from the request so the model's chat template and the server's
`--default-chat-template-kwargs` remain in control.
The entry is consulted once reasoning is configured: to clamp an effort to the
ladder, bound a token budget, or decide how `off` is sent.

Other providers treat a configured capability the way they treat one from their
built-in table.
A model configured with `reasoning` receives that provider's default effort when
reasoning is unset, matching how Ollama and Cerebras treat models in their
built-in tables.

### Where it applies

The overlay is a pure function in `jp_llm`: given a `ModelDetails` and a
configured entry, return the details with the entry's fields applied.

`get_provider` already builds every provider from its config block.
It wraps the provider it builds in a decorator holding that block's `models`
table and `fetch_model_details`, and the decorator applies the overlay to
whatever `model_details` and `models` return.
When `fetch_model_details` is `false`, the decorator calls a new `Provider`
method instead of `model_details` that returns details from the built-in table
without making a network request; its default implementation returns empty
details, so only providers with a built-in table override it.
Every caller (the query turn, inquiries, title generation, summarization, the
unknown-model picker) sees the same details.

Individual provider implementations do not read `models` config directly.
Each request builder consumes the overlaid `ModelDetails` fields it already
reads, and this RFD does not require every builder to honour every field.
Phase 2 updates the vLLM request builder to honour `min_tokens`/`max_tokens` and
`can_disable`, which the motivating case depends on.

Generic code reaches the tables through one accessor on `LlmProviderConfig`,
keyed by `ProviderId`.
If JP later supports multiple instances of the same provider (such as two vLLM
servers), the `models` table stays inside its provider block, and only that
accessor's lookup key changes.

### Limitation: `--cfg KEY=VALUE`

A dotted `--cfg` key is split on every `.`, so it cannot address an entry whose
name contains a `.`:
`providers.llm.vllm.models.deepseek-v4.1-flash.max_output_tokens=65536` splits
the model name in two.
A config file (`-c models.toml`) and the JSON form both work:

```sh
jp q --cfg '{"providers":{"llm":{"vllm":{"models":{
  "deepseek-v4.1-flash":{"max_output_tokens":65536}}}}}}'
```

## Drawbacks

A wrong entry breaks requests that would have worked.
Declaring `prefill = true` for a model that rejects prefill turns a few extra
tokens into a request error, and the error does not point at the config.

Config wins over JP's own tables, so an entry written to work around an outdated
table keeps overriding it after a release corrects the table.

The field names become part of the config surface, and with it of the JSON
Schema JP exports.
`ModelDetails` can still change, but each rename of a mirrored field is a config
migration.

## Alternatives

**A single map at `providers.llm.models`, keyed by `provider/name`.** Places all
overrides in one field next to `aliases` instead of one per provider block.
Rejected because the `provider/name` key format differs from every other key
under `providers.llm`, and the map would need re-keying if multiple instances of
the same provider are supported; a table inside the provider block moves with
it.

**A field on the vLLM block only.** Self-hosted servers need it most, but cloud
tables lag too, and the field costs one delegating line per provider block.

**Infer details from the chat template.** `/tokenizer_info` returns the template
when the server enables it.
Rejected: it is off by default, vLLM advises against enabling it, and reading
capabilities out of a Jinja template is guesswork.

**A separate models file, as [pi's `models.json`][pi-models].** JP already
layers config files and loads extra ones with `-c`; a second format is a second
loader with no new capability.

## Non-Goals

- Adding models to a provider's listing.
  The listing reflects what the server serves; the unknown-model picker relies
  on that.
- Extra request body fields, such as a chat template kwarg JP does not send.
  That is request shaping rather than a fact about the model, and it is applied
  by each provider rather than in one place.
  A vLLM operator can set such kwargs server-wide with
  `--default-chat-template-kwargs`, but a backend behind a router may have no
  equivalent; per-model request fields would need their own design.
- Configuring one provider more than once.
  This RFD keeps the table inside the block so that work does not have to move
  it.
- Pricing and cost tracking.

## Risks and Open Questions

The `reasoning` shape is the part most likely to be wrong.
Each provider reads `ReasoningDetails` differently (Anthropic's adaptive
thinking, Google's levels, Cerebras's efforts), and the config has to express
every variant without exposing the Rust enum.

Two gaps are known.
The vLLM request builder sends an absolute effort as `thinking_token_budget`
without bounding it by `min_tokens` and `max_tokens`, and turns `off` into
`reasoning_effort: "none"` without checking `can_disable`; phase 2 fixes both.
Other providers may have gaps of their own for fields their tables never set,
and a configured value there behaves as a reported one would.

Whether a configured `context_window` below the served one should also cap
requests JP sends, or only inform truncation and compaction, is open.
This RFD only changes what JP believes about the model; anything that reads
`context_window` behaves as it would had the provider reported that value.

## Implementation Plan

1. **Config type.** Add `ModelDetailsConfig` in `jp_config`, with its partial,
   delta, fill-defaults, to-partial, and key assignment implementations, a
   `models: MergeableMap<ModelDetailsConfig>` field on each of the nine provider
   blocks (merged with `map_with_strategy`), and a `fetch_model_details` flag on
   each.
   Add the accessor on `LlmProviderConfig` and update the config schema
   snapshots.
   Changes no behaviour, so it merges on its own.
2. **Overlay.** Add the pure overlay function in `jp_llm`, the decorator applied
   in `get_provider`, the table-only `Provider` method used when
   `fetch_model_details` is `false`, and tests per field, including a vLLM
   request whose effort is clamped to a configured ladder.
   Update the vLLM request builder so the motivating case works: it sends no
   reasoning fields when reasoning is unset, even for a model with a configured
   ladder; it bounds an absolute effort by the configured `min_tokens` and
   `max_tokens`; and for `off` on a model with `can_disable = false` it sends
   the lowest level of a leveled model, and nothing otherwise.
   Each has a request-body test.
   Depends on phase 1.
3. **Documentation.** Update the provider section of the configuration guide
   with a vLLM example.
   Depends on phase 2.

## References

- [vLLM reasoning outputs][vllm-reasoning], including `--reasoning-effort-map`
  and `--default-chat-template-kwargs`.
- [vLLM security guide][vllm-security], on `/tokenizer_info`.
- [pi's model configuration][pi-models], which separates added models from
  overrides of existing ones.

[pi-models]: https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md
[vllm-reasoning]: https://docs.vllm.ai/en/latest/features/reasoning_outputs/
[vllm-security]: https://docs.vllm.ai/en/stable/usage/security/
