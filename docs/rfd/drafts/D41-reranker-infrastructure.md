# RFD D41: Reranker Infrastructure

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-05-22

## Summary

This RFD introduces a reranker primitive: a `Provider` trait that ranks
candidates by relevance to a query, with two initial implementations — Voyage
AI's HTTP reranking API and a local `ember` server backed by `fastembed`.
The primitive is foundational for upcoming features that need on-the-fly
relevance scoring without paying for full LLM inference.

## Motivation

JP has no reusable cross-encoder reranker primitive available to core consumers.
Lexical ranking exists in a contrib crate but is domain-specific (Bear notes)
and not exposed as a general API.
Several near-term features need general-purpose relevance scoring:

- **Instruction reminders** want to score the project's instructions against the
  assistant's last reply, and remind the assistant of the most relevant ones.
- **Knowledge base retrieval** (a future consumer) could use the same mechanism
  to pick which subjects to surface for a given query.
- **Future quality signals** such as sentiment, turn classification, sit
  adjacent to this primitive.

LLM-based relevance scoring works but is the wrong tool.
A Haiku-class chat-completion call costs roughly 500-2000ms and a few cents per
invocation, generates output through a constrained-decoding schema, and uses a
model that was trained for open-ended generation, not relevance ranking.
Cross-encoder rerankers (e.g.
`BAAI/bge-reranker-v2-m3`) solve the same problem in ~50ms, locally, with
task-specific training, and at zero per-call cost once installed.

This RFD introduces the primitive so consumers can be designed against it.
The consumers themselves (instruction reminders, KB retrieval) ship as separate
RFDs.

## Design

### Configuration

Users configure one or more reranker providers under `providers.rerank.*`,
mirroring the shape of `providers.llm.*`:

```toml
[providers.rerank.voyage]
api_key_env = "VOYAGE_API_KEY"
# base_url = "https://api.voyageai.com/v1"  # override for tests

[providers.rerank.ember]
# base_url = "http://127.0.0.1:7373"  # override for tests
```

Consumers reference a provider and model together using the `<provider>/<model>`
shorthand, mirroring how `jp_llm` consumers reference models:

```toml
# Illustrative — the real consumer RFD defines the actual key path.
[some_consumer]
model = "ember/rozgo/bge-reranker-v2-m3"
```

Provider IDs accepted in v1 are `voyage` and `ember`.
Unknown provider IDs in a `<provider>/<model>` reference fail at config-load
time; see [Provider identifiers](#provider-identifiers) below.

### The `Provider` trait

The trait lives in a new `jp_rerank` crate, parallel to `jp_llm`.
Shape:

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

The trait deliberately exposes no batching and no top-k — those are
provider-config concerns, hidden inside each implementation.
Model selection lives on the call, not the provider config: the caller hands the
trait a model, query, and candidates and receives scores.
This mirrors `jp_llm::Provider`, where the provider is the transport and the
model is chosen per call.

All error variants describe the state of the provider, not how the caller should
react.
Consumers decide whether to skip, fall back, retry, or surface the error — the
primitive does not prescribe a policy.

### Observability

Each rerank call is an external boundary — Voyage HTTP call or ember HTTP call
— and the layer logs enough at that boundary for users to diagnose issues when
consumers swallow errors silently.

- **What every rerank call records.** Provider ID, model name, candidate count,
  latency, outcome (success / error variant).
  Logged at `debug` level by default.
- **What it does *not* record by default.** Query text, candidate text.
  Personally-identifiable or project-confidential content stays out of the
  default log stream.
- **Payload tracing.** When deeper inspection is needed, providers serialize the
  outgoing request body via the existing `jp_llm::provider::trace_to_tmpfile`
  pattern (or a `jp_rerank`-local sibling) and emit the path at `trace` level.
  This avoids dumping large payloads into the structured log stream while
  keeping them accessible when investigating a bad-rerank report.
- **Policy stays with the consumer.** The primitive logs facts; how the consumer
  reacts (skip, warn, surface, fall back) remains the consumer's choice.

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

`truncation` is always `true`.
Voyage's API default is also `true`, and the alternative (rejecting overlong
inputs with an error) has no useful behavior in our context.

`top_k` and `return_documents` are not sent.
We always want all scores; we already hold the documents.

The API key is read from the env var named by `api_key_env` (default
`VOYAGE_API_KEY`), matching the convention in `providers.llm.*`.

Error mapping:

| Condition                                          | Maps to        |
| -------------------------------------------------- | -------------- |
| `api_key_env` unset in the environment             | `Unavailable`  |
| HTTP 401 / 403                                     | `Unavailable`  |
| HTTP 400 / 404 indicating unknown model            | `UnknownModel` |
| HTTP 4xx (other: malformed request, etc.)          | `Failed`       |
| HTTP 5xx, timeout, DNS failure, connection refused | `Unavailable`  |
| Network OK, body parse fails                       | `Failed`       |

`base_url` defaults to `https://api.voyageai.com/v1` and is overridable for test
recording (matches how LLM providers work in JP today).

### Ember provider and the `ember` binary

`jp_rerank::provider::ember::EmberProvider` is an HTTP client that talks to a
local `ember` server.
The server holds a fixed set of models in memory between calls, so every
successful request is just inference (~50ms).

JP does not start or manage the `ember` server.
Users run `ember serve` themselves — in a terminal, via systemd/launchd, or
however they prefer.

The `ember` binary lives at `crates/contrib/ember/`.
It is a standalone tool that wraps `fastembed` v5 and is generic enough to be
useful outside JP — the provider is what adapts ember's HTTP responses to the
`Provider` trait, not the other way around.

#### `ember` CLI

```
$ ember serve --port 7373 \
    --model rozgo/bge-reranker-v2-m3 \
    --model BAAI/bge-reranker-base
```

Starts an HTTP server bound to `127.0.0.1:7373` (port configurable).
Each `--model` (repeatable) is loaded during startup; the server begins
accepting requests once all declared models are loaded.
The loaded set is fixed for the lifetime of the process — there is no lazy
loading and no model hot-swap.

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

The `scores` array is sorted by `score` descending; callers reconstruct input
order via `index` if they need it.
JP's `EmberProvider` normalizes back to input order before returning, per the
trait contract.

Requests for a model not in the server's `--model` list return HTTP 404 with a
JSON error body.

Scores are normalized to `[0, 1]` — the server applies a sigmoid to the raw BGE
logits before returning, so the response range matches Voyage's out-of-the-box.

Logs go to stderr.
Exit code 0 on graceful shutdown, non-zero on startup failure.

The CLI shape is ember's contract, not JP's.
The contrib crate is meant to be useful independently — a Rust user who wants a
fast local reranker can `cargo install --path crates/contrib/ember`, run `ember
serve`, and POST to it from any language.

#### Three latency costs

Ember has three distinct latency costs that matter for consumer planning:

- **Model download** (~600 MB per model, once per machine). Happens during
  startup for any `--model` not already in the HuggingFace cache at
  `~/.cache/huggingface/`.
  Paid before the server accepts requests.
- **Model / ONNX session load** (~1–2s per model, once per server start).
  The ONNX session build and tokenizer load run inside
  `fastembed::TextRerank::try_new` for each `--model` at startup.
  Paid before the server accepts requests.
- **Inference** (~50ms per call).
  The per-rerank cost — the *only* cost paid at request time.

Every successful request is ~50ms.
The HTTP-server design plus the explicit `--model` declaration mean download and
session-build costs are paid once per server start, never at request time.

#### Ember provider behavior

The provider holds an HTTP client and a `base_url` (default
`http://127.0.0.1:7373`).
It is purely a client: no subprocess management, no readiness polling, no
lifecycle code.
If `base_url` is unreachable, the call returns `Error::Unavailable`; the user
starts or restarts `ember serve` to recover.

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
JP, in turn, does not depend on the ember crate as a library or a binary — it
only needs an HTTP server reachable at `base_url`, which the user provides by
running `ember serve`.

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

Each provider's config struct lives in its own submodule with provider-specific
fields.
`VoyageRerankConfig` carries `api_key_env` and `base_url`; `EmberRerankConfig`
carries only `base_url`.
Both fields have schematic defaults, so omitting `[providers.rerank.voyage]` or
`[providers.rerank.ember]` from a TOML file is equivalent to using the defaults,
identical to how `providers.llm.*` works.
The model is not a provider-config field — it's named per call by the consumer
(see [Provider identifiers](#provider-identifiers) below).

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
No aliases, no user-defined named instances — if a consumer ever needs two
Voyage instances with different credentials, base URLs, or policies, that gets
its own RFD.

Consumers reference a `(provider, model)` pair using the `<provider>/<model>`
shorthand, parallel to how `jp_llm` consumers reference models:

```toml
[some_consumer]
model = "ember/rozgo/bge-reranker-v2-m3"
```

The shape is defined by a sibling `RerankerModelIdConfig` in
`jp_config::providers::rerank::id` — the same pattern as
`jp_config::model::id::ModelIdConfig`, just over `RerankProviderId`.

Behavior:

- **Unknown provider ID** (e.g.
  `model = "cohere/..."`) fails at config-load time with a typed deserialization
  error.
  Consumer code never sees an unresolvable provider.
- **Omitting a provider's config block is fine** — schematic fills in defaults.
  The `Unavailable` error only surfaces when a consumer actually calls a
  provider whose runtime prerequisites (env var, binary on PATH) aren't met.
- **No default reranker provider.** Consumers that need a reranker must name one
  explicitly.
  A built-in default would create silent fallback paths the user can't see in
  their config.

Resolution lives in `jp_rerank::provider::get_provider`, mirroring
`jp_llm::provider::get_provider`:

```rust
pub fn get_provider(
    id: RerankProviderId,
    config: &RerankProvidersConfig,
) -> Result<Arc<dyn Provider>>;
```

The model is passed at call time, not at resolution.
A consumer holding a `RerankerModelIdConfig` calls `get_provider(id.provider,
...)` to obtain the provider, then `provider.rerank(&id.name, query,
candidates)` for each call.

### Installation

For v1, ember is installed from the workspace via a justfile recipe (matching
`just _install-comfort`):

```sh
just _install-ember
```

This runs `cargo install --path crates/contrib/ember --locked` and places the
binary on the user's PATH.
If a user configures the ember provider but has not installed the binary or is
not running `ember serve`, rerank calls return `Error::Unavailable`.
How a consumer reacts is the consumer's choice (see the trait's error docs).

Better distribution (release artifacts, package managers) is a follow-up
concern, not a v1 blocker.

## Drawbacks

- **Two-binary install for the ember provider.** Users have to install both `jp`
  and `ember`.
  If a consumer needs reranking and the user hasn't run `just _install-ember`,
  the provider returns `Error::Unavailable`.

- **User-managed ember lifecycle.** JP does not start or manage the `ember`
  server — users run `ember serve` themselves, in a terminal or as a system
  service.
  If the server isn't running, rerank calls return `Error::Unavailable`; the
  consumer decides whether to skip, surface the error, or fall back.

- **New top-level config namespace.** `providers.rerank.*` increases JP's config
  surface area.
  Mitigated by mirroring `providers.llm` exactly, so there's no new mental
  model.

- **Cold model load.** Ember's first server start on a new machine downloads
  each `--model` not already in the HuggingFace cache (~600 MB for
  `bge-reranker-v2-m3`). Subsequent server starts hit the cache but still build
  ONNX sessions (~1–2s per model).
  The download is one-time per machine; session build is once per server start.

- **Voyage requires an API key.** Users without a Voyage account can't use the
  cloud provider.
  Ember is the offline fallback.

## Alternatives

### LLM-as-classifier

Use an existing `jp_llm::Provider` (Haiku, Sonnet) with a structured-output
schema to return relevance scores.
Works, but:

- 500–2000ms per call vs ~50ms for a cross-encoder.
- $0.001–0.01 per call vs zero for local.
- Generative LLMs are not trained for relevance ranking; cross-encoders are.

Rejected: wrong tool for the job.
Latency and cost both compound at per-turn frequency.

### In-tree `fastembed` linkage behind a feature flag

A `fastembed` provider that links `fastembed-rs` directly as a JP dependency,
gated by a `rerank-fastembed` feature flag.

Rejected: the bloat tradeoff doesn't favor it.
`fastembed` pulls in `ort`, `tokenizers`, `hf-hub`, and ~50–80 transitive
crates plus a ~10–20 MB binary size increase.
Even behind a feature flag, the workspace has to know about the option, CI has
to test both modes, and config schema generation has to handle both surfaces.
The contrib-binary approach pays workspace and CI costs (Cargo.lock,
supply-chain review, workspace-wide compile and test time) but avoids them in
the `jp` binary itself: the `jp` crate graph never depends on `fastembed`, so
JP's binary size, startup time, and dependency surface are unaffected.
The contrib-crate CI cost is the standard price of in-tree maintained tools, and
we accept it.

The contrib binary can grow an in-process companion crate later if a concrete
consumer needs to avoid the spawn cost.
That cost is ~10ms today, dwarfed by inference.

### Generic stdio rerank protocol

A single `Provider` implementation that accepts any binary speaking a JP-defined
stdio protocol, with the binary configured per provider instance.

Rejected: YAGNI.
We ship one binary today.
A second binary, if it ever materializes, can be a second provider with its own
trait implementation and its own config struct.
The cost of one extra impl is ~50 lines of code; the cost of a generic protocol
contract is documentation, support, and Hyrum's-Law debt forever.

### Cohere Rerank as the cloud provider

Cohere has a comparable reranking API.
The choice of Voyage over Cohere is pragmatic: Voyage has cleaner free-tier
limits and the API is essentially identical in shape.
A Cohere provider can be added later as a small follow-up RFD; the trait shape
is designed to accommodate it.

## Non-Goals

- **`Classifier` trait and zero-shot classification.** A different ML primitive
  with different use cases (turn quality, sentiment, content moderation).
  It belongs in a follow-up RFD if and when a concrete consumer needs it.

- **In-process linking of `fastembed-rs` from JP.** See alternatives above.
  Worth revisiting if the HTTP-sidecar overhead ever becomes a measurable
  problem on a real consumer's critical path.

- **Additional cloud rerank providers (Cohere, Jina, etc.).** Each can be added
  as a small follow-up RFD when needed.
  The trait is designed to accommodate them without changes.

- **Plugin-based provider hosting.** When [RFD 016]'s WASM plugin architecture
  matures, the ember provider can migrate from "talk to a local HTTP sidecar" to
  "invoke a plugin."
  Same trait, different transport.
  Out of scope here.

- **Polished distribution for the ember binary.** v1 is "use the justfile
  recipe."
  Release artifacts, package managers, and auto-install are a follow-up.

- **Low-friction built-in fallback.** A "rerank without configuring a provider"
  path — whether via LLM-as-classifier, lexical/fuzzy matching, or something
  else — is out of scope here.
  Consumer RFDs that need reranking can either require explicit provider
  configuration or define their own fallback behavior until a follow-up RFD
  addresses this.

## Risks and Open Questions

- **Latency on a cold cache or fresh server start.** A first-ever ember server
  start on a new machine downloads each `--model` (~600 MB for
  `bge-reranker-v2-m3`) and builds ONNX sessions (~1–2s per model).
  Subsequent starts skip the download but still pay session build.
  All of this happens before the server accepts requests, so JP's first call
  hits a warm server — cold-start cost is entirely on whoever starts `ember
  serve`.

- **Voyage API stability.** We're pinning to a specific API version
  (`/v1/rerank`).
  If Voyage breaks the contract, the provider breaks.
  Standard cloud-API risk; mitigated by the `base_url` override making it
  trivial to point at a recorded test fixture.

- **Score distribution differences across models.** Voyage returns normalized
  relevance scores natively; ember applies a sigmoid to raw BGE logits.
  Both fall in `[0, 1]`, and both support the same "top-N + threshold" usage
  pattern.
  The relationship between score magnitude and "true" relevance differs between
  models — and between providers normalizing the same nominal relevance
  differently — so a threshold tuned for one model may need adjustment when
  switching.
  Consumer RFDs should treat the threshold as a per-model config value, not a
  universal constant.

- **ember binary upgrade path.** When `fastembed-rs` ships a major version with
  breaking changes, we bump `crates/contrib/ember`'s dep.
  Users running an old `ember` binary against a new JP shouldn't break (the CLI
  contract is stable), but mismatched model files in cache might confuse the
  HuggingFace cache layer.
  Worth monitoring.

## Implementation Plan

### Phase 1: `jp_rerank` crate

Create the crate with the `Provider` trait, `RerankScore` struct, and `Error`
enum (`Unavailable`, `UnknownModel`, `Failed`).
No implementations yet.
Unit tests cover the trait shape and the three error variants.

Independent of all other phases.

### Phase 2: `ember` contrib crate

Create `crates/contrib/ember/` wrapping `fastembed` v5.
Implement `ember serve` with a repeatable `--model` flag and a single `POST
/rerank` endpoint.
Each `--model` is loaded at startup; requests for unloaded models return HTTP
404.
Apply sigmoid normalization to raw BGE logits before returning, with the
`scores` array sorted by score descending.
Add `just _install-ember` recipe.
Smoke test: start the server with one `--model`, POST a request for the loaded
model and assert the response shape, POST a request for an unloaded model and
assert HTTP 404.

Independent of `jp_rerank`.
Can be merged before or after Phase 1.

### Phase 3: Voyage provider

Implement `VoyageProvider` in `jp_rerank::provider::voyage`.
Integration test against recorded HTTP fixtures using the `base_url` override.

Depends on Phase 1.

### Phase 4: Ember provider

Implement `EmberProvider` in `jp_rerank::provider::ember` as a pure HTTP client.
Normalize the server's relevance-sorted response back to input order before
returning.
Tests use recorded HTTP fixtures via the `base_url` override, matching the
Voyage test pattern, and cover the HTTP 404 → `UnknownModel` mapping.
Real binary tested separately via the contrib crate's tests.

Depends on Phases 1 and 2.

### Phase 5: Config integration

Add `jp_config::providers::rerank::{id, voyage, ember}` modules, including a
`RerankerModelIdConfig` that parses `<provider>/<model>` strings into a typed
`(RerankProviderId, Name)` pair.
Wire `RerankProvidersConfig` into `AppConfig`.
Implement `jp_rerank::provider::get_provider(id, config)` returning an `Arc<dyn
Provider>` — the model is passed at call time, not at resolution.

Depends on Phases 3 and 4.

### Phase 6: Documentation

Document the providers under `docs/configuration.md` (or wherever provider docs
live).
Note the install requirement for ember.

Depends on Phase 5.

## References

- [`fastembed-rs`] — the Rust embedding/reranking library wrapped by the ember
  binary.
- [Voyage AI Reranker API] — the HTTP contract for the Voyage provider.
- [`rozgo/bge-reranker-v2-m3`] — the reranking model used in this RFD's
  examples.
  An ONNX-format upload of `BAAI/bge-reranker-v2-m3`; the ONNX variant is
  required because `fastembed` only loads ONNX models.
- [`jp_llm`] — the existing provider crate whose shape this RFD mirrors.
- [RFD 016] — future WASM plugin system the ember provider may migrate to.

[RFD 016]: ../016-wasm-plugin-architecture.md
[Voyage AI Reranker API]: https://docs.voyageai.com/docs/reranker
[`fastembed-rs`]: https://github.com/Anush008/fastembed-rs
[`jp_llm`]: ../../crates/jp_llm
[`rozgo/bge-reranker-v2-m3`]: https://huggingface.co/rozgo/bge-reranker-v2-m3
