# RFD 125: Configured Model Details per Provider

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-27
- **Summary**: Lets each provider's config describe what its models can do,
  overriding what the provider reports and JP's built-in tables.

## Summary

Each LLM provider block gains a `models` table, keyed by the model name the
provider serves, whose entries set the facts JP keeps about a model: its context
window, output limit, reasoning support, structured output, and prefill.
A configured fact overrides what the provider reports and what JP's built-in
tables say, for every request JP makes to that model.

## Motivation

JP shapes each request around a `ModelDetails`: it clamps a reasoning effort to
the levels a model supports, converts a token effort to a level against the
output limit, decides whether to prefill, and truncates inquiry and title input
to the context window.
Those details come from two places: the provider's model listing, and tables
built into JP for providers whose listing says too little.

For a self-hosted server, neither source is enough. vLLM's `/v1/models` reports
a model's id and its served context window (`max_model_len`), and nothing else.
No vLLM endpoint reports the reasoning parser, the output limit, or whether
structured output works; the closest, `/tokenizer_info`, is off by default and
vLLM's [security guide][vllm-security] advises against enabling it.
The server swaps models without JP being rebuilt, so a built-in table cannot
keep up either.
Every model on such a server is an unknown model to JP, and each unknown fact
gets the guess that fails most cheaply.
For reasoning, that means efforts pass through unclamped, and vLLM
[documents][vllm-reasoning] that some chat templates silently fall back to
maximum reasoning for an effort they do not recognize.

Cloud providers have the same gap on a smaller scale.
A model released after the JP binary gets empty details (Cerebras logs `Unknown
Cerebras model, using empty details.`) until a JP release adds it to the table.

The only fix today is a code change.
The user who knows what the model can do, because they deployed it or read its
model card, has nowhere to say so.

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
| `context_window`    | Tokens the model accepts, prompt and output together.     |
| `max_output_tokens` | Tokens the model generates at most per response.          |
| `structured_output` | Whether the model honours a JSON schema response format.  |
| `prefill`           | Whether the model continues a trailing assistant message. |
| `reasoning`         | How the model reasons; see below.                         |

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
are left out: the first two are the provider implementation's own business, and
the last three do not change any request.
`features` in particular holds each provider's private tags
(`sampling-parameters`, `between-tools-thinking`), so exposing it would make
every rename of a tag a config migration.
When a real case shows that users need to correct one of those facts, it gets a
named field of its own, as the five above are.

Every provider block also takes `fetch_model_details`, default `true`.
Set to `false`, JP skips the request it makes before each turn, inquiry, and
title to ask the provider about the model, which saves a round trip on a slow
connection or a server without a model listing:

```toml
[providers.llm.vllm]
fetch_model_details = false
```

The listing the model picker shows after a refused request is still fetched.

### Behaviour

Each fact comes from the first of these that has it:

1. the model's entry in config;
2. what the provider reported for the model;
3. the provider's built-in table, accurate as of the JP release;
4. the provider's default for an unknown model.

So an entry for a vLLM model can add a reasoning ladder and leave the served
`max_model_len` in place, and a fact the provider reports overrides a table
written before the model changed.

An entry applies to exactly one model: the one whose served name equals its key.
There are no patterns, and an entry for a model the provider does not list adds
nothing to the listing.
It still applies when a conversation requests that model, since a request can
name a model the listing omits.

The lookup only fills in details, so it never fails the request.
One that fails, or has not answered within three seconds, is logged and reports
nothing, and the table, then the defaults, fill in what the entry leaves unset.
That makes an entry the way to run a model behind a gateway without a model
listing, such as Synthetic's Anthropic-compatible endpoint, with its real
limits.
Errors from outside the lookup request still fail: a missing credential, or
OpenAI's check that a ChatGPT subscription serves the model.

Entries merge across config layers like `providers.llm.aliases`: by key, and
within an entry by field.
A user-global config can set a model's `context_window` while a workspace config
sets its `reasoning`.

On vLLM, declaring a capability never turns reasoning on: with
`assistant.model.parameters.reasoning` unset, the request carries no reasoning
fields whatever the entry says, so the chat template and the server's
`--default-chat-template-kwargs` keep deciding.
The entry is consulted once reasoning is configured: to clamp an effort to the
ladder, bound a token budget, or decide how `off` is sent.

Other providers treat a configured capability the way they treat one from their
built-in table.
A model declared to reason gets that provider's default effort when reasoning is
unset, as Ollama and Cerebras already give to the models they know.

### Where it applies

The overlay is a pure function in `jp_llm`: given a `ModelDetails` and a
configured entry, return the details with the entry's fields applied.

`get_provider` already builds every provider from its config block.
It wraps the provider it builds in a decorator holding that block's `models`
table and `fetch_model_details`, and the decorator applies the overlay to
whatever `model_details` and `models` return.
With `fetch_model_details = false` the decorator calls a new `Provider` method
instead of `model_details`, which answers from the built-in table without a
request; its default answers with empty details, so only providers with a table
implement it.
Every caller (the query turn, inquiries, title generation, summarization, the
unknown-model picker) sees the same details.

The overlay changes no provider.
A configured fact takes effect wherever a reported one would: each request
builder reads the fields it already reads, and this RFD does not make every
builder honour every field.
The vLLM builder, which the motivating case depends on, ignores two of them
today, and phase 2 closes that gap.

Generic code reaches the tables through one accessor on `LlmProviderConfig`,
keyed by `ProviderId`.
When a provider can be configured more than once, for two vLLM servers say, the
`models` table moves with the rest of the block it sits in, and that accessor is
the one function whose lookup changes.

### Limitation: `--cfg KEY=VALUE`

A dotted `--cfg` key is split on every `.`, so it cannot address an entry whose
name contains one:
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

**One map at `providers.llm.models`, keyed by `provider/name`.** One field
instead of one per provider block, next to `aliases`.
Rejected because the key format differs from every other key under
`providers.llm`, which has to be explained, and because the map would need
re-keying once a provider can be configured more than once; a table inside the
block moves with it.

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

1. **Config type.** `ModelDetailsConfig` in `jp_config`, with its partial,
   delta, fill-defaults, to-partial, and key assignment implementations, and a
   `models: MergeableMap<ModelDetailsConfig>` field on each of the nine provider
   blocks, merged with `map_with_strategy`, and a `fetch_model_details` flag on
   each.
   Adds the accessor on `LlmProviderConfig` and updates the config schema
   snapshots.
   Changes no behaviour, so it merges on its own.
2. **Overlay.** The pure overlay function in `jp_llm`, the decorator applied in
   `get_provider`, the table-only `Provider` method used when
   `fetch_model_details` is `false`, and tests per field, including a vLLM
   request whose effort is clamped to a configured ladder.
   The vLLM request builder changes so the motivating case works: it sends no
   reasoning fields when reasoning is unset, even for a model with a configured
   ladder; it bounds an absolute effort by the configured `min_tokens` and
   `max_tokens`; and for `off` on a model with `can_disable = false` it sends
   the lowest level of a leveled model, and nothing otherwise.
   Each has a request-body test.
   Depends on phase 1.
3. **Documentation.** The provider section of the configuration guide, with a
   vLLM example.
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
