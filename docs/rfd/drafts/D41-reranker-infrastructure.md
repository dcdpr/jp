# RFD D41: Reranker Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22
- **Summary**: Introduces a cross-encoder reranker `Provider` trait with Voyage
  AI HTTP and local `fastembed`-backed `ember` implementations.

## Summary

This RFD introduces a reranker primitive: a `Provider` trait that scores
candidates by relevance to a query, with implementations for Voyage AI's HTTP
reranking API and a local `ember` server backed by `fastembed`.
The trait gives downstream features low-latency relevance scoring without the
cost of full LLM inference.

## Motivation

JP has no general-purpose cross-encoder reranker primitive.
Lexical ranking exists in a contrib crate, but it is specific to Bear notes and
not exposed as a general API.
Several near-term features need general-purpose relevance scoring:

- **Instruction reminders** score project instructions against the assistant's
  last reply and surface the most relevant ones.
- **Knowledge base retrieval** selects which subjects to surface for a query.
- **Quality signals** such as sentiment analysis and turn classification rely on
  adjacent scoring primitives.

Using an LLM for relevance scoring is too slow and expensive.
A Haiku-class chat completion takes 500-2000ms, costs a few cents per call,
requires constrained decoding, and uses a model trained for open-ended
generation rather than relevance ranking.
A task-specific cross-encoder reranker such as `BAAI/bge-reranker-v2-m3` runs
locally in ~50ms at zero per-call cost once installed.

This RFD defines the reranker primitive so downstream consumers (instruction
reminders, knowledge base retrieval) can build on it in separate RFDs.

## Design

### Configuration

Reranker providers are configured under `providers.rerank.*`, matching
`providers.llm.*`:

```toml
[providers.rerank.voyage]
api_key_env = "VOYAGE_API_KEY"
# base_url = "https://api.voyageai.com/v1"  # override for tests

[providers.rerank.ember]
# base_url = "http://127.0.0.1:7373"  # override for tests
```

Consumers select a provider and model with the `<provider>/<model>` shorthand:

```toml
# Illustrative — the real consumer RFD defines the actual key path.
[some_consumer]
model = "ember/rozgo/bge-reranker-v2-m3"
```

The v1 provider IDs are `voyage` and `ember`.
Unknown provider IDs fail at config-load time; see [Provider
identifiers](#provider-identifiers).

### `Provider` trait

The `Provider` trait lives in a new `jp_rerank` crate, parallel to `jp_llm`:

```rust
use jp_config::model::id::Name;

#[async_trait]
pub trait Provider: Send + Sync {
    /// Rerank candidates against a query using the named model.
    ///
    /// The returned vec preserves the caller's candidate order; consumers sort
    /// by score themselves.
    async fn rerank(
        &self,
        model: &Name,
        query: &str,
        candidates: &[&str],
    ) -> Result<Vec<RerankScore>, Error>;
}

pub struct RerankScore {
    /// Index into the candidates slice.
    pub index: usize,

    /// Relevance score in `[0, 1]`, higher is more relevant.
    ///
    /// All providers return values in this range.
    /// Sort order is reliable; threshold values may need per-model tuning,
    /// because different models (and different providers' normalization)
    /// produce different score distributions for the same nominal relevance.
    pub score: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The provider could not be reached or initialized — binary not on PATH,
    /// network unreachable, missing env var, connection refused.
    /// Retrying later or fixing the environment may help.
    #[error("rerank provider is not available: {reason}")]
    Unavailable { reason: String },

    /// The requested model is not served by this provider.
    /// For ember, the model was not in the `--model` list at server start.
    /// For Voyage, the model name is not recognized by the API.
    /// The caller may retry with a different model or reconfigure the provider.
    #[error("rerank provider does not serve model `{model}`")]
    UnknownModel { model: String },

    /// The provider was reached but did not produce usable output — non-zero
    /// exit, parse failure, malformed response, schema mismatch.
    /// Retrying with the same inputs will not help.
    #[error("rerank provider failed: {reason}")]
    Failed { reason: String },
}
```

The trait exposes neither batching nor top-k filtering; both stay internal to
each provider implementation.
Model selection happens per call rather than in provider configuration, matching
`jp_llm::Provider`, where the provider handles transport and the caller supplies
the model, query, and candidates.

Error variants describe the provider's failure state rather than prescribing a
recovery policy.
Each consumer decides whether to skip, fall back, retry, or surface the error.

### Observability

Every rerank call crosses an HTTP boundary to Voyage or `ember`.
Because consumers may swallow rerank errors, the provider layer logs enough
metadata at that boundary to diagnose failures:

- **Default `debug` log**: provider ID, model name, candidate count, latency,
  and outcome (success or error variant).
- **Omitted by default**: query and candidate text, keeping confidential or
  personally identifiable content out of the default log stream.
- **Payload tracing (`trace` level)**: providers serialize the outgoing request
  body to a temporary file using the `jp_llm::provider::trace_to_tmpfile`
  pattern (or a `jp_rerank` equivalent) and log its path.
  This keeps large payloads out of the structured log stream while preserving
  them for debugging.

The provider layer only logs facts; error recovery remains the consumer's
responsibility.

### Voyage provider

`jp_rerank::provider::voyage::VoyageProvider` posts to Voyage's `/rerank`
endpoint:

```json
{
  "model": "rerank-2.5",
  "query": "...",
  "documents": [
    "...",
    "..."
  ],
  "truncation": true
}
```

`truncation` is always `true` (matching Voyage's API default), since rejecting
overlong inputs with an error is not useful here.
`top_k` and `return_documents` are omitted because callers receive all scores
and already hold the input documents.

Following `providers.llm.*`, the provider reads its API key from the environment
variable named by `api_key_env` (default `VOYAGE_API_KEY`) and sends requests to
`base_url` (default `https://api.voyageai.com/v1`, overridable for test
recording).

Error mapping:

| Condition                                          | Maps to        |
| -------------------------------------------------- | -------------- |
| `api_key_env` unset in the environment             | `Unavailable`  |
| HTTP 401 / 403                                     | `Unavailable`  |
| HTTP 400 / 404 indicating unknown model            | `UnknownModel` |
| HTTP 4xx (other: malformed request, etc.)          | `Failed`       |
| HTTP 5xx, timeout, DNS failure, connection refused | `Unavailable`  |
| Network OK, body parse fails                       | `Failed`       |

### Ember provider and the `ember` binary

`jp_rerank::provider::ember::EmberProvider` is an HTTP client for a local
`ember` server (`crates/contrib/ember/`).
The server wraps `fastembed` v5 and keeps a fixed set of models in memory
between calls, so each request pays only the inference cost (~50ms).

JP does not start or manage the `ember` server; users run `ember serve`
themselves in a terminal, via systemd or launchd, or as another background
service.
`ember` is a standalone tool usable outside JP, and `EmberProvider` adapts its
HTTP responses to the `Provider` trait: a Rust user can install it with `cargo
install --path crates/contrib/ember`, run `ember serve`, and POST to it from any
language.

#### `ember` CLI

```
$ ember serve --port 7373 \
    --model rozgo/bge-reranker-v2-m3 \
    --model BAAI/bge-reranker-base
```

Starts an HTTP server bound to `127.0.0.1:7373` (port configurable).
Each repeatable `--model` flag is loaded at startup before the server accepts
requests.
The loaded model set is fixed for the lifetime of the process, with no lazy
loading or model hot-swap.

A single endpoint, `POST /rerank`, accepts a JSON body:

```json
{
  "model": "rozgo/bge-reranker-v2-m3",
  "query": "...",
  "candidates": [
    "...",
    "..."
  ]
}
```

and returns:

```json
{
  "scores": [
    {
      "index": 0,
      "score": 0.87
    },
    {
      "index": 1,
      "score": 0.23
    }
  ]
}
```

The server applies a sigmoid to raw BGE logits so scores fall in `[0, 1]`,
matching Voyage's range.
The `scores` array is sorted by `score` descending; callers that need input
order reconstruct it via `index`, which `EmberProvider` does before returning.

Requests for a model not in the server's `--model` list return HTTP 404 with a
JSON error body.
Logs go to stderr, and the process exits with code 0 on graceful shutdown or
non-zero on startup failure.

#### Latency costs

Ember separates startup costs from request-time inference:

- **Model download** (~600 MB per model, once per machine): downloads any
  `--model` not yet cached in `~/.cache/huggingface/` at startup.
- **Model and ONNX session load** (~1-2s per model, once per server start):
  builds the ONNX session and loads the tokenizer inside
  `fastembed::TextRerank::try_new` for each `--model` at startup.
- **Inference** (~50ms per call): the only cost paid at request time.

Because all declared `--model` instances load before the server accepts
connections, requests never block on model downloads or session initialization.

#### Ember provider behavior

The provider holds an HTTP client and a `base_url` (default
`http://127.0.0.1:7373`).
It is a pure client with no subprocess management, readiness polling, or
lifecycle code.
If `base_url` is unreachable, the call returns `Error::Unavailable` until the
user starts or restarts `ember serve`.

Error mapping:

| Condition                                       | Maps to        |
| ----------------------------------------------- | -------------- |
| HTTP connection refused / DNS failure / timeout | `Unavailable`  |
| HTTP 5xx                                        | `Unavailable`  |
| HTTP 404 (model not in server's `--model` set)  | `UnknownModel` |
| HTTP 4xx (other)                                | `Failed`       |
| Response body parse fails                       | `Failed`       |

### Crate layout

```
crates/jp_rerank/
  Cargo.toml
  src/
    lib.rs              # Provider trait, RerankScore, Error
    provider/
      mod.rs
      voyage.rs
      ember.rs

crates/jp_config/src/providers/rerank/
  mod.rs
  id.rs                  # RerankerModelIdConfig (<provider>/<model>)
  voyage.rs              # VoyageRerankConfig (api_key_env, base_url)
  ember.rs               # EmberRerankConfig (base_url)

crates/contrib/ember/
  Cargo.toml
  src/
    lib.rs               # rerank fn around fastembed (+ sigmoid normalization)
    main.rs              # CLI + HTTP server entry point
```

The `ember` crate depends on `fastembed`, `clap`, `serde`, `serde_json`,
`tokio`, an HTTP server crate (axum or similar), and stdlib.
It does not depend on `jp_rerank`, `jp_config`, or any other JP crate.
Conversely, JP does not depend on `ember` as a library or binary; it only
communicates with the HTTP server at `base_url` started by `ember serve`.

### Configuration types

The `jp_config::providers::rerank` module mirrors `jp_config::providers::llm`:

```rust
// jp_config/src/providers/rerank.rs

#[derive(Debug, Clone, PartialEq, Config)]
#[config(default, rename_all = "snake_case")]
pub struct RerankProvidersConfig {
    #[setting(nested)]
    pub voyage: VoyageRerankConfig,

    #[setting(nested)]
    pub ember: EmberRerankConfig,
}
```

Each provider's config struct lives in its own submodule.
`VoyageRerankConfig` carries `api_key_env` and `base_url`; `EmberRerankConfig`
carries only `base_url`.
All fields have schematic defaults, so omitting `[providers.rerank.voyage]` or
`[providers.rerank.ember]` from a TOML file uses the defaults, matching
`providers.llm.*`.
Model selection is configured per call by the consumer rather than on the
provider (see [Provider identifiers](#provider-identifiers)).

#### Provider identifiers

Reranker providers are referenced by a typed enum, mirroring
`jp_config::model::id::ProviderId`:

```rust
// jp_config/src/providers/rerank/id.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum RerankProviderId {
    Voyage,
    Ember,

    #[serde(skip)]
    Test,
}
```

The v1 IDs are `voyage` and `ember`.
Neither aliases nor user-defined named instances are supported; supporting
multiple Voyage instances with distinct credentials, base URLs, or policies is
deferred to a separate RFD.

Consumers specify a `(provider, model)` pair with the `<provider>/<model>`
shorthand:

```toml
[some_consumer]
model = "ember/rozgo/bge-reranker-v2-m3"
```

`RerankerModelIdConfig` in `jp_config::providers::rerank::id` parses this
string, following the same pattern as `jp_config::model::id::ModelIdConfig` over
`RerankProviderId`:

- **Unknown provider ID** (e.g. `model = "cohere/..."`) fails at config-load
  time with a typed deserialization error, so consumer code never sees an
  unresolvable provider.
- **Omitting a provider's config block** uses schematic defaults.
  `Error::Unavailable` surfaces only when a consumer calls a provider whose
  runtime prerequisites (env var, binary on PATH) are not met.
- **No default reranker provider.** Consumers that need a reranker must name one
  explicitly, avoiding hidden fallback paths in user configuration.

Resolution lives in `jp_rerank::provider::get_provider`, mirroring
`jp_llm::provider::get_provider`:

```rust
pub fn get_provider(
    id: RerankProviderId,
    config: &RerankProvidersConfig,
) -> Result<Arc<dyn Provider>>;
```

A consumer holding a `RerankerModelIdConfig` calls `get_provider(id.provider,
...)` to obtain the provider, then passes `&id.name` to
`provider.rerank(&id.name, query, candidates)` on each call.

### Installation

In v1, a justfile recipe installs `ember` from the workspace (matching `just
_install-comfort`):

```sh
just _install-ember
```

This runs `cargo install --path crates/contrib/ember --locked` and places the
binary on the user's PATH.
If a user configures the ember provider without installing the binary or running
`ember serve`, rerank calls return `Error::Unavailable`.

Release artifacts and package-manager distribution are follow-up work beyond v1.

## Drawbacks

- **Two-binary install for the ember provider.** Users must install both `jp`
  and `ember`.
  Until the user runs `just _install-ember`, `EmberProvider` returns
  `Error::Unavailable`.

- **User-managed ember lifecycle.** JP does not start or supervise `ember
  serve`; users run it in a terminal or as a system service.
  When the server is not running, rerank calls return `Error::Unavailable` and
  the consumer decides whether to skip, surface the error, or fall back.

- **New top-level config namespace.** `providers.rerank.*` increases JP's config
  surface area, though mirroring `providers.llm` avoids introducing a new
  configuration model.

- **Cold model load.** Starting `ember serve` on a new machine downloads each
  uncached `--model` to the HuggingFace cache (~600 MB for
  `bge-reranker-v2-m3`), and every server start builds ONNX sessions (~1-2s per
  model).

- **Voyage requires an API key.** Users without a Voyage account cannot use the
  cloud provider; `ember` is the offline alternative.

## Alternatives

### LLM-as-classifier

An existing `jp_llm::Provider` (Haiku, Sonnet) can return relevance scores
through a structured-output schema, but it is the wrong tool for per-turn
scoring:

- Latency is 500-2000ms per call, compared to ~50ms for a cross-encoder.
- Cost is $0.001-0.01 per call, compared to zero for local execution.
- Generative LLMs are not trained for relevance ranking, whereas cross-encoders
  are.

Latency and cost both compound when reranking runs on every turn.

### In-tree `fastembed` linkage behind a feature flag

Linking `fastembed-rs` directly into JP behind a `rerank-fastembed` feature flag
was rejected due to dependency and binary bloat.
`fastembed` pulls in `ort`, `tokenizers`, `hf-hub`, and ~50-80 transitive crates
plus a ~10-20 MB binary size increase.
Even behind a feature flag, the workspace must maintain the option, CI must test
both modes, and config schema generation must handle both surfaces.

Isolating `fastembed` in a contrib binary still incurs workspace and CI costs
(`Cargo.lock`, supply-chain review, and workspace-wide compile and test time),
which is standard for in-tree tools, but keeps `fastembed` out of the `jp` crate
graph so JP's binary size, startup time, and dependency surface remain
unaffected.
If a concrete consumer later needs to avoid the ~10ms spawn cost (dwarfed by
~50ms inference), the contrib binary can grow an in-process companion library
crate.

### Generic stdio rerank protocol

A single `Provider` implementation that speaks a JP-defined stdio protocol to a
configured binary is unnecessary when JP ships only one local binary.
If a second binary is ever needed, adding a dedicated `Provider` implementation
and config struct takes ~50 lines of code without taking on the documentation,
support, and Hyrum's-Law burden of a generic protocol contract.

### Cohere Rerank as the cloud provider

Cohere offers a comparable reranking API, but Voyage has cleaner free-tier
limits and an almost identical API shape.
A Cohere provider can be added in a follow-up RFD without changing the
`Provider` trait.

## Non-Goals

- **`Classifier` trait and zero-shot classification.** Turn quality, sentiment,
  and content moderation require a different ML primitive and belong in a
  follow-up RFD when a concrete consumer needs them.

- **In-process linking of `fastembed-rs` from JP.** Covered under
  [Alternatives](#in-tree-fastembed-linkage-behind-a-feature-flag); worth
  revisiting only if HTTP-sidecar overhead becomes a measurable problem on a
  consumer's critical path.

- **Additional cloud rerank providers (Cohere, Jina, etc.).** The `Provider`
  trait accommodates additional providers without changes; each can be added in
  a follow-up RFD.

- **Plugin-based provider hosting.** When [RFD 016]'s WASM plugin architecture
  matures, `EmberProvider` can migrate its transport from a local HTTP sidecar
  to a plugin behind the same `Provider` trait.

- **Polished distribution for the ember binary.** v1 uses the justfile recipe;
  release artifacts, package managers, and auto-install are follow-up work.

- **Low-friction built-in fallback.** Reranking without configuring a provider
  (via LLM-as-classifier, lexical/fuzzy matching, or another fallback) is out of
  scope.
  Consumer RFDs must either require explicit provider configuration or define
  their own fallback behavior.

## Risks and Open Questions

- **Latency on a cold cache or fresh server start.** Model downloads (~600 MB
  for `bge-reranker-v2-m3` on a new machine) and ONNX session builds (~1-2s per
  model on every start) run before `ember serve` accepts requests.
  JP's first call hits a warm server, placing cold-start latency entirely on
  whoever starts `ember serve`.

- **Voyage API stability.** `VoyageProvider` pins to `/v1/rerank` and will break
  if Voyage makes incompatible changes to that contract.
  The `base_url` override mitigates this by allowing tests to run against
  recorded fixtures.

- **Score distribution differences across models.** Voyage returns normalized
  relevance scores natively, while `ember` applies a sigmoid to raw BGE logits.
  Both fall in `[0, 1]` and support top-N plus threshold filtering, but the
  mapping between score magnitude and relevance differs across models and
  normalization methods.
  A threshold tuned for one model may need adjustment when switching, so
  consumer RFDs should treat the threshold as a per-model config value rather
  than a constant.

- **ember binary upgrade path.** Bumping `fastembed-rs` across major versions in
  `crates/contrib/ember` preserves the HTTP contract between JP and an older
  `ember` binary, but mismatched model files in the cache could confuse the
  HuggingFace cache layer.

## Implementation Plan

### Phase 1: `jp_rerank` crate

Create `jp_rerank` with the `Provider` trait, `RerankScore` struct, and `Error`
enum (`Unavailable`, `UnknownModel`, `Failed`), along with unit tests covering
the trait shape and the three error variants.

Independent of all other phases.

### Phase 2: `ember` contrib crate

Create `crates/contrib/ember/` wrapping `fastembed` v5.
Implement `ember serve` with a repeatable `--model` flag and a single `POST
/rerank` endpoint that loads each `--model` at startup, returns HTTP 404 for
unloaded models, applies sigmoid normalization to raw BGE logits, and sorts
`scores` descending.
Add the `just _install-ember` recipe.
Smoke test by starting the server with one `--model`, POSTing a request for the
loaded model to assert the response shape, and POSTing a request for an unloaded
model to assert HTTP 404.

Independent of `jp_rerank`; can be merged before or after Phase 1.

### Phase 3: Voyage provider

Implement `VoyageProvider` in `jp_rerank::provider::voyage` and integration test
it against recorded HTTP fixtures using the `base_url` override.

Depends on Phase 1.

### Phase 4: Ember provider

Implement `EmberProvider` in `jp_rerank::provider::ember` as a pure HTTP client
that restores the server's relevance-sorted response to input order before
returning.
Test against recorded HTTP fixtures via the `base_url` override, matching the
Voyage test pattern and covering the HTTP 404 → `UnknownModel` mapping; the
binary itself is tested in the contrib crate.

Depends on Phases 1 and 2.

### Phase 5: Config integration

Add `jp_config::providers::rerank::{id, voyage, ember}` modules, including a
`RerankerModelIdConfig` that parses `<provider>/<model>` strings into a typed
`(RerankProviderId, Name)` pair, and wire `RerankProvidersConfig` into
`AppConfig`.
Implement `jp_rerank::provider::get_provider(id, config)` returning an `Arc<dyn
Provider>`.

Depends on Phases 3 and 4.

### Phase 6: Documentation

Document the providers and the `ember` install requirement under
`docs/configuration.md` (or wherever provider docs live).

Depends on Phase 5.

## References

- [`fastembed-rs`]: Rust embedding and reranking library wrapped by the `ember`
  binary.
- [Voyage AI Reranker API]: HTTP contract for `VoyageProvider`.
- [`rozgo/bge-reranker-v2-m3`]: ONNX-format upload of `BAAI/bge-reranker-v2-m3`
  used in this RFD's examples (`fastembed` only loads ONNX models).
- [`jp_llm`]: Existing provider crate whose shape this RFD mirrors.
- [RFD 016]: Future WASM plugin system that `EmberProvider` may migrate to.

[RFD 016]: ../016-wasm-plugin-architecture.md
[Voyage AI Reranker API]: https://docs.voyageai.com/docs/reranker
[`fastembed-rs`]: https://github.com/Anush008/fastembed-rs
[`jp_llm`]: ../../../crates/jp_llm
[`rozgo/bge-reranker-v2-m3`]: https://huggingface.co/rozgo/bge-reranker-v2-m3
