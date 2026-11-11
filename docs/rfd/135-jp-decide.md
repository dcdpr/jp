# RFD 135: jp decide

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-11-08
- **Extends**: [RFD 133]
- **Requires**: [RFD 134], [RFD 065]
- **Summary**: Adds `jp decide`, which asks a decision model from the shell and
  records the answer for the next `jp query`.

## Summary

`jp decide` asks a decision model ([RFD 133]) one or more questions from the
shell and prints the answers as JSON.
It runs under the active conversation's config and records the decision there,
where the next `jp query` sends it to the model with its request; with no active
conversation, it runs without one.
A recorded decision stays with that request through replay, fork, and
compaction.

## Motivation

Scripts built on JP make small judgments: whether a staged diff needs a
changelog entry, which crate an issue belongs to, whether a report has enough to
reproduce it.
Today each one is a `jp query --schema` turn: the full agent loop for one
structured value, seconds of latency, and an answer with no probability
attached.
[RFD 133] gives JP decision models and gives plugins a way to ask them; the
shell has none.

Many decisions are about the work a conversation is doing: whether the diff the
assistant just wrote breaks the public API, whether the issue it is working on
belongs to another crate.
The assistant should see those answers in its next turn, and the user should be
able to read them back later.
Today the user pastes the JSON into the next query.

A conversation also carries config.
A conversation that picks a decision model with `--cfg decision.model.id=…`
should keep using it for its later decisions, as it does for its assistant
model.

## Design

### Asking

The positional argument is a `noul` question.

```sh
git diff --cached | jp decide 'Does this change alter the public API?'
```

```json
{"answer":{"type":"noul","noul":0.93}}
```

#### Questions

`--choice` and `--score` turn the positional question into a choice or a score
question.
`--choice` takes `name` or `name: description`; `--score` takes levels from
lowest to highest.
Each is given at least twice, as [RFD 133]'s types require.

```sh
jp decide 'Which crate owns this bug?' -a issue.md \
  --choice jp_cli --choice jp_llm --choice 'jp_config: config loading and merging'

jp decide 'How reproducible is this report?' -a issue.md \
  --score 'no steps' --score 'partial steps' --score 'complete steps'
```

Several questions go in one request through the schema DSL ([RFD 030]):

```sh
git diff | jp decide -s '
  breaking bool: Does this change break the public API?
  area "cli"|"llm"|"config": Which area does it touch?'
```

`-s` is parsed into JSON Schema by the same parser `jp query --schema` uses
(`jp_cli::schema`), and each top-level property becomes one question named after
it:

| Property                                                 | Question                     |
| -------------------------------------------------------- | ---------------------------- |
| `bool`                                                   | `noul`                       |
| an enum of strings (`"a"\|"b"`)                          | `choice` with those options  |
| `oneOf` of string `const`s with descriptions (JSON form) | `choice` with descriptions   |
| anything else                                            | an error naming the property |

A property's description becomes the question's instructions; a property without
one is asked by its name.
A schema means what the command running it makes of it: in `jp decide`, a
property's type picks its question type, and its answer is that question type's
answer object, never the bare value `jp query` would return.
The DSL has no spelling for `score`; `--score` asks a single score question.
The positional question and `-s` are mutually exclusive.

#### State

The state comes from stdin and attachments (`-a`).
One source is sent as a string.
Several are sent as an object: stdin under `input`, and each attachment under
its source.
Any attachment handler JP has can supply state, with no handler changes.

Stdin is sent as text even when it holds JSON.
A flag that sends JSON state can then be added later without changing what an
existing invocation sends.

A decision without state is valid when the question holds everything there is to
judge.
More often, a missing state is a mistake: nothing was piped, or `git diff
--cached` had nothing staged.
`jp decide` refuses to run without state, empty stdin included, unless
`--no-state` says it is meant.
State is never empty on the wire, which [RFD 133]'s types require because Ollama
rejects it, so `--no-state` sends the fixed text `(none)`, and the recorded
decision holds what was sent.

#### Output and exit status

Stdout is the answers, keyed by question name.
The positional question is named `answer`, so the shape is the same however many
questions were asked: `jq .answer.noul` and `jq .area.choice` both work.
Answers are data, so every format prints JSON: compact with `--format json` or
when piped, a JSON code block on a terminal, as `jp query --schema` shows
structured output.

With a single `noul` question, `--threshold P` makes the exit status the answer:
0 when the probability is at or above `P`, and 3 below it.

```sh
if git diff --cached | jp decide --threshold 0.8 \
    'Does this change need a CHANGELOG entry?' >/dev/null; then
  echo 'Add a CHANGELOG entry.'
fi
```

In JP, exit status 1 means an error and nothing else, and 2 is a usage error,
from clap and from plugin dispatch, which mirrors it.
A below-threshold answer takes 3, so neither kind of failure reads as "no".
`--threshold` with any other question set is a usage error.

### Which conversation

Without `-i`, `jp decide` finds its conversation the way `jp query` does: the
session's active conversation, then `conversation.default_id`.
Where `jp query` would open the picker, `jp decide` runs without a conversation:
a decision needs none, so there is nothing to choose.
Config then comes from files, environment, and `--cfg`, and nothing is recorded.

```sh
git diff | jp decide 'Does this change alter the public API?'       # the active conversation, if any
jp decide -i jp-c17… 'Which crate owns this bug?' -a issue.md --choice …
jp decide -i '?' 'Is this ready to merge?' < pr.md                  # the picker
```

- `-i <target>` resolves a conversation as `jp query -i` does.
- The conversation's config layer applies, and the decision is recorded in it.
- `-!` records nothing.
  It still applies the targeted conversation's config, as it does for `jp
  query`.
- With a conversation, `--cfg` is recorded as a config delta, as `jp query
  --cfg` records it.
  `jp decide -c decision.model.id=local/kev-4b` also changes the model for the
  conversation's later decisions.
  Without one, `--cfg` applies to that run only.
- Recording takes the conversation lock, as `jp query` does.

### `decision.conversation`

The conversation `jp decide` targets without `-i` is configurable:

```toml
[decision]
conversation = "none"
```

| Value              | Without `-i`                                                                              |
| ------------------ | ----------------------------------------------------------------------------------------- |
| `active` (default) | As above: `jp query`'s order, and no conversation where `jp query` would open the picker. |
| `required`         | `jp query`'s order, picker included. An error when nobody can answer the picker.          |
| `none`             | No conversation: config from files, environment, and `--cfg`, and nothing recorded.       |

- The option decides whether a conversation is loaded, so it cannot come from a
  conversation's own config.
  It is read before the conversation layer, from files, environment, and
  `--cfg`, and then removed from the config, as `conversation.default_id` is
  (`lib.rs:863-869`).
  It never appears in a recorded config delta.
- `JP_CFG_DECISION_CONVERSATION=none` overrides it for every run in an
  environment.
  For one run, `-i none` ([RFD 112]) runs without a conversation, as `--cfg
  decision.conversation=none` does.
- Plugins ignore it (see [Plugins](#plugins)).

### The `Decision` event

```rust
/// A decision recorded in a conversation.
pub struct Decision {
    /// The declared provider, and the model name the server reported.
    pub model: DecisionModelId,

    /// What the questions were about.
    pub state: Value,

    /// The questions, keyed by name.
    pub questions: IndexMap<String, Question>,

    /// The answers, keyed by question name.
    pub answers: IndexMap<String, Answer>,
}
```

- One event, not a request and response pair.
  A decision is atomic, and a pair would add a request-without-response state
  for `sanitize` to repair.
- `EventKind::Decision`, stored as `{"type": "decision", …}`.
  `encode_event` encodes `state` as it encodes `ToolCallResponse` content.
- `Question` and `Answer` are [RFD 133]'s types, so a stored decision has one
  shape whichever server answered it.
- `model` is the name the server reported, which is as specific as the server
  makes it: TypeSafe reports the version an alias such as `jev-latest` resolved
  to, and Ollama echoes the name requested.
- The event is turn-scoped.
  `jp decide` opens the pending turn ([RFD 134]) and appends the decision to it,
  so the decision becomes part of the next turn, before its request.

### What the model sees

A decision reaches the model as a resource on its turn's first `ChatRequest`.
[RFD 065] puts resources on the `ChatRequest` of the turn that attached them,
and has each provider render them at that request: Anthropic as document blocks,
OpenAI and Google as XML text, binary content natively.
Decisions use that path, so no provider has code for them.

`Thread::into_parts` applies projection, then keeps the provider-visible events
(`thread.rs:173-174`).
Between those two steps it turns each decision into one `Resource` on the first
`ChatRequest` of the decision's turn:

- `title` names the decision and the model that answered it.
- The text holds each question with its answer, then the state.
- `uri` identifies the decision event.

The resource exists only in the request JP builds.
On disk the decision stays a `Decision` event, and the `ChatRequest` holds only
the resources the user attached.
`is_provider_visible` does not change, and providers never see a `Decision`
event.
The rendered resource is the same on every request, so the prompt cache prefix
holds.

`sanitize` drops conversation events before the first `ChatRequest`, keeping
only config deltas and `TurnStart`s (`drop_leading_non_user_events`).
Decisions sit before their turn's request, so the first turn of a stream loses
its decisions wherever `sanitize` runs:

- a conversation whose first entry is a decision;
- a fork, which keeps the last N turns and sanitizes the stream it stores
  (`fork.rs:307`);
- window truncation, which drops the oldest events and sanitizes what is left
  (`window.rs`).

`drop_leading_non_user_events` keeps the decisions in the turn of the first
`ChatRequest`, and drops those of earlier turns, whose requests are gone.
A decision it keeps is never leading content for a provider, since it is
rendered into the request after it.

### Replay, fork, and compaction

A decision stays with the request it was recorded for, through the pending turn
alone:

```text
[TS ReqA RespA] [TS D1 ReqB RespB] [P D2]
```

- `jp query --replay` replays `B`: `[TS ReqA RespA] [TS D1 ReqB' RespB']`.
  `D1` was recorded for `B` and stays; `D2` was recorded after `B` and goes.
- `--fork 1` keeps `[TS D1 ReqB RespB] [P D2]`.
- A compaction of turn 1 covers `D1` with `B`.
  `D2` is pending and outside every range.

### Compaction policy

Compaction rules gain a `decisions` policy, next to `reasoning` and
`tool_calls`:

```toml
[[conversation.compaction.rules]]
decisions = { policy = "strip", over = "8KB" }
```

- `strip` drops the state and keeps the questions and answers.
  The state is what grows: a diff, a document.
- `omit` drops the decision.
- `over` limits the policy to decisions whose state is larger than the
  threshold, as it does for the other policies.

`Compaction` gains `decisions: Option<PolicySpec<DecisionPolicy>>`,
`affected_items` reports the decisions a threshold selects, and the `--compact`
DSL gains a `d` policy.
A summary absorbs the decisions in its range like any other event.
Turn transcripts, which summary generation reads, list each decision's questions
and answers without its state.

### Plugins

The `decide` request gains an optional `conversation`.
With it, the host applies that conversation's config, records the decision
there, and records `cfg` as a config delta, as `query` does.
Without it, the request touches no conversation, as [RFD 133] describes: a
plugin names its conversation or does without, whatever `decision.conversation`
says.

### Display

`jp conversation print` shows a decision inside its turn, before the request:
the model, each question with its answer, and the state cut to a few lines.
Before the next turn starts, it shows under the pending section.

### The assistant's own decisions

A builtin `decide` tool, if one is added, records no `Decision` event.
Its tool call response already carries the answer to the model; a `Decision`
event would show the model the answer twice.

### Built in

`jp decide` is built in rather than a command plugin: it needs attachment
resolution, conversation targeting, and `--cfg` parsing, which live in `jp_cli`
and are not exposed to plugins.
It and the plugin `decide` request call the same runner.

## Drawbacks

- The same `-s` text gives different shapes in the two commands: `breaking bool`
  answers `true` in `jp query` and a `noul` object in `jp decide`, and `area
  "cli"|"llm"` answers a string in one and a `choice` object in the other.
  A schema written for one command does not carry over to the other.
  Only part of the DSL maps to questions; the rest is an error.
- `--threshold` introduces exit status 3 for a negative result.
  `jp conversation grep` reports finding nothing as 1 until T-1fm7vp1 moves it
  to 3.
- The stdout shape and the exit codes are a contract from the first release.
- In a terminal with no active conversation, `jp decide` records nothing, so a
  `jp query` that follows does not see the decision.
- Targeting a conversation means both that its config applies and that the
  decision is recorded there.
  `-!` keeps the config and drops the recording; `-i none` drops both.
- A decision's state is resent with every later request until a compaction
  strips it.
  A large diff costs tokens on each of those turns, and `strip` is opt-in.
- The command cannot ship before [RFD 134] and [RFD 065]'s provider migration,
  the largest phase on that RFD's plan, because it records from its first
  release.

## Alternatives

**Decision models in `jp query`.** Simon Willison's `llm-typesafe` plugin does
this for `llm`: the system prompt is the question, and the answer comes back as
JSON text, one question per call.
`llm` treats every model as prompt-in, text-out; `jp query` is an agent loop.
The turn loop, tool coordinator, streaming renderer, interrupts, compaction, and
retries all assume a chat model, and each would need a decision branch.
The output would also change shape with the configured model: `jp query -s 'spam
bool'` prints `{"spam": true}` from an LLM and a probability from a decision
model, which breaks scripts when someone switches models.

**No conversation by default, with `-i .` to record.** A script or a new
terminal would never touch a conversation.
But the user would type `-i .` after every `jp query`, and `jp query` and `jp
decide` would read a missing `-i` differently.

**A `--record` flag.** A new flag for what `-i` already says, and it would leave
the question of whose config applies to a second flag.

**A flag to record without showing the model.** Recording is how the user says
the assistant should know.
Audit-only recording can come later if someone asks for it.

**`jp decide` as a first-party command plugin.** Keeps the core smaller and
exercises the plugin path.
It needs attachment resolution, conversation targeting, and `--cfg` parsing
delegated to plugins first, which the plugin protocol does not offer.

**An attachment, through a config delta.** `jp query -a` stores attachments in
`conversation.attachments` this way.
Attachments go in the first user message (Anthropic sends them as cached
document blocks), so the model would see the decision as known from the start,
and each new one would invalidate the cache from the first message on.
An attachment is a reference JP resolves again on every query; a decision has
nothing to resolve.

**A Global-scope event.** Survives pruning, as config deltas do.
`--replay` of `B` would then keep a decision recorded after `B`, and compaction
ranges could not reach it.

**Each decision its own turn.** No pending turn needed, but `--fork 1` right
after a decision keeps only the decision, and turns without a request show up
wherever turns are counted.

**Provider-visible `Decision` events.** Every provider's event conversion would
need a decision arm.
A resource on the request needs none.

**Decisions as text in the request.** `Thread::into_parts` could prepend an XML
block of the turn's decisions to the request's content, which needs no other
RFD.
It is a second way to put reference content in front of the model, next to [RFD
065]'s resources, and Anthropic would get text where it gets document blocks for
every other resource.

**Decisions stored as resources on the request.** When the pending turn starts,
its decisions would move into `ChatRequest.resources`.
The `sanitize` change goes away, but on disk a decision becomes a blob of text:
`print`, `grep`, and plugin `read_events` lose its typed questions and answers.

**Decisions stored after the turn's request.** `start_turn` would move the
pending decisions behind the request it appends, so none ever leads a stream and
`sanitize` needs no change.
Stream order would then contradict the decisions' timestamps.
[RFD 134]'s `start_turn` would have to know which pending entries to move, since
config deltas have to stay ahead of the request they configure.
And `jp query --no-edit` re-sends a failed request only when it is the last
event of its turn (`query.rs:929-937`); with a decision after it, `jp` sends
"continue" instead.

## Non-Goals

- **A flag that takes a System One request body.** It would cover what the flags
  don't, such as `noul` criteria and several score questions in one request.
  It can be added when someone needs it.
- **JSON state and inline state text.** Stdin and attachments cover the first
  release.
- **Recording during a running turn.** The lock prevents it from the command
  line, and a builtin tool answers through its tool call response.
- **A builtin `decide` tool.** Its own RFD.
- **Editing a recorded decision.** `jp conversation edit` covers it.

## Future Work

**Conversation content as state.** `jp decide` reads its state from stdin and
attachments, not from the conversation it records into.
Asking about the conversation itself, such as whether the last answer addressed
the question, needs a way to select which of its events become state.
A query language over conversation events is the likely fit.

**A temporary conversation when none is active.** Where `jp query` would open
the picker, both `jp query` and `jp decide` could start a temporary conversation
instead, so a decision in a new terminal reaches the query that follows it
(T-1fygs3m).
It changes `jp query`'s default too, so the two commands take it together.

## Risks and Open Questions

**Untrusted state in the prompt.** A decision over a web page or an issue body
puts that text into the next request, as an attachment does.
The resource marks it as decision state, but the model can still follow
instructions inside it.

**The rendered text.** How well models read probabilities from the resource's
text is untested.
The text is built per request and never stored, so it can change at the cost of
one uncached request per conversation.

**The placeholder state.** `--no-state` sends `(none)` as the state, and a model
reads it like any other text.
Whether it shifts answers compared with a question asked about nothing is
untested.

## Implementation Plan

1. **The event.** `EventKind::Decision`, its `TYPE_TAGS` entry, its
   `encode_event` arm, and the `sanitize` rule.
   Merges alone: nothing writes it.
2. **What the model sees.** Decisions as resources in `Thread::into_parts`,
   decisions in turn transcripts, and `print`.
   Depends on 1 and on [RFD 065] phase 4.
3. **Compaction.** The `decisions` policy on `Compaction`, the rule config, the
   `--compact` DSL, and `affected_items`.
   Depends on 1.
4. **`jp decide` for one `noul` question.** Positional question, stdin state and
   `--no-state`, JSON on stdout, and the conversation it records into: the
   default target, running without one, `-i`, the pending turn, the `--cfg`
   delta, and locking.
   Depends on 2, on [RFD 133] phase 3, which builds the runner, and on [RFD 134]
   phase 3.
   The command records from its first merge, so its default never changes.
5. **The rest of the command.** `--choice`, `--score`, `-s`, attachments, and
   `--threshold`.
   Depends on 4.
6. **`decision.conversation`.** Depends on 4.
7. **Plugins.** The `conversation` field on `decide`.
   Depends on 4.
8. **Documentation.** `jp decide`'s help and `decision.conversation` in the
   configuration guide.
   Depends on 6.

## References

- [Jev introduces a new shape of LLM][willison-jev], Simon Willison, with
  `llm-typesafe`
- [RFD 030]: Schema DSL
- [RFD 065]: Typed Resource Model for Attachments
- [RFD 112]: Hierarchy Targets and Explicit Selection Clearing
- [RFD 133]: Decision Models
- [RFD 134]: Pending Turn

[RFD 030]: 030-schema-dsl.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 112]: 112-hierarchy-targets-and-explicit-selection-clearing.md
[RFD 133]: 133-decision-models.md
[RFD 134]: 134-pending-turn.md
[willison-jev]: https://simonwillison.net/2026/Sep/21/jev/
