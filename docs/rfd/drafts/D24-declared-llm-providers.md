# RFD D24: Declared LLM Providers

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-03
- **Summary**: Turns `providers.llm` into a user-named map of provider
  declarations backed by reusable protocol implementations.

## Summary

This RFD turns `providers.llm` into a map of user-named provider declarations.
Each entry selects a provider source (the client implementation that talks to
the server) and holds that source's settings.
Model IDs reference the declared provider name, so a single source can back
multiple servers with distinct endpoints and credentials.
New `openai-chat-completions` and `anthropic-messages` sources support servers
that implement OpenAI's Chat Completions API or Anthropic's Messages API without
vendor-specific extensions.

## Motivation

JP configures LLM providers through a fixed struct, `LlmProviderConfig`, with
one field per implementation (`anthropic`, `openai`, `vllm`, and so on).
`ProviderId` is a closed enum that identifies both the client implementation and
the configured server, and model IDs use it as their prefix (`vllm/qwen3`).
Coupling the implementation to the server config causes three problems.

**One server per implementation.** A user cannot configure both a local and a
remote vLLM server, two OpenRouter accounts, or a corporate proxy alongside a
vendor's direct API.
A single conversation cannot use one compatible server for the main model and
another for title generation, because swapping config files between invocations
never puts both servers into the same resolved config.

**No neutral implementation for compatible servers.** Users must point servers
without a dedicated JP provider (DeepSeek's API, Groq, Together, LM Studio, a
LiteLLM proxy) at the `openai` provider with a custom `base_url`.
That provider speaks OpenAI's Responses API and carries OpenAI-specific behavior
such as ChatGPT login and encrypted reasoning, whereas most compatible servers
speak Chat Completions.
JP already implements the Chat Completions protocol in
`jp_llm::provider::openai_compat` as the shared base for `cerebras`, `llamacpp`,
and `vllm`, but no provider exposes it without vendor-specific extensions.
Anthropic-compatible endpoints have the same problem with `anthropic`, which
applies Claude-specific rules to any model outside its built-in table: adaptive
thinking, omitted sampling parameters, and native reasoning replay restricted to
signed blocks.

**Model IDs name code, not deployments.** Every conversation stores the model ID
it uses, so the prefix should identify the configured deployment rather than the
Rust module.
`vllm/deepseek-v4.1-flash` names the client implementation, which is also
inaccurate behind a router that dispatches models to different engines.
Meanwhile, `ProviderId` includes unimplemented `deepseek` and `xai` variants
that panic with `todo!()` in `get_provider`.

Under this design, supporting a new compatible server requires either routing it
through `openai` or adding a `ProviderId` variant, a config field, and a match
arm at every dispatch site.

## Design

### Terms

- **Provider**: A user-named entry declared under `providers.llm`.
  It selects a provider source, holds that source's settings, and serves as the
  prefix in model IDs (`<provider>/<model>`).
- **Provider source**: A compiled-in client implementation, identified by a
  fixed name such as `anthropic` or `vllm`.

Providers and sources vary independently: a single source can back any number of
providers, and a provider's name does not have to match its source.

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

Each provider entry takes its source's settings: the existing fields of
`providers.llm.<source>`, unchanged.

- Provider names may contain lowercase ASCII letters, digits, `-`, and `_`,
  making them valid bare TOML keys, `--cfg` path segments, and
  `<provider>/<model>` prefixes.
  The key `aliases` remains reserved for the model alias map.

- Every resolved provider requires a `source` after config layers are merged.
  A layer that overrides fields on a provider declared in an earlier layer may
  omit `source`:

  ```toml
  # user-local config, patching the workspace's `gpu` provider
  [providers.llm.gpu]
  api_key_env = "MY_GPU_KEY"
  ```

- Providers with the same name merge field by field across layers.
  Because settings are specific to a source, a layer that changes a provider's
  `source` replaces the entire entry instead of merging fields from the earlier
  layer.

- No provider exists unless declared in config, and source names are not
  reserved as default providers.

- A conversation snapshots each configured provider in full.
  Providers added to config after a conversation starts have new keys and merge
  into the conversation's config alongside existing entries, just like new model
  aliases.

### Model IDs

A model ID has the form `<provider>/<model>`, split at the first `/`:
`gpu/Qwen/Qwen3.8-27B-FP8` selects model `Qwen/Qwen3.8-27B-FP8` on the `gpu`
provider.
The serialized representation is unchanged (`{ "provider": "anthropic", "name":
"claude-opus-5-5" }`), except `provider` holds a declared provider name instead
of a `ProviderId` variant.

A model ID that references an undeclared provider produces a config error naming
the unknown provider and listing the declared ones.

To decide whether to replay a stored reasoning block natively, a source compares
the current provider name against the provider recorded in the event's model ID
(matching `is_anthropic` in `anthropic.rs`).
This check compares provider names rather than sources: cryptographic signatures
are tied to a single account and unsigned reasoning blocks to a single server,
so two providers backed by the same source exchange reasoning as plain text,
just as providers from different vendors do.

### Provider Sources

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
DeepSeek's API speaks Chat Completions and is served by `source =
"openai-chat-completions"`.

Each source is named after the vendor or engine it targets (`anthropic`,
`vllm`).
When a vendor defines multiple protocols, additional sources append the protocol
name (`openai-chat-completions`, `anthropic-messages`).
The full name `openai-chat-completions` avoids ambiguity with
`openai-completions`, which would refer to OpenAI's legacy `/v1/completions`
endpoint that vLLM and llama.cpp still serve.

A source combines a wire-protocol module with server-specific model listing,
credentials, and request quirks.
`openai-chat-completions` and `anthropic-messages` expose the shared Chat
Completions and Messages protocol modules without vendor extensions.
A server with distinct quirks gets its own source on top of the shared protocol
module (like `vllm`, `cerebras`, and `llamacpp`) rather than configuration flags
on the generic source; a flag on `openai-chat-completions` is warranted only
when many servers vary along the same axis.
The wire protocol remains fixed per source rather than configurable per provider
(see [Alternatives](#alternatives)).

### The `openai-chat-completions` Source

`openai-chat-completions` accepts `base_url` (required), `api_key_env`
(optional), and the `models` table from [RFD 125].

- `base_url` is the base URL the vendor documents for the OpenAI SDK (usually
  ending in `/v1`), to which JP appends `/chat/completions` and `/models`.
- When `api_key_env` is set, requests include an `Authorization: Bearer <key>`
  header; when unset, requests are sent unauthenticated for local servers.
- Requests include standard fields from OpenAI's Chat Completions specification,
  plus `top_k` when explicitly configured.
  Setting `top_k` opts into sending it, and servers that do not support `top_k`
  typically ignore unknown fields.
- Reasoning:
  - unset: omits reasoning fields so the server's default applies;
  - `off`: sends `reasoning_effort: "none"`;
  - effort level: sends `reasoning_effort`, clamped to the model's supported
    levels when declared via [RFD 125], with `xlow` mapped to `minimal`.
    Because Chat Completions has no token-budget field, a token-count reasoning
    config is converted to an effort level relative to the output token limit.
- Response reasoning is read from `reasoning_content` or `reasoning`, matching
  `openai_compat`.
- Structured output uses `response_format` with a JSON schema, falling back to a
  forced tool call when the model is configured with `structured_output =
  false`.
- `GET /models` populates the model list with each model's ID and its context
  window from `context_length` (used by OpenRouter and Synthetic) or
  `max_model_len` (used by vLLM).
  If `GET /models` fails or the server does not implement it, JP falls back to
  empty server-reported details and resolves model metadata in the order defined
  by [RFD 125].
- A server error rejecting the requested model is classified as an unknown-model
  error, triggering the interactive model picker.

These choices were verified against Synthetic's vLLM-based endpoint, which
honors `reasoning_effort: "none"` (while ignoring the equivalent chat-template
kwarg), accepts `top_k`, ignores unknown fields, supports `response_format`, and
reports `context_length` in `/models`.

### The `anthropic-messages` Source

Several third-party servers expose an Anthropic-compatible Messages endpoint,
often to support Claude Code.
While `anthropic` targets Anthropic's own API and applies Claude-specific rules
to any model outside its built-in table, `anthropic-messages` speaks the
Messages protocol with vendor-neutral defaults.
Both sources share a common Messages protocol module extracted from the
`anthropic` provider.

`anthropic-messages` accepts `base_url` (required, without a `/v1` suffix,
matching `anthropic`), `api_key_env`, and the `models` table from [RFD 125].

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

These defaults reflect testing against Synthetic's Anthropic-compatible
endpoint, which serves non-Claude models: it has no `/v1/models` route, ignores
`output_config.format` and replies in prose, returns and accepts unsigned
thinking blocks, honors sampling parameters, and allows forced tool calls
alongside thinking.
By contrast, Anthropic's API rejects thinking blocks that lack a valid
signature, so `anthropic` must replay unsigned reasoning as `<think>` text.

### Credentials

Each source retains the credential fields (`auth`, `api_key_env`, and related
settings) from its existing config block.
The `--auth` flag applies its credential chain to the provider selected by the
turn's model ID; if that provider's source has no `auth` field, `--auth` is
rejected, as with `llamacpp` and `ollama`.

The credential store keys logins by source ID rather than provider name, so
providers backed by the same source share stored logins (for example, two
`anthropic` providers share Claude subscription profiles).
Because source names match existing provider names, stored credentials require
no migration.
`jp provider llm auth login|list|logout` accepts either a declared provider name
or a source name.

### `jp init`

`jp init` prompts the user to choose a provider source and writes both the
provider declaration and the selected model to the generated config:

```toml
[providers.llm.anthropic]
source = "anthropic"

[assistant.model.id]
provider = "anthropic"
name = "claude-sonnet-5-5"
```

### Migration

Legacy provider configs exist in two places: user and workspace config files,
and persisted conversation configs.

**Stored conversations.** `jp_conversation::compat::deserialize_partial_config`
adds a migration pass alongside `migrate_legacy_rule_bounds`: any
`providers.llm` entry without `source` whose key matches a known source name
receives `source` set to that key, and the `deepseek` block is dropped.
Because `to_partial` previously serialized all nine provider blocks into every
conversation, migrated conversations automatically declare every provider their
events might reference, and stored model IDs continue to resolve without
modification.

**Config files.** For one transition release, a `providers.llm.<key>` block
without `source` whose key matches a known source name defaults `source` to that
key, and a model ID referencing an undeclared provider whose name matches a
known source resolves against that source's default configuration.
Both fallback paths emit a warning showing the explicit TOML declaration to add.
The following release turns both into hard errors that include the same
declaration snippet.

**Older binaries.** Binaries built before this RFD reject unknown config keys,
so they fail on `source` fields or custom provider names.
Phase 1 below narrows that compatibility window: shipping an intermediate
release that accepts an optional `source` field on the existing fixed provider
blocks lets configs that use standard provider names load on both versions.

## Drawbacks

- Every configured provider requires an explicit declaration.
  Although `jp init` generates the default declaration and the transition
  release prints migration warnings, hand-written configs and documentation
  examples grow by two lines per provider.
- Because `providers.llm.aliases` lives in the same TOML table as declared
  providers, `aliases` cannot be used as a provider name.
- A config overlay that misspells a provider name creates a new partial entry
  with no `source` instead of patching the intended provider.
  The resulting missing-`source` error after merging must name the provider key
  so the typo is easy to spot.
- `openai-chat-completions` expects a `base_url` ending in `/v1` (matching
  vendor SDK documentation), whereas `vllm`, `llamacpp`, and both Messages
  sources omit `/v1`.
  Both URL conventions coexist in the same config.
- Sharing the Messages protocol module between `anthropic` and
  `anthropic-messages` means changes to request formatting require choosing
  whether a behavior belongs to the shared module or only to `anthropic`.
- Call sites that match on `ProviderId` must be updated to match on the source
  or key by provider name.
  The refactoring is mostly mechanical, but `ProviderId` appears on more than
  460 lines across the workspace.

## Alternatives

**Named endpoints in a sub-table** (`providers.llm.endpoints.<name>.kind =
"vllm"`).
An additive sub-table is smaller to implement, but it splits backend
configuration across two locations, keeps the existing provider blocks as
special cases, and does not move toward pluggable provider sources.

**Built-in providers with reserved names** (Codex's `model_providers`).
Implicitly declaring one default provider per source avoids initial boilerplate,
but it ties each source name to a single instance, forces a second endpoint on
the same source to use a different configuration pattern, and requires
special-case merging to override built-in entries.

**Model-centric configuration** (LiteLLM, Continue).
Attaching the endpoint and credentials to each model entry fits a routing proxy,
but duplicates connection settings whenever one server hosts multiple models.
[RFD 125] and model aliases already cover per-model metadata and shorthand
names.

**A wire-protocol setting** (pi's per-model `api`).
A per-model protocol field lets one provider serve models over different wire
protocols, as multi-protocol gateways like Azure and GitHub Copilot do.
JP has no such source.
Because model IDs do not encode the wire protocol, an optional `api` field can
be added to [RFD 125] model entries later without a migration.

**Pointing compatible servers at `openai` or `anthropic`.** The status quo
workaround.
`openai` speaks the Responses API, which most OpenAI-compatible servers do not
implement.
`anthropic` tolerates servers without a `/v1/models/{id}` route now that model
lookup is advisory, but it strips sampling parameters, replays unsigned
reasoning as plain text for any model outside its built-in table, and sends
Claude-specific beta headers.
Neither allows configuring more than one compatible server.

**`type` instead of `source`.** `source` matches
`conversation.tools.<name>.source`, which identifies the implementation backing
a tool, and continues to fit when provider sources can be loaded from plugins.

## Non-Goals

- Automatic fallback from one provider to another.
- Recording which provider served a response.
- Loading provider sources from plugins.
  The naming and dispatch design leaves room for plugin-backed sources, but all
  sources in this RFD are compiled in.
- Redesigning credential configuration.
  Splitting `anthropic` and `openai` by credential kind, binding a single
  credential per provider, and reworking `--auth` belong to a separate RFD.
- Server-specific request quirks beyond those already handled by each source,
  such as SGLang's reasoning budget (ticket T-19mv7az).
- Default model parameters on aliases.

## Risks and Open Questions

**Patching a provider without `source`.** `jp_config` merges partial config
structs layer by layer.
Representing a provider as a `source`-tagged enum has the limitation seen in
`McpProviderConfig`, where every layer that modifies an entry must repeat the
tag (`type`).
Allowing an override layer to omit `source` requires either deferring
source-specific deserialization until after layers are merged or using a flat
partial struct containing the union of all sources' fields.
Phase 3 prototypes this first; if neither approach works, every layer that
modifies a provider will have to include `source`.

**Error locations.** Deferring source-specific deserialization until after
merging risks losing the originating file location of an invalid setting.
Validation errors must preserve per-field file paths so misspelled or invalid
keys are still attributed to the config file that set them.

**`base_url` for the new source.** Accepting the `/v1`-suffixed URL that vendors
document matches what users copy from provider docs; omitting `/v1` matches JP's
other provider blocks.
This draft chooses the vendor-documented URL.

## Implementation Plan

1. **Accept `source` on existing provider blocks.** Each fixed provider block
   accepts an optional `source` field that must match the block's name, with no
   other behavior changes.
   Shipping this as its own release lets configs that declare providers under
   existing source names load in both this release and later releases.
   Merges independently.

2. **Split `ProviderId`.** Introduce `SourceId` (a closed enum of built-in
   sources) and `ProviderName` (a validated string newtype).
   Change `ModelIdConfig::provider` to `ProviderName`; update `get_provider` to
   look up the declared provider and dispatch on its `SourceId`; key the turn
   loop's provider cache, inquiry path, title generator, and reasoning-ownership
   checks by `ProviderName`; and key the credential store by `SourceId`.
   Provider names still come from the fixed config blocks, so user-visible
   behavior is unchanged.
   Merges independently.

3. **Replace fixed fields with an open map.** Replace the fixed provider fields
   on `LlmProviderConfig` with `aliases` plus a map of declared providers,
   implementing layer merging, per-name `to_partial`, conversation config
   migration, transition fallback resolution for legacy blocks, `jp init`
   provider declarations, and updated schema snapshots.
   Remove `deepseek` and `xai`.
   Begins with the partial-merging prototype from
   [Risks](#risks-and-open-questions).
   Depends on Phase 2.

4. **Implement `openai-chat-completions`.** Build the source on top of
   `openai_compat`, with request-body unit tests for each reasoning mode,
   `top_k`, and both structured-output modes, plus VCR cassette tests.
   Depends on Phase 3.

5. **Implement `anthropic-messages`.** Extract the shared Messages protocol
   module from `anthropic.rs` without changing `anthropic`'s request output
   (verified by existing VCR tests).
   Implement `anthropic-messages` on that module, with request-body unit tests
   for each row of the comparison table above and VCR cassette tests.
   Depends on Phase 3.

6. **Update documentation.** Update the provider section of the configuration
   guide and add glossary entries for Provider and Provider Source (replacing
   the current Provider entry, which describes a fixed vendor integration and
   still lists `deepseek`).
   Depends on Phase 3.

7. **Remove transition fallbacks.** Reject provider blocks without `source` and
   model IDs referencing undeclared source names.
   Ships one release after Phase 3.

[RFD 125] can land before or after this RFD.
Its `models` table belongs to each source's settings and moves into declared
provider entries alongside the other source fields, and its lookup accessor keys
on `ProviderName` instead of `ProviderId`.

## References

- [RFD 125]: Configured Model Details per Provider.
- [RFD 090]: Anthropic Subscription Auth with Credential Fallback, which defines
  the `auth` chain preserved by this RFD.
- [RFD 110]: Anthropic Subscription Queries via ACP.
- [RFD 079]: Config Sources and Load Order.
  A config source in [RFD 079] is a configuration layer, distinct from a
  provider source.
- [pi's provider and API split][pi-ai].

[RFD 079]: ../079-config-sources-and-load-order.md
[RFD 090]: ../090-anthropic-subscription-auth-with-credential-fallback.md
[RFD 110]: ../110-anthropic-subscription-queries-via-acp.md
[RFD 125]: ../125-configured-model-details-per-provider.md
[pi-ai]: https://github.com/earendil-works/pi/blob/main/packages/ai/README.md
