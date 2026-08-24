# RFD D58: Multi-Participant Conversations

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-06
- **Requires**: [RFD 097]
- **Extends**: [RFD 076], [RFD 078]

> [!WARNING]
> This RFD is in the process of being split up into several smaller RFDs
> valuable on their own and independently implementable:
>
> - [RFD D51]: migrating `conversation.tools.*` to `assistant.tools.*`
> - [RFD 098]: explicit `event_id` reference from each response to request event
> - [RFD D53]: parse inline URIs from query prompts
> - [RFD D54]: The main RFD implementing multi-participant conversations
>
> This RFD is kept for historical purposes *until* all four of the above RFDs
> are promoted, after which this one can be removed.

## Summary

This RFD generalizes JP conversations from one human and one assistant to many
participants sharing one conversation.
Humans are observed through event authorship; assistant participants are
captured in the conversation config and activated through
`conversation.participants`.
The design preserves the current single-assistant behavior, keeps reusable
assistant authoring as normal config files, and projects the shared conversation
into each assistant's provider request with explicit speaker labels.

## Motivation

Several JP workflows need more than one assistant voice in one shared context.
A pull-request panel is the motivating example: a reviewer assistant, a triager
assistant, and the human should deliberate in one room instead of maintaining
separate conversations and manually relaying context.

JP also stores conversations as self-contained data: `base_config.json` captures
the conversation's starting config, and `events.json` carries later events and
config deltas.
When a teammate pulls a conversation from Git, they get the same snapshotted
config and deltas.
Multi-participant state should use that existing model rather than adding a
metadata roster or sidecar store.

The goal is a narrow core:

- named assistant participants;
- event-level authorship, addressing, and response attribution;
- deterministic responder selection;
- a provider projection that preserves who said what;
- no autonomous arbitration in core.

## Design

### Terms

A **Participant** is a named member of a conversation.
There are two kinds:

- **Human participants** are observed.
  Their identity comes from `user.name`, stored as the `author` of each
  `ChatRequest`.
  Humans are not configured in `conversation.participants`.
- **Assistant participants** are configured.
  Their behavior is stored as an assistant config and their active membership is
  listed in `conversation.participants`.

The name `assistant` is reserved for the existing single assistant backed by the
top-level `assistant.*` config.
Named assistant participants other than `assistant` are backed by
`assistants.<name>.*`.

The participant identifier is the stable identity used for addressing, config
paths, and event attribution.
Participant identifiers must match:

```text
ParticipantIdentifier = [A-Za-z_][A-Za-z0-9_-]*
```

`conversation.participants` entries, `assistants.<name>` keys, `--invite`,
`--at`, and inline `@name` mentions all use this grammar.
The identifier `assistant` is reserved and cannot appear under `assistants`.

`assistant.name` is a display label only.
It may contain whitespace and punctuation.
If `assistant.name` is unset, displays fall back to the participant identifier.
Renaming `assistant.name` does not rename the participant.

Terminal rendering shows both when they differ, for example `Software Architect
(@architect)`, so the human can discover the addressable participant name.

### Conversation config shape

The final logical config shape is:

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

`conversation.participants` is the current active assistant roster.
Array order is significant: it defines deterministic response, render, and
append order for broadcast requests.
The array contains assistant participant names only.
Humans remain event authors, not config entries.

If `conversation.participants` is absent, readers treat the roster as
`["assistant"]` for compatibility with existing conversations.
`assistants.assistant` is invalid; the reserved `assistant` participant always
maps to top-level `assistant.*`.

New conversations follow the same rule unless participants are explicitly
invited:

- `jp q --new "hello"` creates `conversation.participants = ["assistant"]`.
- `jp q --new --invite dev "hello"` creates `conversation.participants =
  ["dev"]`.
- `jp q --new --at dev "hello"` resolves, invites, and selects `dev`, creating
  `conversation.participants = ["dev"]`.
- `jp q --new --invite assistant --invite dev "hello"` creates
  `conversation.participants = ["assistant", "dev"]`.

Explicit `--at` during conversation creation replaces the implicit assistant
roster.
This supports starting directly with a named assistant participant:

```sh
jp q --new --at dev "hello world" # participants: ["dev"]
jp q "how are you?"              # broadcasts to ["dev"]
```

Continuing an existing conversation with `--at` adds the participant when needed
and preserves the existing roster:

```sh
jp q --new "hello world"         # participants: ["assistant"]
jp q --at dev "how are you?"     # participants: ["assistant", "dev"]
```

A name in `conversation.participants` must resolve to config:

- `assistant` resolves to `assistant.*`;
- every other name resolves to `assistants.<name>.*`.

A resolved query config is invalid if a participant name has no assistant
config.
JP reports the broken name and suggests inviting it or removing it from the
roster.
Raw inspection and repair commands may still load the stored conversation so
users can fix the config.

Runtime code must access assistant participant config through a single helper,
not by open-coding the `assistant` branch at each call site.
The reserved `assistant` participant seam is accepted to preserve `assistant.*`
as the canonical composable authoring shape and to keep existing
`base_config.json` snapshots valid without introducing custom aliasing in v1.

### Authoring reusable assistants

Reusable assistants are normal JP config files shaped around `[assistant]`.
There is no new file type and no special inheritance syntax.

Example:

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

A shared knowledge fragment remains unchanged:

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

Both `assistants/dev.toml` and `assistants/architect.toml` can extend the same
fragment.
The fragment stays generic because it targets `assistant.*`, the same shape the
config loader already understands.

### Config composition is not invitation

Loading config with `--cfg` does not invite participants.

A command like:

```sh
jp q -c dev -c architect "Please investigate this issue"
```

continues to compose both config files into the current assistant configuration,
exactly as it does today.
It produces one assistant response from the reserved `assistant` participant,
not a multi-participant conversation.

Invitation is explicit.
`--invite <name>`, `jp c invite <name>`, `--at <name>`, or an allowed inline
`@name` mention resolves a config source and captures its assistant-facing
configuration under a participant identifier.

The same config file can be used either way: `--cfg` composes it into the
current effective config; invite extracts its resolved `assistant.*` subtree and
captures it as a named participant.

### Inviting assistants

Inviting resolves a normal config source, extracts the resolved assistant-facing
config, and captures it into the conversation config.
`--invite` uses the same config-source resolution as `--cfg`: direct paths,
configured `config_load_paths`, supported extensions, and `extends` behave the
same.
The difference is what JP does with the resolved config.
`--cfg` merges it into the current effective config.
`--invite` extracts the resolved `assistant.*` subtree, captures it under a
participant name, and adds that participant to `conversation.participants`.

```sh
jp c invite dev
```

Resolution:

1. If `dev` is already in `conversation.participants`, the command is a no-op.
2. If `assistants.dev` already exists, append `dev` to
   `conversation.participants`.
3. Otherwise resolve `dev` as a normal config source through the existing config
   load paths, applying its `extends` chain.
4. Extract the resolved assistant-facing config and store it as `assistants.dev`
   in a config delta.
5. Append `dev` to `conversation.participants` in the same effective config
   change.

The loaded file is not special.
It is the same kind of config file that can be loaded with `--cfg`.
The special operation is the invite command: it captures the file's
assistant-facing result under a participant name.

Inviting `assistant` adds the reserved participant name to the roster and uses
the top-level `assistant.*` config.
It does not create `assistants.assistant`.

A participant identifier can be supplied separately when the source path does
not imply the desired identifier or when two sources have the same file stem:

```sh
jp c invite reviewer --source personas/pr-reviewer
jp q --new --invite reviewer=personas/pr-reviewer "hello"
```

Both forms capture `personas/pr-reviewer` as participant `reviewer`.
`--cfg` remains reserved for ordinary config composition and is never used as
the invite source syntax.

### Uninviting and re-inviting

Uninviting removes the participant name from `conversation.participants` and
leaves `assistants.<name>` intact.

```sh
jp c uninvite dev
```

For a roster `["assistant", "dev", "architect"]`, the command writes a
replacement delta for the small participant array:

```toml
[conversation]
participants = ["assistant", "architect"]
```

The captured `assistants.dev` config remains in the conversation snapshot and
deltas.
Re-inviting `dev` adds the name back to `conversation.participants` and uses the
existing captured config, including conversation-local overrides.

This avoids tombstone state, sidecar files, and negative deltas.
The event stream remains the join/leave record: deltas show when the participant
was added or removed, and event attribution shows who said what.

A future refresh command may discard the captured config and reload from the
source file:

```sh
jp c participant refresh dev
```

That is not part of v1.

### Assistant-facing tool configuration

Tool bindings are assistant-facing.
The current `conversation.tools` field is de facto scoped to the single
assistant.
Multi-participant conversations make that scope explicit.

Final shape:

```toml
[assistant.tools.fs_read_file]
source = "local"

[assistants.dev.tools.fs_modify_file]
source = "local"
run = "ask"
```

`providers.mcp` remains global because MCP server configuration describes shared
processes.
Participants capture the tool entries they may use.
`ToolConfig` is not split into definition, behavior, display, and access
sub-objects.
Per-participant differences are represented by different captured tool entries.

This RFD migrates `conversation.tools` to assistant-facing config:

- old config files may still use `conversation.tools` as legacy input;
- for the reserved `assistant` participant, legacy `conversation.tools` maps to
  `assistant.tools`;
- when inviting a named assistant from a source file, legacy
  `conversation.tools` in that source maps to `assistants.<name>.tools`;
- query setup, tool rendering, tool enable/disable flags, and `ToolCoordinator`
  use the selected participant's assistant-facing tool config.

New config should use `assistant.tools` in reusable assistant files and
`assistants.<name>.tools` in captured conversation config.

### Relationship to tool configuration RFDs

This RFD extends the tool configuration surfaces from [RFD 076] and [RFD 078]
from one assistant to multiple assistant participants.

Legacy input:

- `conversation.tools` remains accepted as legacy input.
- For the reserved `assistant` participant, legacy `conversation.tools` maps to
  `assistant.tools`.
- During invite, legacy `conversation.tools` in the source maps to
  `assistants.<name>.tools`.

Runtime config writes are participant-relative.
A tool running for participant `dev` that writes `assistant.model.id` stores the
delta at `assistants.dev.model.id`.
A tool running for the reserved `assistant` participant writes to top-level
`assistant.model.id`.
Tools must not mutate another participant's assistant config unless granted an
explicit absolute config path by [RFD 078]'s access model.

Sensitive tool access paths include the participant-scoped locations:

- `assistant.tools.*.access`;
- `assistants.*.tools.*.access`;
- legacy `conversation.tools.*.access` while compatibility remains.

### At-mention policy

An assistant controls whether mentioning it can invite or rejoin it.

```toml
[assistant.at_mention]
invite = "error_if_known" # allow | error_if_known | ignore
rejoin = "error_if_known" # allow | error_if_known | ignore
```

Captured examples:

```toml
[assistants.architect.at_mention]
invite = "allow"
rejoin = "allow"

[assistants.dev.at_mention]
invite = "error_if_known"
rejoin = "error_if_known"
```

Defaults are `error_if_known`.

Mentions are parsed from `ChatRequest.content` with a small lexer rule:

- a mention starts at the beginning of the string or after whitespace;
- the mention starts with `@`;
- the participant name matches `[A-Za-z_][A-Za-z0-9_-]*`;
- `\@name` is literal text;
- `email@example.com` and `foo@bar` are not mentions.

When JP parses `@name` inside a query:

1. If `name` is active in `conversation.participants`, select it as a responder.
2. If `assistants.<name>` exists but `name` is not active, apply that
   assistant's `rejoin` policy.
3. If no captured config exists but `name` resolves as a config source, load it
   and apply its `assistant.at_mention.invite` policy.
4. If no captured config or source resolves, treat `@name` as plain text.

Unknown inline mentions are plain text.
The `invite` and `rejoin` policies apply only after the mention resolves to a
captured assistant or loadable assistant source.

The explicit `--at <name>` flag is stricter and not governed by
`assistant.at_mention`:

1. If `name` is active in `conversation.participants`, select it as a responder.
2. If `assistants.<name>` exists but `name` is not active, add it back to
   `conversation.participants` and select it.
3. If no captured config exists but `name` resolves as a config source, load and
   capture it, add it to `conversation.participants`, and select it.
4. If no captured config or source resolves, error with a hint to invite the
   participant or check the spelling.

Policy outcomes:

- `allow`: mutate the roster as needed and select the participant;
- `error_if_known`: fail with a hint to use `jp c invite <name>`;
- `ignore`: treat the mention as plain text.

The mention is parsed from `ChatRequest.content`.
This is valid:

```sh
jp q "@dev please review this"
```

This is not the at-mention form:

```sh
jp q @dev "please review this"
```

The explicit CLI equivalent is:

```sh
jp q --at dev "please review this"
```

Inline `@name` mentions replace the previous query-text `@path` shorthand.
File references in query text use a `file:` prefix instead:

```sh
jp q "@dev please review file:README.md"
```

The `file:` form is an attachment candidate.
Future attachment handlers may add other inline references, such as HTTP URLs or
`jp://` conversation links.

### Addressing and visibility

Addressing and visibility are separate axes.

- **Visibility**: who sees the event in assistant projections. v1 events are
  public to all assistant participants.
- **Addressing**: who is expected to respond.

A `ChatRequest` carries `recipients.responders`:

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

If no `@name` or `--at <name>` selects responders, the responder set is all
active assistant participants.
This means `jp q "hello"` is a room-visible request and all active assistant
participants are expected to answer.

If one or more responders are selected, only those assistant participants are
expected to answer.
The request is still visible to everyone in v1.

Responder order is:

1. For unaddressed broadcast requests, `conversation.participants` order.
2. For addressed requests, explicit order: `--at` values in flag order, followed
   by inline `@name` mentions in content order for names not already selected.

Duplicate responders are deduped by first occurrence.

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
Stored events are human-visible by being present in the conversation store.
Visibility metadata controls assistant provider projections only.

### Query behavior

A single `ChatRequest` addressed to N assistant participants produces one turn
containing N participant execution substreams.
`TurnStart` marks the turn boundary.
A normal query appends `TurnStart`, then a `ChatRequest`, then the
participant-originated events that answer that request.

For a broadcast request:

```sh
jp q "hello"
```

all active assistant participants respond to the same request.
Responders generate sequentially in `conversation.participants` order.
Each responder's projection includes the original `ChatRequest` and any prior
assistant responses already produced for that turn.
The terminal renders each response as it streams, then the next responder
begins.
Appends to the event stream use the same order under the conversation lock ([RFD
020]).

Sequential generation preserves the room model: later responders can react to
earlier responders in the same turn.
Parallel independent responder execution is future work because it changes
semantics, not only performance.

For an addressed request:

```sh
jp q "@dev hello"
```

only `dev` responds.

A future reply-order graph may allow branching plans such as "dev and architect
respond independently, then reviewer summarizes both". v1 defines a single
linear responder order.

### Participant execution scope

Participant execution produces a substream of events while answering a specific
`ChatRequest`.
Every participant-originated event carries:

- `participant`: the assistant participant that produced the event;
- `request_id`: the stable event ID of the `ChatRequest` being answered.

This applies to:

- `ChatResponse`;
- `ToolCallRequest`;
- `ToolCallResponse`;
- `InquiryRequest` created during that participant's tool execution;
- `InquiryResponse` answering an inquiry created during that participant's tool
  execution.

Tool-call matching uses `(participant, tool_call_id)` rather than `tool_call_id`
alone.
Inquiry matching uses `(participant, inquiry_id)` rather than `inquiry_id`
alone.
This keeps provider-generated IDs scoped to the participant that produced them
and lets projection distinguish own tool/inquiry events from peer events.

`request_id` depends on stable event identifiers from [RFD 097].

### Assistant-scoped CLI flags

Current query flags such as `--model`, `--param`, `--tool`, and `--no-tools`
mutate assistant-facing config.
In a multi-participant conversation, JP resolves `@name` and `--at <name>` into
a responder set before applying these flags.
Assistant-scoped flags apply only when the request has exactly one assistant
responder.

Allowed:

```sh
jp q --model gpt "@dev hello"
jp q --at dev --model gpt "hello"
```

Both mutate only `dev`:

```toml
[assistants.dev.model]
id = "gpt"
```

For the reserved assistant participant:

```sh
jp q --at assistant --model gpt "hello"
```

mutates:

```toml
[assistant.model]
id = "gpt"
```

A broadcast request with assistant-scoped mutation is ambiguous:

```sh
jp q --model gpt "hello"
```

If more than one assistant participant is active, this fails:

```text
error: `--model` is ambiguous for a broadcast request
hint: use `--at dev --model gpt` or `-c @dev:model.id=gpt`
```

Participant-scoped config sugar maps `@name:` to the captured config path:

```sh
jp q -c @dev:model.parameters.reasoning.effort=max "@dev try again"
```

maps to:

```toml
[assistants.dev.model.parameters.reasoning]
effort = "max"
```

For `assistant`, `@assistant:` maps to top-level `assistant.*`.
This is a `--cfg` parser addition: values matching
`@<participant>:<key>=<value>` are parsed as participant-scoped assignments
before the existing leading-`@` path form.
Values such as `@config/dev.toml` remain path references.

### Event attribution

`ChatRequest` gains:

- `author`: existing human display name from `user.name`;
- `recipients.responders`: assistant participant identifiers expected to
  respond.

Attribution is event-level, not turn-level.
A turn can contain more than one request, and requests can address different
participants.

Attribution is stored even in 1:1 conversations.
Provider projections can omit labels for strict 1:1 history, but stored history
remains attributable if more participants join later.

### Speaker-aware projection

Projection maps the stored multi-voice event stream into the dyadic
`user`/`assistant` message format each provider expects.
The projection is built separately for each assistant participant.

Rules:

- The generating participant's own prior responses map to `assistant`.
- Every human request and every peer assistant response maps to `user`.
- External turns are labeled in-band with the speaker name when the projection
  has more than one external speaker.
- Labels are omitted for strict 1:1 projections so existing conversations remain
  byte-for-byte equivalent at the provider boundary.
- Peer reasoning is not projected to other assistants.
- Peer tool calls, tool responses, and inquiry events are not projected to other
  assistants.
  Only peer chat responses are shared across assistant projections.
  Tool and inquiry internals remain visible in the stored conversation and to
  the producing participant.

Example projecting for `dev`:

```text
system:    <dev prompt>
user:      [jean]: should we refactor this?
assistant: yes, I would refactor it
user:      [architect]: I disagree; the boundary is still unstable
```

The provider API remains strictly dyadic.
In-band labels carry participant identity because providers may merge
consecutive same-role turns.

## Drawbacks

- **Two assistant config shapes exist.** `assistant.*` remains the canonical
  assistant authoring shape and backs the reserved `assistant` participant;
  `assistants.<name>.*` backs captured named participants.
  The seam is accepted to preserve composable `assistant.*` fragments and keep
  existing `base_config.json` snapshots valid without custom aliasing in v1.
  Runtime code must contain the seam behind config accessors.
- **Captured participant config can be verbose.** Large assistant prompts and
  instructions may appear in config deltas.
  This is accepted: `events.json` is the event store.
  Readability should be improved with tooling, not by splitting storage into
  side files.
- **Assistant-scoped flags become conditional.** Flags such as `--model` remain
  ergonomic for one responder, but error for broadcasts.
- **Broadcast latency scales with responders.** A broadcast to `K` assistant
  participants runs `K` model turns sequentially in v1.
  This preserves the room model because later responders can react to earlier
  responders in the same turn, but it makes multi-assistant broadcasts slower
  than parallel independent execution.
- **Invite does more than append a name.** It resolves a config source with the
  same loader used by `--cfg`, captures assistant-facing config, and updates the
  participant roster.

## Alternatives

### Separate conversations with relay

Rejected as the core model.
Separate conversations keep each provider request simple, but the shared room
becomes an illusion maintained by an orchestrator.
That duplicates context, loses a single authoritative event stream, and pushes
speaker attribution and projection into workflow code.
A relay workflow remains buildable on top of this RFD; it should not be JP's
core conversation model.

### Hierarchical sub-agents

Rejected for this use case.
[RFD 051] describes delegation: a main agent sends scoped work to sub-agents and
receives summaries.
This RFD describes peer participation: multiple assistants are first-class
voices in one conversation.
Delegation remains useful for research and task fan-out, but it does not solve
participants deliberating in the same room.

### Store participants in metadata

Rejected.
Conversations already store a snapshotted config plus deltas.
Participant membership and captured config belong in that self-contained config
state, not in `metadata.json`.

### Store participant snapshots in sidecar files

Rejected.
Mid-stream participant changes are ordered config changes.
Sidecar files would need references, versioning, and extra failure handling.
Large config deltas are simpler and keep the event store authoritative.

### Store captured config under `conversation.participants.<name>.assistant`

Rejected.
It makes membership and captured config one map entry, so uninviting removes the
captured config unless tombstones or negative deltas are introduced.
Keeping active membership (`conversation.participants`) separate from captured
config (`assistants.<name>`) preserves re-invite behavior without extra state.

### Tombstone participant state

Rejected.
`state = "uninvited"` leaves non-participants in the participant map.
The roster should list current participants only.

### Per-field conditions

Rejected.
Conditions make shared fragments know who consumes them, which couples
orthogonal config fragments and grows special cases as more assistants are
added.
Existing file-level `extends` keeps fragments independent.

### Group-transcript projection

Rejected.
Rendering the whole history as one transcript loses native assistant continuity,
prompt caching, and tool-call structure for the generating participant.

## Non-Goals

- **Autonomous arbitration.** Deciding who should chime in, whether to stay
  silent, and how bots converse without a human request belongs in a plugin
  built on query delegation, not in v1 core.
- **Reply-order graphs.** v1 selects responders for one request.
  Arbitrary sequential and fan-out/fan-in plans are future work.
- **Private messages.** `--dm <participant>` is future work. v1 events are
  public to all assistant projections.
- **Deleting captured assistant configs.** Uninvite removes a name from the
  active roster and preserves `assistants.<name>`.
- **Refreshing from source.** Reloading a captured assistant from its source
  file is future work.
- **Wider config namespace cleanup.** Moving unrelated `conversation.*` fields
  is not part of this RFD.

The at-mention policy and assistant-scoped CLI flag scoping are included because
they define the minimum coherent v1 query surface.
If review stalls on either, they can be split into follow-up RFDs without
changing the storage or projection model.

## Future Work

### Direct messages

A future `--dm <participant>` sends a request only to that assistant participant
and hides both request and response from other assistant projections.
The events remain visible to humans in the stored conversation.

Future event shape:

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

Humans are not configured recipients.
Visibility controls assistant provider projections only.

### Reply-order graph and parallel responses

A future RFD may add a reply plan for cases where responders should branch,
merge, or run independently.
The internal model can be a dependency graph over responders; v1 exposes only a
single linear responder order.

Parallel independent responder execution is also future work.
It can reduce latency, but it means co-responders do not see each other's
outputs until the next turn, which is different from the room model v1 provides.

### Participant refresh

Re-invite uses the existing captured assistant config.
This follows the same rule as all conversation config: source config changes do
not affect an existing conversation unless explicitly applied.
Use `-c @name:...` for conversation-local tweaks.

A future command may intentionally replace a captured assistant config by
resolving its source again:

```sh
jp c participant refresh dev
```

## Risks and Open Questions

- **Tool migration.** Moving `conversation.tools` into assistant-facing config
  is necessary, but it touches query execution, tool rendering, and config docs.
- **At-mention source resolution.** Checking a resolvable `@name` may need to
  load a source config to read `assistant.at_mention.invite`.
  Cache source-resolution results per command invocation.
  Unknown inline mentions are plain text; unknown `--at` values are errors.
  `--invite` and `--at` use the same config-source resolution as `--cfg`, but
  they capture the resolved assistant-facing config instead of merging it into
  the current assistant.
- **Large config deltas.** This worsens raw `events.json` readability.
  The answer is editor folding and JP tooling for viewing/editing event streams.
- **Projection behavior.** `user` + labels is the v1 projection.
  It should be tested against real multi-assistant review conversations before
  acceptance.
- **Backwards compatibility.** Existing conversations without
  `conversation.participants` must behave as a single `assistant` participant.

## Implementation Plan

1. **Config schema and accessors.** Add `assistants` as a top-level map with the
   same assistant-facing shape as `assistant`.
   Add `conversation.participants` as an ordered array of assistant participant
   identifiers.
   Add `assistant.at_mention` policy and validation linking participants to
   captured configs.
   Add helper accessors so runtime code does not open-code the reserved
   `assistant` branch.
   Depends on nothing.

2. **Stable event identifiers.** Implement or require [RFD 097], so
   `ChatRequest` entries can be referenced by stable `event_id`.
   Depends on nothing in this RFD.

3. **Participant execution attribution.** Add `participant` and `request_id` to
   participant-originated chat, tool, and inquiry events.
   Tool and inquiry matching move to `(participant, id)` keys.
   Depends on Phases 1 and 2.

4. **Single-responder query path.** Resolve one assistant participant and run a
   normal query through that participant's assistant config.
   Depends on Phases 1 and 3.

5. **Tool migration and participant-aware tool execution.** Move
   assistant-facing tool bindings to assistant participant config while
   preserving compatibility for config sources that still use
   `conversation.tools`.
   Query setup, tool rendering, tool enable/disable flags, and `ToolCoordinator`
   move to the selected participant's assistant-facing tool config.
   Depends on Phases 1, 3, and 4.

6. **Invite and uninvite commands.** Implement `jp c invite <name>` and `jp c
   uninvite <name>`.
   Invite resolves normal config sources, captures assistant-facing config into
   `assistants.<name>`, and appends the name to `conversation.participants`.
   Uninvite replaces the participant array without the removed name.
   Depends on Phases 1 and 5.

7. **Query responder resolution.** Parse `@name` mentions inside
   `ChatRequest.content`, implement `--at`, apply at-mention invite/rejoin
   policy, and compute the responder set.
   Depends on Phases 3 and 6.

8. **Broadcast sequential responders.** Execute multiple responders in a single
   turn using the linear responder order.
   Depends on Phases 3, 5, and 7.

9. **Speaker-aware projection.** Implement speaker-aware projection with
   conditional labels, peer reasoning stripping, and peer tool/inquiry omission.
   Depends on Phases 3 and 7.

10. **Assistant-scoped CLI flag scoping.** Rewrite existing assistant-mutating
    query flags to the selected single responder.
    Error for broadcasts unless the user uses participant-scoped `--cfg @name:`.
    Depends on Phases 1 and 7.

11. **Documentation and tooling.** Document reusable assistants as normal config
    files loaded by invite.
    Add event-viewing/editing affordances so large config deltas do not make raw
    `events.json` the only practical editing surface.
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
[RFD 051]: ../051-sub-agent-workflows.md
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
