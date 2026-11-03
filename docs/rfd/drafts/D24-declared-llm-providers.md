# RFD D24: Declared LLM Providers

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03

## Summary

`providers.llm` becomes a map of providers the user declares and names.
Each declaration points at a provider source, the code that talks to the server,
and holds that source's settings.
Model ids name a declared provider, so one source can back several providers,
each with its own endpoint and credentials.
A new `openai-chat-completions` source covers servers that speak OpenAI's Chat
Completions API without extensions.

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

**Model ids name code, not deployments.** A model id is stored in every
conversation that uses it, so its prefix should name something the user chose.
`vllm/deepseek-v4.1-flash` names the implementation; behind a router that sends
each model to a different engine, it is also wrong about the engine.
`ProviderId` also lists `deepseek` and `xai`, which have no implementation:
`get_provider` answers both with `todo!()`.

Without a change, each new compatible server needs a new `ProviderId` variant, a
config field, and an arm in every match on the enum, or it gets bent through
`openai`.

## Design

### Terms

- **Provider**: a named entry under `providers.llm`, declared in config.
  It names a provider source and holds that source's settings.
  Model ids name it: `<provider>/<model>`.
- **Provider source**: the code a provider runs on, named by a fixed identifier
  such as `anthropic` or `vllm`.
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
  A layer that only changes a provider another layer declares may leave it out:

  ```toml
  # user-local config, patching the workspace's `gpu` provider
  [providers.llm.gpu]
  api_key_env = "MY_GPU_KEY"
  ```

- Providers merge by name, then field by field, as today's blocks do.
  A layer that sets a different `source` than the layer below replaces the
  provider whole, because one source's settings mean nothing to another.

- No name is reserved for a source, and nothing is declared unless config
  declares it.

- A conversation records each provider it has seen in full, as it records any
  setting.
  A provider declared after the conversation started is a new name and reaches
  it, the same way a new alias does.

### Model ids

A model id is `<provider>/<model>`, split at the first `/` as today, so
`gpu/Qwen/Qwen3.8-27B-FP8` names the model `Qwen/Qwen3.8-27B-FP8` on the
provider `gpu`.
On disk the id keeps its shape, `{ "provider": "anthropic", "name":
"claude-opus-5-5" }`; the `provider` field holds a provider name instead of a
`ProviderId` value.

An id naming a provider that nothing declares is a config error that names the
provider and lists the declared ones.

### Provider sources

| Source                    | Talks to                                      |
| ------------------------- | --------------------------------------------- |
| `anthropic`               | Messages API; Claude Code through ACP         |
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
A vendor's further endpoints add the vendor's own name for the endpoint
(`openai-chat-completions`).
`openai-completions` would name OpenAI's legacy Completions API at
`/v1/completions`, a different protocol that vLLM and llama.cpp still serve.

A source is a protocol module plus what is specific to its server: model
listing, credentials, and quirks.
`openai-chat-completions` is the Chat Completions module with nothing added.
A server with quirks worth handling gets its own source on the same module, as
`vllm`, `cerebras` and `llamacpp` already have, rather than options on the
generic one.
An option on `openai-chat-completions` is right only when many servers differ
along the same axis.
The wire protocol stays inside the source and is not a setting (see
[Alternatives](#alternatives)).

### The `openai-chat-completions` source

Its settings are `base_url` (required) and `api_key_env` (optional).

- `base_url` is the URL the vendor documents for the OpenAI SDK, usually ending
  in `/v1`.
  JP appends `/chat/completions` and `/models`.
- With `api_key_env` set, requests carry `Authorization: Bearer <key>`.
  Without it they carry no credential, which suits a local server.
- Requests carry only fields from OpenAI's Chat Completions reference.
  Reasoning is sent as `reasoning_effort`, and reasoning in a response is read
  from `reasoning_content` or `reasoning`, as `openai_compat` already does.
- The model list comes from `GET /models` and reports each model's id and
  nothing else.
  Facts the listing lacks come from [RFD 125]'s `models` table.
- A rejected model is classified as an unknown model, so the model picker
  applies.

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

**Config files.** For one release, a block without `source` whose name is a
source name is read as that source, and a model id naming an undeclared provider
whose name is a source name resolves to that source with its defaults.
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
- The new source takes a `base_url` ending in `/v1`, as vendors document it;
  `vllm` and `llamacpp` take one without.
  Two conventions sit in one config.
- Code that matches on `ProviderId` becomes code that matches on the source.
  Most of it is mechanical, but `ProviderId` appears on more than 460 lines
  across the workspace.

## Alternatives

**Named endpoints in a sub-table** (`providers.llm.endpoints.<name>.kind =
"vllm"`).
Additive and cheaper to build, but backends would be configured in two places,
today's blocks would stay special, and nothing moves toward sources that are not
compiled in.

**Built-in providers with reserved names** (Codex's `model_providers`).
Works with no configuration, but each source's name stays tied to one provider,
so a second Anthropic endpoint needs a name the first does not, and replacing a
built-in means working around it.

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

**Pointing compatible servers at `openai`.** Today's workaround.
It sends Responses API requests, which most compatible servers do not accept,
and allows only one such server.

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
shows the cost: its partial needs `type` in every layer that writes it.
Letting a layer omit `source` means typing a provider's settings after merging,
or a partial that holds every source's fields.
Phase 3 prototypes this first.
If neither works, the fallback is requiring `source` in every layer that writes
a provider.

**Error locations.** Typing settings after merging can lose which file a bad
value came from.
The error has to name the file, or misspelled fields get worse errors than they
do today.

**`base_url` for the new source.** Taking the URL vendors document matches what
users copy; leaving out `/v1` matches the rest of JP's config.
This draft picks the first.

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
   the inquiry and title paths key on `ProviderName`, and the credential store
   keys on `SourceId`.
   Names still come from today's fixed blocks, so behaviour is unchanged.
   Merges alone.

3. **The open map.** `LlmProviderConfig` becomes `aliases` plus a map of
   declared providers, with the merge rules above, per-name `to_partial`, the
   repair for stored conversations, the transition reading of old blocks, `jp
   init` writing declarations, and updated schema snapshots.
   Removes `deepseek` and `xai`.
   Starts with the prototype from [Risks](#risks-and-open-questions).
   Depends on phase 2.

4. **The `openai-chat-completions` source**, built on `openai_compat`, with
   request-body tests against a recorded server.
   Depends on phase 3.

5. **Documentation.** The provider section of the configuration guide, and
   glossary entries for Provider and Provider Source.
   The current Provider entry describes a vendor integration and lists
   `deepseek`.
   Depends on phase 3.

6. **End of the transition.** Blocks without `source` and undeclared source
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
