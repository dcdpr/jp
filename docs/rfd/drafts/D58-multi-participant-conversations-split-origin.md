# RFD D58: Multi-Participant Conversations (split origin)

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-06
- **Extends**: [RFD 076], [RFD 078]
- **Summary**: Proposes multi-participant conversations with named assistant
  configs, event authorship, and speaker-labeled provider projections.

> [!WARNING]
> This RFD is being split into four independently implementable RFDs:
>
> - [RFD D51]: Migrate `conversation.tools.*` to `assistant.tools.*`.
> - [RFD 098]: Link response events to their originating request `event_id`.
> - [RFD D53]: Parse inline attachment URIs from query prompts.
> - [RFD D54]: Multi-participant conversations.
>
> This document remains for historical context until all four RFDs are promoted,
> after which it can be removed.

## Summary

This RFD generalizes JP conversations from a single human and assistant to
multiple participants sharing one event stream.
Human participants are recorded through event authorship; assistant participants
are configured and activated under `conversation.participants` and receive
speaker-labeled provider projections.

## Motivation

Several JP workflows need multiple assistants in a single shared conversation.
In a pull-request review panel, for example, a reviewer assistant, a triager
assistant, and a human can deliberate in one conversation instead of maintaining
separate threads and relaying context by hand.

JP stores conversations as self-contained data: `base_config.json` captures the
initial configuration, and `events.json` records subsequent events and config
deltas.
Pulling a conversation from Git reproduces the exact snapshotted configuration
and event history.
Multi-participant state should fit this model rather than introducing a separate
metadata roster or sidecar files.

This RFD scopes the core design to:

- named assistant participants;
- event-level authorship, addressing, and response attribution;
- deterministic responder selection;
- speaker-labeled provider projections;
- no autonomous turn arbitration in core.

## Design

### Terms

A **Participant** is a named member of a conversation:

- **Human participants** are observed from `user.name` and recorded as the
  `author` of each `ChatRequest`.
  Humans are not listed in `conversation.participants`.
- **Assistant participants** are configured by an assistant config and activated
  by listing their identifier in `conversation.participants`.

Each assistant participant has a stable identifier used for addressing, config
paths, and event attribution:

```text
ParticipantIdentifier = [A-Za-z_][A-Za-z0-9_-]*
```

Entries in `conversation.participants`, keys under `assistants.<name>`,
`--invite`, `--at`, and inline `@name` mentions all use this grammar.
The identifier `assistant` is reserved for the default assistant backed by the
top-level `assistant.*` config and cannot appear under `assistants`; every other
assistant participant is backed by `assistants.<name>.*`.

`assistant.name` is a display label only and may contain whitespace and
punctuation.
Changing `assistant.name` does not rename the participant identifier.
If `assistant.name` is unset, displays fall back to the participant identifier;
when the two differ, terminal output shows both (for example, `Software
Architect (@architect)`) so the user can see the addressable identifier.

### Conversation config shape

A multi-participant conversation has the following logical configuration shape:

```toml
# Existing single-assistant config. Also backs participant "assistant".
[assistant]
name = "JP"
model.id = "opus"

# Active assistant participants in this conversation, in response order.
[conversation]
participants = ["assistant", "dev", "architect"]

# Captured config for named participants.
[assistants.dev]
name = "Dev"
model.id = "sonnet"

[assistants.architect]
name = "Architect"
model.id = "opus"
```

`conversation.participants` is the active assistant roster, ordered by the
deterministic response, render, and event-append sequence used for broadcast
requests.
When `conversation.participants` is absent, readers treat the roster as
`["assistant"]` for compatibility with existing conversations.

Each entry in `conversation.participants` must resolve to an assistant
configuration: `assistant` resolves to top-level `assistant.*`
(`assistants.assistant` is invalid), and every other name resolves to
`assistants.<name>.*`.
If a roster entry has no matching configuration, the resolved query
configuration is invalid; JP reports the missing participant and suggests
inviting it or removing it from the roster.
Raw inspection and repair commands can still load the conversation so users can
fix the configuration.

Runtime code must access participant configuration through a single helper
rather than branching on `assistant` at each call site.
Keeping top-level `assistant.*` alongside `assistants.<name>.*` preserves
`assistant.*` as the composable authoring shape and keeps existing
`base_config.json` snapshots valid without custom aliasing in v1.

New conversations default to `["assistant"]` unless `--invite` or `--at`
specifies an initial roster:

- `jp q --new "hello"` sets `conversation.participants = ["assistant"]`.
- `jp q --new --invite dev "hello"` sets `conversation.participants = ["dev"]`.
- `jp q --new --at dev "hello"` resolves, invites, and selects `dev`, setting
  `conversation.participants = ["dev"]`.
- `jp q --new --invite assistant --invite dev "hello"` sets
  `conversation.participants = ["assistant", "dev"]`.

Passing `--at` with `--new` replaces the default `["assistant"]` roster so a
conversation can start directly with a named participant:

```sh
jp q --new --at dev "hello world" # participants: ["dev"]
jp q "how are you?"              # broadcasts to ["dev"]
```

In an existing conversation, `--at` preserves the current roster and appends the
target participant if it is not already active:

```sh
jp q --new "hello world"         # participants: ["assistant"]
jp q --at dev "how are you?"     # participants: ["assistant", "dev"]
```

### Authoring reusable assistants

Reusable assistants are standard JP config files that configure `[assistant]`.
They require no new file format or inheritance syntax:

```toml
# .jp/config/assistants/dev.toml
extends = [
    "../knowledge/project-structure",
    "../knowledge/software-engineering",
    "../roles/dev",
]

[assistant]
name = "Dev"
model.id = "sonnet"
```

Shared knowledge fragments also remain unchanged:

```toml
# .jp/config/knowledge/project-structure.toml
[[assistant.system_prompt_sections]]
tag = "Project Structure"
content = """\
You are an expert at understanding the structure of the project.
"""

[[assistant.instructions]]
title = "Project Structure: Individual Crates"
items = [
    "The project is structured as a Cargo workspace, with all code in individual crates in `crates/`.",
    "Crates are organized into their logical domains, e.g. `jp_attachment`, `jp_conversation`, etc.",
    "The Rust code in `.config/jp/tools` is used for project maintenance tooling and is NOT part of the project's main codebase.",
]
```

Because shared fragments target `assistant.*`, both `assistants/dev.toml` and
`assistants/architect.toml` can extend the same fragment through the existing
config loader.

### Config composition is not invitation

Loading config files with `--cfg` (`-c`) composes them into the active
configuration without inviting new participants:

```sh
jp q -c dev -c architect "Please investigate this issue"
```

This merges both files into the top-level `assistant.*` configuration and
produces a single response from the reserved `assistant` participant.

Invitation is explicit: `--invite <name>`, `jp c invite <name>`, `--at <name>`,
or an allowed inline `@name` mention resolves a config source, extracts its
resolved `assistant.*` subtree into `assistants.<name>`, and adds `<name>` to
`conversation.participants`.
The same config file can be used either way, and `--invite` shares `--cfg`'s
source resolution rules (direct paths, `config_load_paths`, supported file
extensions, and `extends` chains).

### Inviting assistants

The `invite` command activates a participant in the conversation:

```sh
jp c invite dev
```

Inviting `dev` runs the following steps:

1. If `dev` is already in `conversation.participants`, do nothing.
2. If `assistants.dev` already exists in the conversation config, append `dev`
   to `conversation.participants`.
3. Otherwise, resolve `dev` as a config source (applying its `extends` chain),
   store the resolved `assistant.*` subtree at `assistants.dev`, and append
   `dev` to `conversation.participants` in the same config delta.

Inviting the reserved `assistant` participant adds `"assistant"` to
`conversation.participants` using the top-level `assistant.*` configuration,
without creating `assistants.assistant`.

When the source path does not match the desired participant identifier, or when
two sources share a file stem, the identifier and source path can be given
separately (`--cfg` is never used as the invite source syntax):

```sh
jp c invite reviewer --source personas/pr-reviewer
jp q --new --invite reviewer=personas/pr-reviewer "hello"
```

Both commands capture `personas/pr-reviewer` as participant `reviewer`.

### Uninviting and re-inviting

Uninviting removes a participant from `conversation.participants` while leaving
its captured `assistants.<name>` configuration intact:

```sh
jp c uninvite dev
```

Given the roster `["assistant", "dev", "architect"]`, the command writes a
config delta replacing the participant list:

```toml
[conversation]
participants = ["assistant", "architect"]
```

Re-inviting `dev` appends `"dev"` back to `conversation.participants` and reuses
the existing `assistants.dev` configuration, including any conversation-local
overrides.
Keeping `assistants.<name>` intact avoids tombstones, sidecar files, and
negative deltas, while config deltas and event attribution in the stream record
when participants joined, left, and spoke.

Reloading a captured participant from its source file is deferred to a future
command outside v1:

```sh
jp c participant refresh dev
```

### Assistant-facing tool configuration

Tool bindings belong to individual assistants rather than the conversation as a
whole.
Multi-participant conversations move `conversation.tools` into each assistant's
configuration:

```toml
[assistant.tools.fs_read_file]
source = "local"

[assistants.dev.tools.fs_modify_file]
source = "local"
run = "ask"
```

`providers.mcp` stays global because MCP server definitions describe shared
processes, while each participant captures the tool bindings it uses.
`ToolConfig` is not split into separate definition, behavior, display, and
access sub-objects; per-participant differences are expressed as distinct
captured tool entries.

New reusable assistant configs use `assistant.tools`, and captured participant
configs use `assistants.<name>.tools`.
Query setup, tool rendering, tool enable/disable flags, and `ToolCoordinator`
read the selected participant's tool configuration.
For backward compatibility, `conversation.tools` remains accepted as legacy
input: it maps to `assistant.tools` for the reserved `assistant` participant,
and to `assistants.<name>.tools` when inviting a named participant from a source
file.

### Relationship to tool configuration RFDs

This RFD extends the tool configuration model from [RFD 076] and [RFD 078] to
multiple assistant participants.

Runtime config writes from tools ([RFD 078]) are participant-relative: a tool
running for `dev` that writes `assistant.model.id` records the delta at
`assistants.dev.model.id`, whereas a tool running for the reserved `assistant`
participant writes to top-level `assistant.model.id`.
A tool cannot mutate another participant's configuration unless granted an
explicit absolute config path under [RFD 078]'s access model.

Sensitive tool-access paths ([RFD 076]) include the participant-scoped locations
alongside the legacy path while compatibility remains:

- `assistant.tools.*.access`;
- `assistants.*.tools.*.access`;
- `conversation.tools.*.access`.

### At-mention policy

Each assistant controls whether an inline `@name` mention can invite it into a
conversation or rejoin it after `uninvite`.
Both `invite` and `rejoin` default to `"error_if_known"`:

```toml
[assistant.at_mention]
invite = "error_if_known" # allow | error_if_known | ignore
rejoin = "error_if_known" # allow | error_if_known | ignore
```

In captured participant configs:

```toml
[assistants.architect.at_mention]
invite = "allow"
rejoin = "allow"

[assistants.dev.at_mention]
invite = "error_if_known"
rejoin = "error_if_known"
```

Inline mentions are lexed from `ChatRequest.content`:

- a mention starts at the beginning of the string or after whitespace;
- the mention begins with an unescaped `@`;
- the participant name matches `[A-Za-z_][A-Za-z0-9_-]*`;
- `\@name` is literal text;
- mid-word `@` characters such as `email@example.com` and `foo@bar` are not
  mentions.

When JP parses `@name` inside `ChatRequest.content`:

1. If `name` is active in `conversation.participants`, select it as a responder.
2. If `assistants.<name>` exists in the conversation config but `name` is not
   active, apply that assistant's `rejoin` policy.
3. If no captured config exists but `name` resolves as a config source, load the
   source and apply its `assistant.at_mention.invite` policy.
4. If neither a captured config nor a config source resolves, treat `@name` as
   plain text.

The `invite` and `rejoin` policies apply only after `name` resolves to a
captured participant or a loadable config source:

- `allow`: capture the config if needed, add `name` to
  `conversation.participants`, and select it as a responder;
- `error_if_known`: fail with a hint to run `jp c invite <name>`;
- `ignore`: leave the roster unchanged and treat `@name` as plain text.

The explicit `--at <name>` flag bypasses `assistant.at_mention`:

1. If `name` is active in `conversation.participants`, select it as a responder.
2. If `assistants.<name>` exists but `name` is not active, add it back to
   `conversation.participants` and select it.
3. If no captured config exists but `name` resolves as a config source, load and
   capture it, add it to `conversation.participants`, and select it.
4. If neither a captured config nor a config source resolves, fail with a hint
   to invite the participant or check the spelling.

Because inline mentions are parsed from `ChatRequest.content`, the mention must
appear inside the prompt string:

```sh
jp q "@dev please review this"
```

Passing `@dev` as a separate CLI argument is not a mention:

```sh
jp q @dev "please review this"
```

The explicit CLI flag form is:

```sh
jp q --at dev "please review this"
```

Inline `@name` mentions replace the `@path` query-text shorthand for file
attachments.
Inline file references use a `file:` prefix instead:

```sh
jp q "@dev please review file:README.md"
```

The `file:` token is parsed as an attachment candidate; future attachment
handlers may recognize additional inline URI schemes such as HTTP URLs or
`jp://` conversation links.

### Addressing and visibility

Addressing and visibility are separate axes:

- **Visibility** controls which assistant projections include the event.
  In v1, all events are visible to all assistant participants.
- **Addressing** controls which assistant participants are expected to respond.

Each `ChatRequest` records its human `author` and addressed responders under
`recipients.responders`:

```json
{
  "type": "chat_request",
  "author": "jean",
  "recipients": {
    "responders": [
      "dev"
    ]
  },
  "content": "@dev please review this"
}
```

When no `@name` mention or `--at <name>` flag selects responders, the request
broadcasts to all active assistant participants in `conversation.participants`
order.
When one or more responders are selected, only those participants respond,
though the request remains visible to all participants in v1.
Addressed responders run in explicit order: `--at` values in flag order,
followed by inline `@name` mentions in content order, deduplicated by first
occurrence.

Future private messages can extend `recipients` with projection visibility:

```json
{
  "recipients": {
    "responders": [
      "dev"
    ],
    "visibility": {
      "only": [
        "dev"
      ]
    }
  }
}
```

Humans are never listed in `recipients`.
Stored events are always visible to humans in the conversation store; visibility
metadata filters assistant provider projections only.

### Query behavior

A single `ChatRequest` addressed to `N` assistant participants produces one turn
(started by `TurnStart`, followed by the `ChatRequest` and `N` participant
execution substreams).

An unaddressed query broadcasts to every active assistant participant:

```sh
jp q "hello"
```

Responders execute sequentially in `conversation.participants` order.
Each responder's projection includes the `ChatRequest` and any responses already
produced earlier in the turn.
The terminal streams each response in turn, and events are appended in the same
order under the conversation lock ([RFD 020]).
Sequential execution lets later responders react to earlier responses in the
same turn; parallel responder execution changes those semantics and is left to
future work.

An addressed query runs only the selected responders:

```sh
jp q "@dev hello"
```

Here, only `dev` responds. v1 defines a single linear responder order; branching
plans (such as having `dev` and `architect` respond independently before
`reviewer` summarizes both) are deferred to future work.

### Participant execution scope

While answering a `ChatRequest`, an assistant participant produces a substream
of events.
Every participant-originated event carries `participant` (the identifier of the
assistant that produced it) and `request_id` (the stable `event_id` of the
`ChatRequest` being answered, from [RFD 097]).
These fields appear on `ChatResponse`, `ToolCallRequest`, `ToolCallResponse`,
and any `InquiryRequest` or `InquiryResponse` created during that participant's
tool execution.

Tool calls and inquiries are matched by `(participant, tool_call_id)` and
`(participant, inquiry_id)` rather than by ID alone.
Scoping provider-generated IDs to the participant prevents collisions and lets
projection distinguish a participant's own tool and inquiry events from a
peer's.

### Assistant-scoped CLI flags

Query flags that mutate assistant configuration (`--model`, `--param`, `--tool`,
`--no-tools`) apply only when the request resolves to a single assistant
responder.
JP resolves `--at <name>` and inline `@name` mentions into the responder set
before applying these flags.

When a single named responder is selected, the flag is allowed:

```sh
jp q --model gpt "@dev hello"
jp q --at dev --model gpt "hello"
```

Both commands mutate only `dev`:

```toml
[assistants.dev.model]
id = "gpt"
```

Selecting the reserved `assistant` participant:

```sh
jp q --at assistant --model gpt "hello"
```

mutates top-level `assistant`:

```toml
[assistant.model]
id = "gpt"
```

On a broadcast request with more than one active assistant participant, an
assistant-scoped flag is ambiguous:

```sh
jp q --model gpt "hello"
```

JP rejects the command:

```text
error: `--model` is ambiguous for a broadcast request
hint: use `--at dev --model gpt` or `-c @dev:model.id=gpt`
```

To target a specific participant via `--cfg`, the `@<participant>:<key>=<value>`
shorthand maps to that participant's captured config path
(`assistants.<name>.*`, or top-level `assistant.*` for `@assistant:`):

```sh
jp q -c @dev:model.parameters.reasoning.effort=max "@dev try again"
```

This assignment writes:

```toml
[assistants.dev.model.parameters.reasoning]
effort = "max"
```

The `--cfg` parser matches `@<participant>:<key>=<value>` before the leading-`@`
file-path syntax, so values such as `@config/dev.toml` remain file references.

### Event attribution

Attribution is recorded per event rather than per turn (`author` and
`recipients.responders` on `ChatRequest`, and `participant` and `request_id` on
assistant-originated events), because a single turn can contain multiple
requests addressing different participants.
JP stores attribution even in 1:1 conversations so history remains attributable
if additional participants join later, while strict 1:1 provider projections
omit speaker labels.

### Speaker-aware projection

Provider APIs expect a dyadic `user`/`assistant` message sequence, so JP builds
a separate projection of the event stream for each responding assistant
participant:

- The responding participant's own prior responses map to `assistant`.
- Every human request and peer assistant response maps to `user`.
- When the projection has more than one external speaker, `user` messages are
  prefixed in-band with `[<speaker>]:` so speaker identity survives if the
  provider merges consecutive `user` messages.
- Strict 1:1 projections omit speaker prefixes, keeping existing
  single-assistant conversations byte-for-byte identical at the provider
  boundary.
- Peer reasoning, tool calls, tool responses, and inquiry events are omitted.
  Only peer `ChatResponse` messages are shared across assistants; tool and
  inquiry internals remain visible in the stored conversation and in the
  producing participant's own projection.

For example, when projecting for `dev`:

```text
system:    <dev prompt>
user:      [jean]: should we refactor this?
assistant: yes, I would refactor it
user:      [architect]: I disagree; the boundary is still unstable
```

## Drawbacks

- **Two assistant config paths.** `assistant.*` remains the canonical authoring
  shape and backs the reserved `assistant` participant, while
  `assistants.<name>.*` backs captured named participants.
  Runtime code must isolate this split behind config accessors.
- **Verbose config deltas.** Capturing full assistant prompts and instructions
  into `assistants.<name>` produces large deltas in `events.json`.
  Readability should be addressed with inspection and editing tooling rather
  than sidecar files.
- **Conditional assistant-scoped flags.** Flags such as `--model` work for a
  single responder, but error on multi-assistant broadcasts unless written as
  `-c @<name>:...`.
- **Sequential broadcast latency.** Broadcasting to `K` assistant participants
  runs `K` model turns sequentially in v1 so later responders can react to
  earlier responses in the same turn.
- **Stateful invitation.** Inviting a participant resolves a config source,
  snapshots its `assistant.*` configuration, and updates the participant roster
  rather than simply appending a name.

## Alternatives

### Separate conversations with relay

Maintaining a separate conversation per assistant and relaying context through
an orchestrator keeps individual provider requests simple, but loses a single
authoritative event stream, duplicates context, and pushes speaker attribution
and projection into workflow code.
A relay workflow can still be built on top of this RFD, but it should not be
JP's core conversation model.

### Hierarchical sub-agents

[RFD 051] models delegation, where a primary agent dispatches scoped work to
sub-agents and receives summaries.
Delegation fits research and task fan-out, whereas this RFD models peer
assistants deliberating in a shared conversation.

### Store participants in metadata

Conversations already store a base config snapshot and ordered config deltas.
Participant membership and captured configurations belong in that self-contained
config state, not in `metadata.json`.

### Store participant snapshots in sidecar files

Mid-stream participant changes are ordered config mutations.
Sidecar files would require cross-file references, versioning, and separate
failure handling, whereas config deltas keep `events.json` authoritative.

### Store captured config under `conversation.participants.<name>.assistant`

Combining active membership and captured configuration into a single map entry
means uninviting a participant deletes its captured config unless tombstones or
negative deltas are introduced.
Separating active membership (`conversation.participants`) from captured config
(`assistants.<name>`) preserves conversation-local state across uninvite and
re-invite.

### Tombstone participant state

Marking removed participants with `state = "uninvited"` leaves inactive entries
in the participant map.
`conversation.participants` should list only active participants.

### Per-field conditions

Conditional config fields force shared fragments to know which participants
consume them, coupling orthogonal fragments as more assistants are added.
File-level `extends` keeps shared fragments independent.

### Group-transcript projection

Formatting the entire conversation history as a single transcript loses native
`assistant` message continuity, prompt caching, and tool-call structure for the
responding participant.

## Non-Goals

- **Autonomous arbitration.** Deciding when assistants speak or stay silent
  without a human request belongs in a plugin built on query delegation, not in
  v1 core.
- **Reply-order graphs.** v1 executes responders for a single request in linear
  order; branching and fan-out/fan-in plans are deferred.
- **Private messages.** `--dm <participant>` is deferred; all v1 events are
  visible to every assistant projection.
- **Deleting or refreshing captured configs.** Uninviting a participant
  preserves `assistants.<name>`, and reloading a captured config from its source
  file is deferred.
- **Wider `conversation.*` namespace cleanup.** Moving unrelated
  `conversation.*` fields is out of scope.

The `assistant.at_mention` policy and single-responder CLI flag scoping are
included because they complete the v1 query surface; if review stalls on either,
they can be split into follow-up RFDs without changing the storage or projection
model.

## Future Work

### Direct messages

A future `--dm <participant>` flag can address a request to a single assistant
participant while hiding both the request and its response from other assistant
projections:

```json
{
  "type": "chat_request",
  "author": "jean",
  "recipients": {
    "responders": [
      "dev"
    ],
    "visibility": {
      "only": [
        "dev"
      ]
    }
  },
  "content": "private question"
}
```

Stored events remain visible to humans; `recipients.visibility` filters
assistant provider projections only.

### Reply-order graph and parallel responses

A future RFD may generalize v1's linear responder list into a dependency graph
for branching, merging, or parallel execution.
Running responders in parallel reduces broadcast latency, but co-responders no
longer see each other's outputs until the next turn.

### Participant refresh

Re-inviting a participant reuses its captured `assistants.<name>` configuration
(and `-c @<name>:...` applies conversation-local overrides) so external source
edits do not silently change an existing conversation.
A future command may explicitly reload a captured participant from its source
file:

```sh
jp c participant refresh dev
```

## Risks and Open Questions

- **Tool migration scope.** Moving `conversation.tools` into assistant
  configuration touches query setup, tool rendering, CLI flags, and config
  documentation.
- **At-mention source resolution.** Checking whether an uncaptured `@name`
  mention resolves to a config source requires loading that source to read
  `assistant.at_mention.invite`.
  Source resolution results should be cached per command invocation.
- **Large config deltas.** Capturing full assistant configs inline reduces raw
  `events.json` readability, which should be mitigated with editor folding and
  JP tooling for viewing and editing event streams.
- **Projection quality.** Projecting peer responses as speaker-labeled `user`
  messages should be tested against real multi-assistant review conversations
  before acceptance.
- **Backwards compatibility.** Existing conversations without
  `conversation.participants` must continue to behave as single-assistant
  conversations backed by `assistant`.

## Implementation Plan

1. **Config schema and accessors.** Add `assistants` as a top-level map matching
   the shape of `assistant`, `conversation.participants` as an ordered list of
   assistant participant identifiers, `assistant.at_mention` policy, validation
   linking active participants to their configs, and accessor helpers that
   abstract over `assistant` vs. `assistants.<name>`.
   Depends on nothing.

2. **Stable event identifiers.** Implement or require [RFD 097] so `ChatRequest`
   events can be referenced by stable `event_id`.
   Depends on nothing in this RFD.

3. **Participant execution attribution.** Add `participant` and `request_id` to
   participant-originated chat, tool, and inquiry events, and key tool and
   inquiry matching by `(participant, id)`.
   Depends on Phases 1 and 2.

4. **Single-responder query path.** Resolve one assistant participant and run a
   query using that participant's assistant configuration.
   Depends on Phases 1 and 3.

5. **Tool migration and participant-aware tool execution.** Move tool bindings
   to `assistant.tools` / `assistants.<name>.tools` while accepting legacy
   `conversation.tools` input, and update query setup, tool rendering, tool
   flags, and `ToolCoordinator` to use the selected participant's tool config.
   Depends on Phases 1, 3, and 4.

6. **Invite and uninvite commands.** Implement `jp c invite <name>` (resolving
   config sources into `assistants.<name>` and appending `<name>` to
   `conversation.participants`) and `jp c uninvite <name>` (removing `<name>`
   from `conversation.participants`).
   Depends on Phases 1 and 5.

7. **Query responder resolution.** Parse inline `@name` mentions in
   `ChatRequest.content`, implement `--at`, enforce `assistant.at_mention`
   invite and rejoin policies, and compute the ordered responder set.
   Depends on Phases 3 and 6.

8. **Sequential broadcast execution.** Execute multiple responders sequentially
   within a single turn in responder order.
   Depends on Phases 3, 5, and 7.

9. **Speaker-aware projection.** Build per-participant projections with
   conditional speaker labels, omitting peer reasoning, tool calls, and
   inquiries.
   Depends on Phases 3 and 7.

10. **Assistant-scoped CLI flag scoping.** Apply assistant-mutating query flags
    to the single selected responder, reject them on broadcasts, and support `-c
    @<name>:<key>=<value>`.
    Depends on Phases 1 and 7.

11. **Documentation and tooling.** Document authoring and inviting reusable
    assistants, and add event-stream inspection and editing affordances for
    conversations with large config deltas.
    Depends on all prior phases.

## References

- [RFD 020]: conversation locks and concurrent mutation control.
- [RFD 031]: durable conversation storage and workspace projection.
- [RFD 051]: hierarchical sub-agent workflows, contrasted with peer
  participation.
- [RFD 054]: conversation config snapshots and deltas.
- [RFD 070]: future negative config deltas, not required by this RFD.
- [RFD 072]: command plugin system, relevant to future arbitration plugins.
- [RFD 076]: tool access grants, extended by participant-scoped tool paths.
- [RFD 078]: tool config mutation, extended by participant-relative config
  paths.
- [RFD 097]: stable event identifiers, required for `request_id` references.

[RFD 020]: ../020-parallel-conversations.md
[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 051]: ../051-sub-agent-workflows-with-local-tools.md
[RFD 054]: ../054-split-conversation-config-and-events.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 078]: ../078-tool-config-mutation.md
[RFD 097]: ../097-stable-event-identifiers.md
[RFD 098]: ./../098-request-response-event-linking.md
[RFD D51]: ./D51-assistant-scoped-tool-configuration.md
[RFD D53]: ./D53-inline-attachment-uri-parsing.md
[RFD D54]: ./D54-multi-participant-conversations.md
