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
When an active conversation exists, `jp decide` applies that conversation's
configuration and records the decision in the conversation's pending turn ([RFD
134]), so the next `jp query` includes the decision with its request; when no
conversation is active, `jp decide` runs standalone without recording.
Recorded decisions stay attached to their turn's request across replay, fork,
and compaction.

## Motivation

Shell scripts built around JP often make small classifications: whether a staged
diff needs a changelog entry, which crate an issue belongs to, or whether a bug
report has enough detail to reproduce.
Without a dedicated subcommand, each classification requires a `jp query
--schema` turn, which runs the full agent loop with seconds of latency and
returns a value with no probability.
[RFD 133] adds decision models and a `decide` protocol request for plugins, but
exposes no CLI command for shell scripts or interactive terminal sessions.

Many decisions also evaluate work in an active conversation, such as checking
whether a diff the assistant just produced breaks the public API or whether the
current issue belongs to another crate.
The assistant needs those results on its next turn, and the user should be able
to inspect them later in the conversation history rather than pasting JSON
output into the next query by hand.

Running inside a conversation also lets `jp decide` inherit conversation-scoped
configuration: a conversation that sets `--cfg decision.model.id=…` continues
to use that model for subsequent decisions, just as it does for
`assistant.model`.

## Design

### Asking

The positional argument asks a single `noul` question:

```sh
git diff --cached | jp decide 'Does this change alter the public API?'
```

```json
{"answer":{"type":"noul","noul":0.93}}
```

#### Questions

Passing `--choice` or `--score` turns the positional argument into a choice or
score question.
`--choice` accepts `name` or `name: description`, and `--score` accepts rubric
levels ordered from lowest to highest.
Both flags must be passed at least twice, matching the minimum option count in
[RFD 133].

```sh
jp decide 'Which crate owns this bug?' -a issue.md \
  --choice jp_cli --choice jp_llm --choice 'jp_config: config loading and merging'

jp decide 'How reproducible is this report?' -a issue.md \
  --score 'no steps' --score 'partial steps' --score 'complete steps'
```

To ask multiple questions in a single request, pass a schema in the `-s` DSL
([RFD 030]):

```sh
git diff | jp decide -s '
  breaking bool: Does this change break the public API?
  area "cli"|"llm"|"config": Which area does it touch?'
```

`jp decide -s` parses the schema into JSON Schema using the same parser as `jp
query --schema` (`jp_cli::schema`), mapping each top-level property to a
question named after the property:

| Property                                                 | Question                    |
| -------------------------------------------------------- | --------------------------- |
| `bool`                                                   | `noul`                      |
| String enum (`"a"\|"b"`)                                 | `choice` with those options |
| `oneOf` of string `const`s with descriptions (JSON form) | `choice` with descriptions  |
| Any other schema type                                    | Error naming the property   |

Each property's description becomes the question's `instructions`; if a property
has no description, the property name is used as `instructions`.
In `jp decide`, a schema property's type selects the question type and its
output is the full answer object for that question type rather than the bare
scalar `jp query --schema` returns.
Because the schema DSL has no syntax for ordered rubrics, `score` questions are
only asked via `--score`.
The positional question argument and `-s` are mutually exclusive.

#### State

The decision state comes from stdin and attachments (`-a`).
When a single input source is provided, the state is sent as a string; when
multiple sources are provided, the state is sent as a JSON object with stdin
under `input` and each attachment keyed by its source URI.
All existing attachment handlers work without modification.

Stdin is always sent as a string, even when its content is valid JSON, so a
future flag for structured JSON state can be added without changing the behavior
of existing commands.

Although a decision without external state is meaningful when the question is
self-contained, an empty state on the CLI usually indicates a mistake (such as
an unpiped command or an empty `git diff --cached`).
`jp decide` therefore rejects missing or empty state unless `--no-state` is
passed explicitly.
Because [RFD 133] forbids a blank string state on the wire (which Ollama
rejects), `--no-state` sends the literal string `"(none)"` and records that
value in the `Decision` event.

#### Output and exit status

`jp decide` writes the answers to stdout as a JSON object keyed by question
name.
A positional question is named `answer`, keeping the top-level JSON structure
uniform regardless of how many questions were asked (`jq .answer.noul` and `jq
.area.choice` follow the same path).
Output is always JSON: compact JSON with `--format json` or when stdout is
piped, and a fenced JSON code block on an interactive terminal, matching `jp
query --schema`.

When asking a single `noul` question, `--threshold P` maps the answer to the
process exit status: `0` when the probability is at or above `P`, and `3` when
it is below `P`.

```sh
if git diff --cached | jp decide --threshold 0.8 \
    'Does this change need a CHANGELOG entry?' >/dev/null; then
  echo 'Add a CHANGELOG entry.'
fi
```

In JP, exit status `1` indicates a runtime error and `2` indicates a CLI usage
error (from `clap` and plugin dispatch).
Using exit status `3` for a below-threshold answer keeps negative decision
outcomes distinct from execution and usage errors.
Passing `--threshold` with multiple questions or with a `choice` or `score`
question is a usage error (exit status `2`).

### Which conversation

Without `-i`, `jp decide` resolves its conversation in the same order as `jp
query`: the session's active conversation first, then `conversation.default_id`.
Where `jp query` would fall back to the interactive conversation picker, `jp
decide` instead runs without a conversation: configuration is loaded from files,
environment variables, and `--cfg` flags, and no decision is recorded.

```sh
git diff | jp decide 'Does this change alter the public API?'       # the active conversation, if any
jp decide -i jp-c17… 'Which crate owns this bug?' -a issue.md --choice …
jp decide -i '?' 'Is this ready to merge?' < pr.md                  # the picker
```

- `-i <target>` resolves a target conversation using the same grammar as `jp
  query -i`.
- When a conversation is selected, its configuration layer applies and the
  decision is recorded in its stream.
- `-!` (`--no-persist`) applies the targeted conversation's configuration
  without recording the decision or any `--cfg` delta, matching `jp query -!`.
- When a conversation is persisted, `--cfg` overrides are recorded as a config
  delta just like `jp query --cfg`, so `jp decide -c
  decision.model.id=local/kev-4b` also updates the decision model for subsequent
  decisions in that conversation.
  When running without a conversation, `--cfg` applies to that invocation only.
- Recording a decision acquires the conversation lock, matching `jp query`.

### `decision.conversation`

`decision.conversation` configures how `jp decide` resolves a conversation when
`-i` is omitted:

```toml
[decision]
conversation = "none"
```

| Value              | Behavior without `-i`                                                                              |
| ------------------ | -------------------------------------------------------------------------------------------------- |
| `active` (default) | Uses `jp query`'s resolution order, running without a conversation where `jp query` opens a picker |
| `required`         | Uses `jp query`'s resolution order including the picker, and errors if the picker cannot prompt    |
| `none`             | Runs without a conversation (config from files, environment, and `--cfg`; nothing recorded)        |

- Because `decision.conversation` determines whether a conversation is loaded at
  all, it cannot come from a conversation's own config delta.
  JP reads `decision.conversation` from files, environment variables, and
  `--cfg` before loading the conversation layer, then strips it from the partial
  config so it is never written into a conversation config delta, matching
  `conversation.default_id` (`lib.rs:863-869`).
- Setting `JP_CFG_DECISION_CONVERSATION=none` disables conversation loading for
  every invocation in that environment.
  For a single invocation, `-i none` ([RFD 112]) or `--cfg
  decision.conversation=none` runs without a conversation.
- Plugin `decide` requests ignore `decision.conversation` (see
  [Plugins](#plugins)).

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

- Each decision is stored as a single `EventKind::Decision` event (`{"type":
  "decision", …}`) rather than a request/response pair.
  A decision is atomic, and a split pair would introduce a
  request-without-response state for `sanitize` to repair.
- `encode_event` encodes `state` using the same representation as
  `ToolCallResponse` content.
- `Question` and `Answer` reuse the types from [RFD 133], giving stored
  decisions a uniform schema across providers.
- `model` stores the declared provider name and the model identifier returned by
  the server (TypeSafe resolves aliases such as `jev-latest` to the concrete
  model version, whereas Ollama echoes the requested model name).
- `Decision` is turn-scoped.
  `jp decide` opens the pending turn ([RFD 134]) and appends the `Decision`
  event to it, placing the decision at the start of the next turn ahead of its
  first `ChatRequest`.

### What the model sees

A recorded decision reaches the assistant model as a `Resource` on the first
`ChatRequest` of its turn.
Under [RFD 065], resources are attached to a turn's `ChatRequest` and rendered
by each LLM provider (as document blocks on Anthropic, XML text on OpenAI and
Google, or native binary content).
Reusing `Resource` requires no provider-specific code for decisions.

`Thread::into_parts` applies projection and filters for provider-visible events
(`thread.rs:173-174`).
Between those two steps, it converts each `Decision` event into a `Resource` on
the first `ChatRequest` of that turn:

- `title` names the decision and the model that answered it.
- The text body lists each question and its answer, followed by the state.
- `uri` identifies the source `Decision` event.

This `Resource` exists only in the in-memory provider request built by
`Thread::into_parts`.
On disk, the decision remains a `Decision` event and `ChatRequest` stores only
user-attached resources.
`is_provider_visible` is unchanged, providers never receive `Decision` events
directly, and the rendered resource text is deterministic across requests so the
prompt cache prefix remains stable.

`sanitize` drops conversation events that precede the stream's first
`ChatRequest`, keeping only config deltas and `TurnStart` entries
(`drop_leading_non_user_events`).
Because `Decision` events sit before their turn's first `ChatRequest`, an
unmodified `sanitize` pass would drop the first turn's decisions in three
places:

- A conversation whose first recorded entry is a decision.
- A fork, which retains the last `N` turns and sanitizes the resulting stream
  (`fork.rs:307`).
- Window truncation, which drops the oldest events and sanitizes the remainder
  (`window.rs`).

`drop_leading_non_user_events` is updated to retain `Decision` events that
belong to the turn of the first `ChatRequest` while dropping `Decision` events
from earlier turns whose `ChatRequest` was removed.
Retained decisions never appear as leading provider messages because
`Thread::into_parts` folds them into the `ChatRequest` that follows them.

### Replay, fork, and compaction

Because `jp decide` writes into the pending turn ([RFD 134]), recorded decisions
stay attached to the `ChatRequest` they precede across stream operations:

```text
[TS ReqA RespA] [TS D1 ReqB RespB] [P D2]
```

- `jp query --replay` replays turn `B`, producing `[TS ReqA RespA] [TS D1 ReqB'
  RespB']`: `D1` was recorded for `B` and is kept, while `D2` was recorded after
  `B` and is discarded.
- `--fork 1` retains `[TS D1 ReqB RespB] [P D2]`.
- Compacting turn 1 includes `D1` alongside `B`, while `D2` remains in the
  pending section outside every compaction range.

### Compaction policy

Compaction rules gain a `decisions` policy alongside `reasoning` and
`tool_calls`:

```toml
[[conversation.compaction.rules]]
decisions = { policy = "strip", over = "8KB" }
```

- `strip` removes `state` (which holds large payloads such as diffs or
  documents) while preserving `questions` and `answers`.
- `omit` removes the entire `Decision` event.
- `over` restricts the policy to decisions whose serialized `state` exceeds the
  byte threshold, matching the other compaction policies.

`Compaction` gains `decisions: Option<PolicySpec<DecisionPolicy>>`,
`affected_items` reports decisions matched by a threshold, and the `--compact`
DSL adds a `d` selector.
Summary compaction absorbs `Decision` events in its turn range like any other
turn event.
Turn transcripts used for summary generation include each decision's questions
and answers while omitting its `state`.

### Plugins

The plugin `decide` request ([RFD 133]) gains an optional `conversation` field.
When `conversation` is set, the host applies that conversation's configuration,
records the `Decision` event in its pending turn, and persists any `cfg`
overrides as a config delta, matching `query`.
When `conversation` is omitted, the request neither reads nor writes a
conversation regardless of `decision.conversation`.

### Display

`jp conversation print` renders each `Decision` inside its turn ahead of the
`ChatRequest`, showing the model, each question and answer, and the state
truncated to a few lines.
Before the next turn starts, pending decisions appear under the `pending`
section.

### The assistant's own decisions

If a built-in `decide` tool is added in the future, its executions will not
record `Decision` events: the `ToolCallResponse` already delivers the answer to
the model, and recording a `Decision` event as well would duplicate the answer
in the prompt.

### Built in

`jp decide` is implemented as a built-in CLI command rather than a command
plugin because it depends on attachment resolution, conversation targeting, and
`--cfg` parsing, which live in `jp_cli` and are not exposed over the plugin
protocol.
Both `jp decide` and the plugin `decide` request handler invoke the same
underlying runner.

## Drawbacks

- **Divergent output shapes for the same `-s` schema.** Passing `breaking bool`
  to `-s` returns a bare boolean (`true`) in `jp query` and a `noul` answer
  object in `jp decide`; passing `area "cli"|"llm"` returns a string in `jp
  query` and a `choice` answer object in `jp decide`.
  Schemas are therefore not interchangeable between the two commands, and any
  schema property outside `bool` and string enums fails in `jp decide`.
- **New exit status convention.** `--threshold` introduces exit status `3` for a
  below-threshold result, whereas `jp conversation grep` returns `1` when no
  matches are found until T-1fm7vp1 updates it to `3`.
- **Immediate compatibility surface.** Both the JSON output schema and the exit
  status conventions become public contracts for scripts from the first release.
- **No recording in terminals without an active conversation.** When a terminal
  session has no active conversation, `jp decide` runs standalone and records
  nothing, so a subsequent `jp query` in that terminal does not receive the
  decision.
- **Coupled configuration and persistence on conversation targets.** Targeting a
  conversation both applies its configuration layer and records the decision
  into its stream: `-!` keeps the conversation's configuration while skipping
  persistence, and `-i none` skips both.
- **Repeated state tokens until compaction.** A decision's `state` is re-sent
  with every subsequent turn until a compaction rule strips it; large states
  (such as diffs) consume tokens on every turn unless `strip` is configured.
- **Dependency on [RFD 134] and [RFD 065].** Because `jp decide` records
  decisions into the active conversation from its first release, it cannot ship
  until [RFD 134] and [RFD 065]'s provider migration land.

## Alternatives

**Support decision models inside `jp query`.** Rejected.
Simon Willison's `llm-typesafe` plugin takes this approach for `llm`, using the
system prompt as the question and returning JSON text for one question per call.
Unlike `llm`, which models every call as prompt-in/text-out, `jp query` drives a
multi-step agent loop: its turn loop, tool coordinator, streaming renderer,
interrupt handling, compaction, and retry logic all assume a chat model and
would each require a decision-model branch.
Sharing `jp query` would also make `-s` output depend on the configured model
(`jp query -s 'spam bool'` would print `{"spam": true}` with an LLM and a
probability object with a decision model), breaking scripts when switching
models.

**Default to no conversation and require `-i .` to record.** Rejected.
Although scripts and fresh terminals would never touch a conversation by
default, interactive users would have to pass `-i .` on every decision, and `jp
query` and `jp decide` would interpret a missing `-i` flag differently.

**Add a `--record` flag.** Rejected.
`-i` already selects a conversation, and adding `--record` would require a
separate flag to control whether the conversation's configuration applies when
`--record` is omitted.

**Add a flag to record a decision without exposing it to the assistant.**
Rejected for the initial design.
Recording a decision in a conversation is how the user shares it with the
assistant; audit-only recording can be added later if needed.

**Implement `jp decide` as a first-party command plugin.** Rejected.
A plugin implementation would keep `jp_cli` smaller and exercise the plugin
protocol, but `jp decide` requires attachment resolution, conversation
targeting, and `--cfg` parsing, none of which are exposed to command plugins.

**Store decisions as conversation attachments via config deltas.** Rejected.
`jp query -a` stores attachments in `conversation.attachments` via config
deltas, which providers place on the first user message (for example, as cached
document blocks on Anthropic).
Using config-delta attachments would present every decision as if it existed at
the start of the conversation and would invalidate the prompt cache from the
first message whenever a new decision was recorded.
Attachments are also re-resolved on every turn, whereas a completed decision is
immutable.

**Record `Decision` as a Global-scope entry.** Rejected.
Global entries preserve stream position and survive turn pruning, which means
`--replay` of turn `B` would retain decisions recorded after `B`, and compaction
ranges would skip them.

**Give each decision its own turn.** Rejected.
Creating a turn per decision avoids [RFD 134]'s pending turn, but `--fork 1`
immediately after a decision would retain only the decision without the
preceding chat turn, and requestless turns would inflate turn counts everywhere.

**Make `Decision` events provider-visible.** Rejected.
Making `Decision` provider-visible would require every `jp_llm` provider to
implement custom `Decision` formatting, whereas converting decisions into
`Resource` entries in `Thread::into_parts` reuses each provider's existing
resource rendering.

**Prepend decisions as XML text in `ChatRequest` content.** Rejected.
Having `Thread::into_parts` prepend an XML block directly to the request text
avoids depending on [RFD 065], but introduces a parallel mechanism for attaching
reference material to a request and sends plain text to Anthropic where [RFD
065] sends native document blocks.

**Move pending decisions into stored `ChatRequest.resources` when a turn
starts.** Rejected.
Moving decisions into `ChatRequest.resources` at `start_turn` avoids changing
`sanitize`, but flattens each decision into an untyped text resource on disk so
`print`, `grep`, and plugin `read_events` lose the structured `Question` and
`Answer` fields.

**Move pending decisions after the turn's `ChatRequest` when a turn starts.**
Rejected.
Moving pending decisions behind the newly appended `ChatRequest` avoids changing
`sanitize`, but places the `Decision` entries out of timestamp order, forces
[RFD 134]'s `start_turn` to distinguish decisions from config deltas (which must
remain ahead of the request), and breaks `jp query --no-edit` retry detection
(`query.rs:929-937`), which only re-sends a failed `ChatRequest` when it is the
final event of its turn.

## Non-Goals

- **A flag accepting a raw System One request body.** A raw-request flag would
  expose features outside the CLI flags (such as `noul` criteria and multiple
  `score` questions in one request) and can be added later if needed.
- **Structured JSON state and inline state flags.** Stdin and `-a` attachments
  cover the initial release.
- **Recording decisions into a conversation while a turn is running.** The
  conversation lock prevents concurrent CLI writes during a turn, and any future
  built-in `decide` tool returns its answer through `ToolCallResponse`.
- **A built-in `decide` tool for the assistant.** Deferred to a separate RFD.
- **Dedicated subcommands for editing a recorded decision.** `jp conversation
  edit` already edits stream events.

## Future Work

**Selecting conversation events as decision state.** `jp decide` reads state
from stdin and `-a` attachments rather than from the target conversation's
history.
Evaluating the conversation itself (for example, checking whether the last
assistant response answered the user's question) requires a selector or query
language over conversation events.

**Starting a temporary conversation when none is active.** Where `jp query`
currently opens the conversation picker, both `jp query` and `jp decide` could
instead start a temporary conversation so a decision run in a fresh terminal
automatically reaches the `jp query` that follows it (T-1fygs3m).
Because that change alters `jp query`'s default behavior as well, it is tracked
separately for both commands together.

## Risks and Open Questions

**Prompt injection through decision state.** Recording a decision over untrusted
input (such as a web page or issue body) places that state text into the next
`ChatRequest` alongside user attachments.
Although the rendered `Resource` labels the content as decision state, the
assistant model can still follow instructions embedded in untrusted text.

**Formatting probabilities for the assistant model.** How effectively assistant
models interpret probability and confidence values in the rendered `Resource`
text has not yet been tested.
Because `Thread::into_parts` builds the `Resource` text in memory per request
rather than persisting it on disk, the format can be adjusted later at the cost
of a single prompt-cache miss per conversation.

**Effect of the `"(none)"` placeholder state.** `--no-state` sends `"(none)"` as
the state string, which the decision model evaluates like any other state text.
Whether `"(none)"` shifts probabilities compared to a server-native empty state
is untested.

## Implementation Plan

1. **The `Decision` event.** Add `EventKind::Decision`, its `TYPE_TAGS` entry,
   its `encode_event` arm, and the `drop_leading_non_user_events` rule in
   `sanitize`.
   This phase is independently mergeable because nothing writes `Decision`
   events yet.
2. **Provider rendering and display.** Convert `Decision` events into `Resource`
   entries in `Thread::into_parts`, include decisions in turn transcripts, and
   render `Decision` events in `jp conversation print`.
   This phase depends on Phase 1 and [RFD 065] Phase 4.
3. **Compaction.** Add the `decisions` policy to `Compaction`, the compaction
   rule configuration, the `d` selector in the `--compact` DSL, and
   `affected_items` accounting.
   This phase depends on Phase 1.
4. **`jp decide` for a single `noul` question.** Implement the positional
   question argument, stdin state and `--no-state`, JSON output on stdout, and
   conversation integration (default target resolution, standalone execution
   when no conversation is active, `-i`, pending turn recording, `--cfg` delta
   persistence, and conversation locking).
   This phase depends on Phase 2, [RFD 133] Phase 3 (the decision runner), and
   [RFD 134] Phase 3.
5. **Remaining CLI flags.** Add `--choice`, `--score`, `-s`, `-a` attachments,
   and `--threshold`.
   This phase depends on Phase 4.
6. **`decision.conversation` configuration.** Add `decision.conversation` and
   pre-conversation layer extraction.
   This phase depends on Phase 4.
7. **Plugin conversation support.** Add the optional `conversation` field to the
   plugin `decide` request.
   This phase depends on Phase 4.
8. **Documentation.** Add CLI help for `jp decide` and document
   `decision.conversation` in the configuration guide.
   This phase depends on Phase 6.

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
