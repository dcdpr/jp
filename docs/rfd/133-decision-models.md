# RFD 133: Decision Models

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-08
- **Extended by**: [RFD 135]
- **Summary**: Adds decision models through a `providers.decision` namespace,
  typed questions and answers, and a `decide` plugin request.

## Summary

A decision model answers typed questions about a piece of input with
probabilities, and generates no text.
This RFD adds a `providers.decision` namespace for servers that speak the System
One API, JP's own types for questions and answers, and a `decide` request that
lets command plugins ask a decision model.

## Motivation

A decision model takes a *state* (text or a JSON value) and a set of named
questions, and returns one typed answer per question: the probability that a
statement is true, a pick from a fixed set of options, or a position on an
ordered rubric.
TypeSafe's Jev was the first.
The endpoint it introduced, `POST /v1/systemone`, is also served by Ollama,
AWS's Strands Decider, and open models such as Kev.
An answer arrives in 70 to 500 ms, and input costs a fraction of an LLM's.

Plugins built on JP make small judgments of this kind: which crate an issue
belongs to, whether a ticket is worth acting on, whether a staged diff needs a
changelog entry.
Today each one is a `query` request with a `schema` ([RFD 072]): a full turn for
one structured value, seconds of latency, and an answer with no probability
attached.
No JP provider speaks System One, so a plugin that wants a decision model has to
bring its own credentials and HTTP client.

## Design

### Terms

- **Decision model**: a model that answers questions about a state with typed,
  probabilistic answers.
- **Decision**: one request to a decision model: a state, named questions, and
  the answers that come back.
- **State**: the input every question in a decision is about.

A decision question is not an inquiry.
An inquiry is a question a tool asks during a turn, answered by the user or the
assistant ([RFD 028]).

### Questions and answers

Question types follow the System One API:

| Type     | Asks                                        | Answer                                                    |
| -------- | ------------------------------------------- | --------------------------------------------------------- |
| `noul`   | Is this statement true?                     | probability of yes, 0 to 1                                |
| `choice` | Which of these options applies? (up to 255) | the option, a probability per option, a confidence        |
| `score`  | Where does this sit on a rubric? (up to 10) | the expected level, a probability per level, a confidence |

Questions and answers are JP's own types, in `jp_conversation::event::decision`.
Each decision source converts its server's format into them, so an answer has
one shape whichever server gave it.
This follows `jp_llm`, whose providers convert their wire formats into
`jp_conversation`'s `ChatResponse` and `ToolCallRequest`.
Their JSON matches the System One shapes, so a plugin author reads one set of
documentation.

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

- `providers.decision` is a map of providers the user declares and names, each
  naming a provider source and holding that source's settings.
  One source can back several servers, such as a hosted endpoint and a local
  one, which a struct with one field per implementation, as `providers.llm` is,
  cannot express.
  The namespace is new, so it starts in that shape with nothing to migrate.
- `systemone` is the only source.
  Its settings are `base_url` and an optional `api_key_env`; without a key,
  requests carry no credential, which suits a local server.
- `source` is required once layers are merged.
  A layer that patches a declared provider may leave it out.
  With one source there is one settings struct, so a patch that leaves out
  `source` still merges into the right fields.
- `decision.model.id` is `<provider>/<model>`.
  A provider not declared under `providers.decision` is a config error that
  lists the declared ones.
- `decision` is a new top-level section, next to `assistant`, layered and
  overridden like any other.

Decision and LLM providers are separate namespaces with separate model ID types.
Setting a decision model as `assistant.model.id`, or an LLM as
`decision.model.id`, is a config error rather than a failed request.

### Command plugins

A plugin sends `decide`, and the host answers `decided`, or `error` when the
request fails:

```json
{"type": "decide", "id": "7",
 "state": {"ticket": "…"},
 "questions": {"area": {"type": "choice", "criteria": {"cli": null, "llm": null}}},
 "cfg": ["decision.model.id=local/kev-4b"]}
```

```json
{"type": "decided", "id": "7", "model": "kev-4b",
 "answers": {"area": {"type": "choice", "choice": "cli", "confidence": 0.81,
                      "probabilities": {"cli": 0.9, "llm": 0.1}}}}
```

- `questions` and `answers` are the types above.
- `cfg` layers over the plugin's config for this request only, as `--cfg` does
  for one run of a command.
- A `decide` request touches no conversation.
- The host holds the credentials, as it does for `query`.
- The host answers requests as they complete, not in the order they arrived.
  A plugin classifying a backlog keeps many in flight, and that only pays off if
  the host runs them concurrently.
- The messages raise `PROTOCOL_VERSION`, 13 today.
  A plugin that sends `decide` declares the new version in `ready`.

### Crates

```text
plugin `decide` ──> decide runner (jp_cli) ──> jp_decision::Provider ──> systemone source
```

- **`jp_decision`** is a new crate.
  It holds the `Provider` trait (`decide` and `models`) and the `systemone`
  source.
  The System One wire format stays inside the source, so a change to the
  standard changes one module.
- **`jp_conversation`** gains the question and answer types.
- **`jp_config`** gains `providers.decision`, `decision.model`, and a decision
  model ID keyed by provider name.
  `ModelIdConfig` keys on the closed `ProviderId` enum, which cannot name a
  declared provider.
- **`jp_cli`** gains the plugin messages and a runner that resolves config,
  builds the request, and calls the provider.

`jp_llm` does not change.
A decision model has no messages, tools, streaming, or reasoning, and fits none
of `jp_llm::Provider`'s methods.

## Drawbacks

- A third provider namespace next to `llm` and `mcp`, in a different shape from
  `providers.llm`, and a second model ID type next to `ModelIdConfig`.
- System One is two months old.
  A breaking change stays inside the `systemone` source, but JP's question and
  answer types follow the standard's.

## Alternatives

**A `systemone` source under `providers.llm`.** One namespace instead of two,
but `assistant.model.id = "typesafe/jev-latest"` would be accepted and fail only
when a query runs.

**Decisions through an LLM with structured output.** Works with every provider
JP has.
Without logprobs the probabilities are invented, and each decision costs a
turn's latency.
It stays a possible later source (see Non-Goals), whose answers would have to
say that their probabilities are not calibrated.

**Plugins call decision servers themselves.** Needs no protocol change, but each
plugin brings its own HTTP client and asks the user for credentials JP already
holds.

**A `model` field on the plugin request.** Shorter for a plugin that alternates
models, but it bypasses the config pipeline every other request goes through.
`cfg` reaches the same models.

## Non-Goals

- **A shell command, and decisions recorded in a conversation.** Both are left
  to their own RFD.
- **A `decide` tool for the assistant.**
- **`box` questions and images in state.** TypeSafe gates both behind opt-in
  headers, and a `box` answer has nothing to render it.
- **Other decision sources,** such as OpenAI's Decisions API, which has no
  published schema, or an LLM-backed source.
- **Decisions inside JP's own flows,** such as answering tool inquiries ([RFD
  028]) or gating tool calls.
  Each builds on `jp_decision::Provider` in its own RFD.

## Risks and Open Questions

**Steerable answers.** A University of Southern California study reported that
short, natural-looking injected text flipped 61% of correct Jev decisions.
A decision over untrusted input (an issue body, a web page, tool output) that
gates an action can be steered.
The documentation for the `decide` request says so.

**Servers differ.** Servers implementing System One differ in option limits and
in how they compute confidence.
JP passes answers through and claims nothing about comparing them across models.

## Implementation Plan

1. **Config.** `providers.decision` with the `systemone` settings,
   `decision.model`, the decision model ID type, and the check that its provider
   is declared.
   Schema snapshots updated.
   Merges alone.
2. **Types and `jp_decision`.** The question and answer types in
   `jp_conversation`, the trait, and the `systemone` source, with request and
   response tests against recorded responses, as `jp_llm`'s providers have.
   Depends on 1.
   Merges alone.
3. **Plugins.** The runner, `decide` and `decided`, concurrent handling, and the
   protocol version.
   Depends on 2.
4. **Documentation.** A configuration guide section, and glossary entries for
   Decision and Decision Model, each noting that a decision is not an inquiry.
   Depends on 3.

## References

- [System One API reference][systemone-api]
- [Introducing System One Models & Jev][jev-intro], TypeSafe
- [RFD 028]: Structured Inquiry System for Tool Questions
- [RFD 072]: Command Plugin System

[RFD 028]: 028-structured-inquiry-system-for-tool-questions.md
[RFD 072]: 072-command-plugin-system.md
[RFD 135]: 135-jp-decide.md
[jev-intro]: https://typesafe.ai/blog/introducing-system-one-models-and-jev
[systemone-api]: https://docs.typesafe.ai/api/system-one
