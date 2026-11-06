# RFD D24: Declared LLM Providers

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03
- **Summary**: Turns `providers.llm` into a user-named map of provider
  declarations backed by reusable protocol implementations.

## Summary

This RFD turns `providers.llm` into a map of user-declared, user-named provider
instances, each pointing at a provider source (the implementation that talks to
the server) and holding that source's settings.
Model IDs reference the declared provider name so a single source can back
multiple servers, each with its own endpoint and credentials.
New `openai-chat-completions` and `anthropic-messages` sources support servers
that speak OpenAI's Chat Completions API or Anthropic's Messages API without
vendor-specific extensions.

## Motivation

JP configures LLM providers through one fixed struct, `LlmProviderConfig`, with
one field per implementation: `anthropic`, `openai`, `vllm`, and so on.
`ProviderId`, a closed enum, names both the implementation and the configured
server, and model ids carry it as their prefix (`vllm/qwen3`).
That shape has three consequences.

**One server per implementation.** A local vLLM server and a remote one cannot
both be configured; neither can two OpenRouter accounts, or a corporate proxy
next to the vendor's own API.
Using two compatible servers in one conversation, one for the main model and one
for title generation, is impossible.
Swapping config files between invocations does not help, because it never puts
two servers in one resolved config.

**Compatible servers have no fitting implementation.** Servers without their own
JP provider (DeepSeek's API, Groq, Together, LM Studio, a LiteLLM proxy) get
pointed at the `openai` provider with a custom `base_url`.
That provider speaks OpenAI's Responses API and carries OpenAI-specific
behaviour such as ChatGPT login and encrypted reasoning, while most compatible
servers speak Chat Completions.
JP already implements the Chat Completions protocol in
`jp_llm::provider::openai_compat`, which `cerebras`, `llamacpp` and `vllm` build
on, but no provider offers it without one of those servers' extensions.
Servers with an Anthropic-compatible endpoint fare the same with `anthropic`,
which applies Claude's rules to every model not in its built-in table: adaptive
thinking, no sampling parameters, and native reasoning replay only for signed
blocks.

**Model ids name code, not deployments.** A model id is stored in every
conversation that uses it, so its prefix should identify the user's configured
deployment rather than the Rust module.
`vllm/deepseek-v4.1-flash` names the implementation; behind a router that sends
each model to a different engine, it is also wrong about the engine.
`ProviderId` also lists `deepseek` and `xai`, which have no implementation:
`get_provider` answers both with `todo!()`.

Without a change, each new compatible server needs a new `ProviderId` variant, a
config field, and an arm in every match on the enum, or must be routed through
`openai`.

## Design

### Terms

- **Provider**: a named entry under `providers.llm`, declared in config.
  It names a provider source and holds that source's settings.
  Model ids name it: `<provider>/<model>`.
- **Provider source**: the client implementation a provider uses, named by a
  fixed identifier such as `anthropic` or `vllm`.
  Every source is compiled into JP.

The two vary independently: one source backs any number of providers, and a
provider's name says nothing about its source.

### Configuration

```toml
[providers.llm.anthropic]
source = "anthropic"

[providers.llm.local]
source = "vllm"
base_url = "http://localhost:8000"

[providers.llm.gpu]
source = "vllm"
base_url = "https://gpu.example.net"
api_key_env = "GPU_API_KEY"

[providers.llm.groq]
source = "openai-chat-completions"
base_url = "https://api.groq.com/openai/v1"
api_key_env = "GROQ_API_KEY"

[assistant.model]
id = "gpu/qwen3.8-27b"

[conversation.title.generate.model]
id = "local/qwen3.8-4b"
```

A provider's settings are its source's settings: the fields of today's
`providers.llm.<source>` block, unchanged.

- Provider names use lowercase letters, digits, `-` and `_`, so a name works as
  a bare TOML key, in a `--cfg` path, and before the `/` of a model id.
  `aliases` is not a provider name; the alias map keeps that key.

- `source` is required on every provider once layers are merged.
  A config layer that overrides fields on a provider declared in an earlier
  layer may omit `source`:

  ```toml
  # user-local config, patching the workspace's `gpu` provider
  [providers.llm.gpu]
  api_key_env = "MY_GPU_KEY"
  ```

- Providers merge by name, then field by field, as today's blocks do.
  A layer that sets a different `source` than a lower layer replaces the entire
  provider entry, since settings are specific to a source.

- No name is reserved for a source, and nothing is declared unless config
  declares it.

- A conversation records each provider it has seen in full, as it records any
  setting.
  A provider added to config after the conversation started has a new key and is
  merged into the conversation's config, just like a new alias.

### Model ids

A model id is `<provider>/<model>`, split at the first `/` as today, so
`gpu/Qwen/Qwen3.8-27B-FP8` names the model `Qwen/Qwen3.8-27B-FP8` on the
provider `gpu`.
On disk the id keeps its shape, `{ "provider": "anthropic", "name":
"claude-opus-5-5" }`; the `provider` field holds a provider name instead of a
`ProviderId` value.

A model id that references an undeclared provider is a config error that names
the unknown provider and lists the declared ones.

A source decides whether a stored reasoning block is its own by comparing the
provider in the model id recorded with the event (`is_anthropic` in
`anthropic.rs` does this today).
It compares provider names, not sources: a signature belongs to one account, and
an unsigned block to one server, so two providers on the same source exchange
reasoning as text, as two vendors do today.

### Provider sources

| Source                    | Talks to                                      |
| ------------------------- | --------------------------------------------- |
| `anthropic`               | Messages API; Claude Code through ACP         |
| `anthropic-messages`      | Messages API, no extensions                   |
| `cerebras`                | Chat Completions, with Cerebras's extensions  |
| `google`                  | Gemini API                                    |
| `llamacpp`                | Chat Completions, with llama.cpp's extensions |
| `ollama`                  | Ollama's native API                           |
| `openai`                  | Responses API                                 |
| `openai-chat-completions` | Chat Completions, no extensions               |
| `openrouter`              | OpenRouter API                                |
| `vllm`                    | Chat Completions, with vLLM's extensions      |

`deepseek` and `xai` are not sources.
DeepSeek's API speaks Chat Completions, so a provider with `source =
"openai-chat-completions"` reaches it.

A source is named after the vendor or engine whose server it talks to
(`anthropic`, `vllm`).
Additional endpoints from a vendor append the endpoint's name
(`openai-chat-completions`, `anthropic-messages`).
`openai-completions` would name OpenAI's legacy Completions API at
`/v1/completions`, a different protocol that vLLM and llama.cpp still serve.

A source is a protocol module plus what is specific to its server: model
listing, credentials, and quirks.
`openai-chat-completions` exposes the Chat Completions module without vendor
extensions, and `anthropic-messages` does the same for the Messages module.
A server with quirks worth handling gets its own source on the same module, as
`vllm`, `cerebras` and `llamacpp` already have, rather than options on the
generic one.
An option on `openai-chat-completions` is right only when many servers differ
along the same axis.
The wire protocol stays inside the source and is not a setting (see
[Alternatives](#alternatives)).

### The `openai-chat-completions` source

Its settings are `base_url` (required), `api_key_env` (optional), and [RFD
125]'s `models` table.

- `base_url` is the URL the vendor documents for the OpenAI SDK, usually ending
  in `/v1`.
  JP appends `/chat/completions` and `/models`.
- With `api_key_env` set, requests carry `Authorization: Bearer <key>`.
  Without it they carry no credential, which suits a local server.
- Requests carry fields from OpenAI's Chat Completions reference, plus `top_k`
  when it is configured: setting `top_k` opts into sending it, and servers that
  do not support `top_k` typically ignore unknown fields.
- Reasoning:
  - unset: no reasoning fields, so the server's default applies;
  - `off`: `reasoning_effort: "none"`;
  - an effort: `reasoning_effort`, clamped to the model's ladder when [RFD 125]
    declares one, and `xlow` sent as `minimal`.
    A token count is converted to a level against the output limit, since Chat
    Completions has no budget field.
- Reasoning in a response is read from `reasoning_content` or `reasoning`, as
  `openai_compat` already does.
- Structured output is sent as `response_format` with a JSON schema, unless the
  model is declared `structured_output = false`, in which case it is a forced
  tool call.
- The model list comes from `GET /models`: each model's id, and its context
  window where the entry reports `context_length` (OpenRouter's and Synthetic's
  name) or `max_model_len` (vLLM's).
  If the `/models` request fails or the server has no listing endpoint, JP
  proceeds with empty reported details and resolves model details in the order
  defined by [RFD 125].
- A rejected model is classified as an unknown model, so the model picker
  applies.

Testing Synthetic's endpoint, which runs vLLM, backs each choice above: it
honours `reasoning_effort: "none"` and ignores a chat template kwarg meant to do
the same, accepts `top_k` and ignores unknown fields, follows `response_format`,
and reports `context_length` in its listing.

### The `anthropic-messages` source

Some servers offer an Anthropic-compatible endpoint, often so Claude Code can
use them.
`anthropic` is written for Anthropic's own API, and applies Claude's rules to
any model not in its built-in table; `anthropic-messages` is the Messages
protocol with neutral defaults.
Both run on one Messages module, extracted from today's `anthropic` provider.

Its settings are `base_url` (required, without `/v1`, as `anthropic` takes it),
`api_key_env`, and the `models` table.

|                                                        | `anthropic`                                           | `anthropic-messages`                                                                          |
| ------------------------------------------------------ | ----------------------------------------------------- | --------------------------------------------------------------------------------------------- |
| Model details                                          | `/v1/models/{id}`, then the built-in table            | `/v1/models` when the server lists models, then [RFD 125] entries                         |
| Reasoning configured, support unknown                  | adaptive thinking                                     | `thinking: enabled` with a budget                                                             |
| Reasoning with a declared mode                         | per the table                                         | per the entry: `adaptive` with an effort, or `enabled` with a budget                          |
| Reasoning `off`                                        | per the table: `disabled`, `between_tools`, omitted   | `thinking: disabled`                                                                          |
| Sampling parameters                                    | only for models marked in the built-in table          | whenever configured                                                                           |
| Replayed reasoning                                     | thinking blocks when signed, otherwise `<think>` text | thinking blocks, signed or not                                                                |
| Structured output                                      | `output_config.format`                                | `output_config.format` when declared `structured_output = true`, otherwise a forced tool call |
| Beta headers, service tier, fast mode, context editing | yes                                                   | no                                                                                            |
| `cache_control`                                        | per `assistant.request.cache`                         | the same                                                                                      |
| Output limit when none is configured or reported       | 32,000 tokens                                         | the same                                                                                      |
| Credentials                                            | the `auth` chain: API keys and subscriptions          | `api_key_env` only                                                                            |

The neutral defaults come from testing Synthetic's endpoint, which serves other
vendors' models.
It has no `/v1/models` endpoint, ignores `output_config.format` and answers in
prose, returns unsigned thinking blocks and accepts them back, accepts sampling
parameters, and accepts a forced tool call with thinking on.
Anthropic's own API rejects a thinking block without a valid signature, which is
why `anthropic` replays unsigned reasoning as text.

### Credentials

Credential settings stay where they are: each source keeps the `auth`,
`api_key_env` and other fields its block has today.
`--auth` writes the chain onto the provider the turn's model names.
A provider whose source has no `auth` chain rejects `--auth`, as `llamacpp` and
`ollama` do today.

The credential store keys stored logins by source, so providers that share a
source share logins: two `anthropic` providers see the same Claude subscription
profiles.
Source names match today's provider names, so stored credentials need no
migration.
`jp provider llm auth login|list|logout` accept a declared provider name or a
source name.

### `jp init`

`jp init` writes a declaration for the provider it offers, next to the model it
already writes:

```toml
[providers.llm.anthropic]
source = "anthropic"

[assistant.model.id]
provider = "anthropic"
name = "claude-sonnet-5-5"
```

Its provider prompt lists sources.

### Migration

Two places hold configuration in the old shape: config files, and the config
stored with each conversation.

**Stored conversations.** `jp_conversation::compat::deserialize_partial_config`
gains a repair next to `migrate_legacy_rule_bounds`: each `providers.llm` entry
without `source` whose name is a source name gets `source` set to that name, and
the `deepseek` block is dropped.
Today's `to_partial` records all nine blocks, so every stored conversation ends
up declaring every provider it could have used, and its model ids keep resolving
without being rewritten.

**Config files.** For one release, a provider block without `source` whose key
matches a known source name defaults `source` to that key, and a model id
referencing an undeclared provider whose name matches a known source resolves to
that source's default configuration.
Both print a warning showing the declaration to add.
The release after makes both errors, with the same declaration in the message.

**Older binaries.** A binary built before this RFD rejects a config that sets
`source` or declares a provider under a new name, because its config types
reject unknown keys.
Phase 1 below narrows that window: a release that accepts `source` in today's
blocks reads any config that declares providers under today's names.

## Drawbacks

- Every config needs declarations.
  `jp init` writes them and the transition release warns, but hand-written
  configs and the documentation's examples grow by two lines per provider.
- `aliases` shares a table with provider names, so it is the one name a provider
  cannot have.
- A patch that misspells a provider name creates a provider with no `source`.
  That surfaces as a missing-`source` error after merging, which has to name the
  provider for the misspelling to be findable.
- `openai-chat-completions` takes a `base_url` ending in `/v1`, as vendors
  document it; `vllm`, `llamacpp` and both Messages sources take one without.
  Two conventions sit in one config.
- Two sources share the Messages module, so a fix to how Claude is called has to
  land in the module or in `anthropic` deliberately, not by habit.
- Code that matches on `ProviderId` becomes code that matches on the source.
  Most of it is mechanical, but `ProviderId` appears on more than 460 lines
  across the workspace.

## Alternatives

**Named endpoints in a sub-table** (`providers.llm.endpoints.<name>.kind =
"vllm"`).
This approach is additive and smaller to build, but it configures backends in
two places, keeps today's provider blocks as special cases, and does not move
toward pluggable provider sources.

**Built-in providers with reserved names** (Codex's `model_providers`).
Requires no initial configuration, but ties each source name to a single default
provider instance; adding a second Anthropic endpoint requires a different
configuration style than the first, and overriding a built-in provider requires
special-case merging.

**Model-centric configuration** (LiteLLM, Continue).
Each model entry carries its endpoint and credential.
That suits a routing proxy, but one server serving several models repeats its
settings for each, and [RFD 125] plus aliases already cover per-model facts and
names.

**A wire-protocol setting** (pi's per-model `api`).
It lets one provider serve different models over different protocols, as
gateways such as Azure or GitHub Copilot do.
JP has no such source today.
Model ids do not name the protocol, so an optional `api` on an [RFD 125] model
entry can be added later without a migration.

**Pointing compatible servers at `openai` or `anthropic`.** Today's workaround.
`openai` sends Responses API requests, which most compatible servers do not
accept.
`anthropic` works against a server without a model route now that the lookup is
advisory, but drops sampling parameters and replays reasoning as text for every
model not in its built-in table, and sends Claude-only beta headers.
Both allow only one such server.

**`type` instead of `source`.** `source` matches
`conversation.tools.<name>.source`, which names what serves a tool, and still
reads correctly once a source can come from a plugin.

## Non-Goals

- Falling back from one provider to another.
- Recording which provider served a response.
- Loading provider sources from plugins.
  Names and dispatch leave room for it; every source here is compiled in.
- Changing credentials.
  Splitting `anthropic` and `openai` by credential kind, one credential per
  provider, and what happens to `--auth` belong to a separate RFD.
- Request quirks of particular servers beyond what each source already handles,
  such as SGLang's reasoning budget (ticket T-19mv7az).
- Parameters on aliases.

## Risks and Open Questions

**Patching a provider without `source`.** Config types in `jp_config` are
partials merged layer by layer.
A provider is naturally an enum tagged by `source`, and `McpProviderConfig`
shows the cost: its partial needs `type` in every layer that modifies it.
Letting a layer omit `source` requires either deserializing a provider's
settings into the source-specific struct after merging, or using a flat partial
struct that holds every source's fields.
Phase 3 prototypes this first.
If neither works, the fallback is requiring `source` in every layer that touches
a provider.

**Error locations.** Deserializing settings after merging can lose the source
file location of an invalid value.
Validation errors must preserve the file path so misspelled fields are still
attributed to the file that set them.

**`base_url` for the new source.** Accepting the `/v1`-suffixed URL that vendors
document matches what users copy from provider docs; omitting `/v1` matches JP's
other provider blocks.
This draft chooses the vendor-documented URL.

## Implementation Plan

1. **Accept `source` in today's blocks.** Each block takes an optional `source`
   that must equal its name.
   Nothing else changes.
   Ships as its own release, so configs that declare providers under today's
   names load in it and in every later release.
   Merges alone.

2. **Split `ProviderId`.** `SourceId` is a closed enum of today's providers;
   `ProviderName` is an open string newtype with the name rules above.
   `ModelIdConfig::provider` becomes a `ProviderName`, `get_provider` looks the
   provider up and dispatches on its source, the turn loop's provider cache and
   the inquiry and title paths key on `ProviderName`, the checks that decide
   whether stored reasoning is a provider's own compare `ProviderName`s, and the
   credential store keys on `SourceId`.
   Names still come from today's fixed blocks, so behaviour is unchanged.
   Merges alone.

3. **The open map.** `LlmProviderConfig` becomes `aliases` plus a map of
   declared providers, with the merge rules above, per-name `to_partial`, the
   repair for stored conversations, the transition reading of old blocks, `jp
   init` writing declarations, and updated schema snapshots.
   Removes `deepseek` and `xai`.
   Starts with the prototype from [Risks](#risks-and-open-questions).
   Depends on phase 2.

4. **The `openai-chat-completions` source.** Implement the source on top of
   `openai_compat`, with request-body tests for each reasoning setting, `top_k`,
   and both forms of structured output, plus VCR cassette tests.
   Depends on phase 3.

5. **The `anthropic-messages` source.** Extract the shared Messages protocol
   module from `anthropic.rs` without changing `anthropic`'s request output
   (verified by existing VCR tests).
   Then implement `anthropic-messages` on that module, with request-body tests
   for each row of the comparison table above and VCR cassette tests.
   Depends on phase 3.

6. **Documentation.** Update the provider section of the configuration guide,
   and add glossary entries for Provider and Provider Source (the current
   Provider entry describes a vendor integration and still lists `deepseek`).
   Depends on phase 3.

7. **End of the transition.** Blocks without `source` and undeclared source
   names become errors.
   One release after phase 3.

[RFD 125] lands in either order.
Its `models` table belongs to each source's settings and moves into provider
entries with them, and its accessor is keyed by `ProviderName` instead of
`ProviderId`.

## References

- [RFD 125]: Configured Model Details per Provider.
- [RFD 090]: Anthropic Subscription Auth with Credential Fallback, the `auth`
  chain this RFD leaves in place.
- [RFD 110]: Anthropic Subscription Queries via ACP.
- [RFD 079]: Config Sources and Load Order.
  A config source there is a layer of configuration, a different concept from a
  provider source.
- [pi's provider and API split][pi-ai].

[RFD 079]: ../079-config-sources-and-load-order.md
[RFD 090]: ../090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 110]: ../110-anthropic-subscription-queries-via-acp.md
[RFD 125]: ../125-configured-model-details-per-provider.md
[pi-ai]: https://github.com/earendil-works/pi/blob/main/packages/ai/README.md
