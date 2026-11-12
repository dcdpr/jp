# RFD 133: Decision Models

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-08
- **Extended by**: [RFD 135]
- **Extends**: [RFD 072]
- **Summary**: Adds decision models through a `providers.decision` namespace,
  typed questions and answers, and a `decide` plugin request.

## Summary

A decision model answers typed questions about an input with probabilities
instead of generating text.
This RFD adds a `providers.decision` namespace for servers implementing the
System One API, question and answer types in `jp_conversation`, and a `decide`
protocol request for command plugins.

## Motivation

A decision model takes a *state* (text or a JSON value) and a set of named
questions, and returns one typed answer per question: the probability that a
statement is true, a selection from a fixed set of options, or a position on an
ordered rubric.
TypeSafe's Jev introduced this model class and its `POST /v1/systemone`
endpoint, which Ollama, AWS's Strands Decider, and open models such as Kev also
serve.
Responses return in 70 to 500 ms at a fraction of an LLM's input cost.

JP plugins frequently make these classifications: which crate owns an issue,
whether a ticket needs action, or whether a staged diff requires a changelog
entry.
With only LLM providers available, each classification requires a `query`
request with a `schema` ([RFD 072]), which runs a full turn with seconds of
latency and returns a structured value without a probability.
Because no JP provider speaks System One, a plugin that uses a decision model
must manage its own credentials and HTTP client.

## Design

### Terms

- **Decision model**: A model that answers questions about a state with typed,
  probabilistic answers.
- **Decision**: A single request to a decision model, consisting of a state,
  named questions, and the returned answers.
- **State**: The input evaluated by every question in a decision.

A decision question is distinct from an inquiry, which is a question a tool asks
during a turn and the user or assistant answers ([RFD 028]).

### Questions and answers

Question types follow the System One API:

| Type     | Question                                   | Answer                                                  |
| -------- | ------------------------------------------ | ------------------------------------------------------- |
| `noul`   | Is this statement true?                    | Probability of `true` (0 to 1)                          |
| `choice` | Which of these options applies? (2 to 255) | Selected option, probability per option, and confidence |
| `score`  | Where does this sit on a rubric? (2 to 10) | Expected level, probability per level, and confidence   |

Questions and answers are defined as JP types in
`jp_conversation::event::decision`.
Each decision source converts its server's wire format into these types so
answers have a uniform representation regardless of which server produced them,
matching how `jp_llm` providers convert wire formats into `ChatResponse` and
`ToolCallRequest`.
Their JSON serialization matches the System One schema within the subset below,
so plugin authors can reference the System One documentation for it.

Where System One servers differ in what they accept, JP's types accept only what
every server accepts:

- **State and instructions:** a non-blank string, an object, or an array.
  Empty objects and arrays are allowed.
  Every question requires `instructions` (optional in TypeSafe, required by
  Ollama).
- **Criterion descriptions:** strings only, for `choice`, `score`, and `noul`.
  A `choice` option may also have no description (`null`).
  TypeSafe also accepts structured descriptions, which these types reject.
- **Option counts:** 2 to 255 for `choice` and 2 to 10 for `score` (TypeSafe
  accepts one).

Limits that depend on the server or model, such as the number of questions in a
request or the characters a question name may use on Ollama's cloud, are not
enforced by JP; the server reports a violation as a provider error.

### Configuration

```toml
[providers.decision.typesafe]
source = "systemone"
base_url = "https://api.typesafe.ai"
api_key_env = "TYPESAFE_API_KEY"

[providers.decision.local]
source = "systemone"
base_url = "http://localhost:11434"

[decision.model]
id = "typesafe/jev-latest"
```

- `providers.decision` is a map of user-named provider entries, each specifying
  a `source` and that source's settings.
  Unlike `providers.llm`, which has one fixed struct field per implementation, a
  map lets a single source back multiple servers (such as a hosted endpoint and
  a local server).
- `systemone` is the only supported `source`.
  It accepts `base_url` and an optional `api_key_env`; omitting `api_key_env`
  sends unauthenticated requests for local servers.
- `source` is required in the finalized configuration, but an override layer
  patching an existing provider entry can omit it.
  Because `systemone` is the only source, all entries share a single settings
  struct and partial layers merge without repeating `source`.
- `decision.model` is optional, and `providers.decision` is empty by default.
  Configurations that do not use decision models require neither section.
- `decision.model.id` has the form `<provider>/<model>`.
  Referencing a provider not declared under `providers.decision` fails config
  resolution with an error listing the declared providers.
- `decision` is a top-level configuration section alongside `assistant`,
  participating in normal config layering and `--cfg` overrides.

Decision and LLM providers use separate namespaces and distinct model ID types.
Each model ID resolves its provider within its own namespace.
If `decision.model.id` names a model that the configured server cannot serve
(such as a chat model), the server rejects it when a request runs.

### Command plugins

A plugin sends `decide`, and the host replies with `decided`, or with `error` if
the request fails:

```json
{"type": "decide", "id": "7",
 "state": {"ticket": "…"},
 "questions": {"area": {"type": "choice",
                       "instructions": "Which crate owns this ticket?",
                       "criteria": {"cli": null, "llm": null}}},
 "cfg": ["decision.model.id=local/kev-4b"]}
```

```json
{"type": "decided", "id": "7", "model": "kev-4b",
 "answers": {"area": {"type": "choice", "choice": "cli", "confidence": 0.81,
                      "probabilities": {"cli": 0.9, "llm": 0.1}}}}
```

- `questions` and `answers` use the `jp_conversation::event::decision` types.
- A `decide` runs under the configuration the plugin last read: the one the host
  started the plugin with, replaced by each successful `read_config` reload.
  A failed reload leaves it unchanged.
- Each request captures that configuration when the host accepts it, and `cfg`
  layers over the request's own copy, matching `--cfg` on the CLI.
  Neither a later reload nor another request's `cfg` changes a decision already
  accepted.
- If `decision.model.id` remains unset after applying `cfg`, the host replies
  with an `error` naming `decision.model.id`.
- A `decide` request does not read or modify any conversation, and the host
  resolves provider credentials just as it does for `query`.
- The host executes `decide` requests concurrently and replies as each completes
  rather than in arrival order, so a plugin classifying a batch of items can
  keep multiple requests in flight.
- Because replies can arrive out of order, `decide` requires an `id` field,
  matching `query` and `interrupt` in [RFD 072].
  The host echoes `id` on `decided` and `error`; a `decide` request without an
  `id` is rejected immediately with an `error` before reaching the provider.
- In-flight `decide` requests are cancelled without a reply when the plugin
  exits, closes stdout, or receives a host shutdown.
  Unlike delegated `query` turns, which the host awaits after plugin exit
  because they persist conversation events, a `decide` request writes no state
  and has no reader once the plugin terminates.
- Adding `decide` and `decided` increments `PROTOCOL_VERSION` (from 13).
  A plugin that sends `decide` must declare the new protocol version in `ready`.

> [!TIP]
> [RFD 135] lets a `decide` request name a conversation to record into.
> Such a request writes state, so the host awaits it after the plugin exits, as
> it awaits a delegated `query`, instead of cancelling it.

### Crates

```text
plugin `decide` ──> decide runner (jp_cli) ──> jp_decision::Provider ──> systemone source
```

- **`jp_decision`**: New crate defining the `Provider` trait (`decide` and
  `models`) and the `systemone` source.
  Keeping the System One wire format inside the `systemone` module isolates
  changes to the external API.
- **`jp_conversation`**: Defines the decision question and answer types.
- **`jp_config`**: Adds `providers.decision`, `decision.model`, and a decision
  model ID keyed by user-declared provider name (unlike `ModelIdConfig`, which
  is keyed by the closed `ProviderId` enum).
- **`jp_cli`**: Adds the plugin protocol messages and a runner that resolves
  configuration, builds the request, and invokes the provider.

`jp_llm` is unchanged: decision models have no messages, tools, streaming, or
reasoning, so they do not fit `jp_llm::Provider`.

## Drawbacks

- **Additional provider namespace and model ID type.** Introducing
  `providers.decision` adds a third provider namespace alongside `llm` and `mcp`
  with a map structure that differs from `providers.llm`, plus a second model ID
  type alongside `ModelIdConfig`.
- **Coupling to an early specification.** The System One API is two months old.
  Wire-format changes stay isolated inside the `systemone` source, but JP's
  question and answer types mirror the System One data model.

## Alternatives

**Add a `systemone` source under `providers.llm`.** Rejected.
Sharing a single namespace would force chat and decision models to share one
model ID type and provider lookup, requiring every caller of `jp_llm::Provider`
to distinguish the two at runtime.

**Run decisions through an LLM with structured output.** Rejected as the primary
mechanism.
Although this works with every existing LLM provider, each decision incurs full
turn latency, and without logprobs any returned probabilities are uncalibrated
estimates.
An LLM-backed source remains a possible future addition (see
[Non-Goals](#non-goals)), provided its answers indicate that their probabilities
are uncalibrated.

**Have plugins call decision servers directly.** Rejected.
Avoiding a protocol addition would force each plugin to bundle its own HTTP
client and manage credentials that JP already holds.

**Add a `model` field to the `decide` plugin request.** Rejected.
While slightly shorter for plugins that switch models per request, a dedicated
field would bypass the configuration pipeline that every other request uses, and
`cfg` already supports per-request model overrides.

## Non-Goals

- **A CLI subcommand or conversation persistence for decisions.** Both are
  deferred to a separate RFD.
- **A `decide` tool for the assistant.**
- **`box` questions and image states.** TypeSafe gates both behind opt-in
  headers and sends images as `assets`, Ollama sends them in a separate `images`
  field, and JP has no rendering surface for `box` answers.
  The `systemone` source therefore sends text and JSON only.
- **Other decision sources**, such as OpenAI's Decisions API (which has no
  published schema) or an LLM-backed fallback source.
- **Using decision models inside JP's own internal flows**, such as answering
  tool inquiries ([RFD 028]) or gating tool calls.
  Those workflows can build on `jp_decision::Provider` in separate RFDs.

## Risks and Open Questions

**Prompt injection against decision models.** A University of Southern
California study found that short, natural-looking injected text flipped 61% of
correct Jev decisions.
Any decision over untrusted input (such as an issue body, web page, or tool
output) that gates an action is vulnerable to steering, and the `decide`
protocol documentation notes this risk.

**Divergence across System One servers.** Implementations of System One differ
in request validation, option limits, and confidence calculation.
For example, Ollama requires `instructions` and a non-blank string `state`, caps
cloud requests at 64 questions, and echoes the requested model name where
TypeSafe returns the resolved model version.
Context limits differ too: Jev allows 64k tokens per request and 32k per
question, Ollama's cloud 65,536 tokens, and a local Ollama model 64 KiB of text.
JP does not track these limits: a state that is too long is the server's to
reject.
JP's types enforce the requirements shared by all servers, and any
server-specific rejection is returned as a provider error.
JP forwards the server's answers as returned and makes no guarantee that
probabilities or confidence scores are comparable across models.

## Implementation Plan

1. **Configuration.** Add `providers.decision` with `systemone` settings,
   `decision.model`, the decision model ID type, validation that the referenced
   provider is declared, and updated schema snapshots.
   This phase is independently mergeable.
2. **Types and `jp_decision`.** Add the question and answer types in
   `jp_conversation`, the `jp_decision::Provider` trait, and the `systemone`
   source with recorded request/response fixture tests matching `jp_llm`.
   This phase depends on Phase 1 and is independently mergeable.
3. **Plugin protocol.** Add the `decide` runner, `decide` and `decided`
   messages, concurrent execution, required `id` validation, and the bumped
   protocol version, and list `decide` alongside `query` and `interrupt` in [RFD
   072]'s Request IDs section.
   This phase depends on Phase 2.
4. **Documentation.** Add a configuration guide section and glossary entries for
   Decision and Decision Model (distinguishing decisions from inquiries).
   This phase depends on Phase 3.

## References

- [System One API reference][systemone-api]
- [Introducing System One Models & Jev][jev-intro], TypeSafe
- [Ollama's System One API reference][ollama-systemone]
- [RFD 028]: Structured Inquiry System for Tool Questions
- [RFD 072]: Command Plugin System

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 072]: 072-command-plugin-system.md
[RFD 135]: 135-jp-decide.md
[jev-intro]: https://typesafe.ai/blog/introducing-system-one-models-and-jev
[ollama-systemone]: https://docs.ollama.com/api/systemone
[systemone-api]: https://docs.typesafe.ai/api/system-one
