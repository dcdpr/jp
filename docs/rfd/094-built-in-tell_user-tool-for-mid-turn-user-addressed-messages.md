# RFD 094: Built-in tell\_user Tool for Mid-Turn User-Addressed Messages

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-02
- **Extends**: [RFD 058]

## Summary

Add a built-in `tell_user` tool that lets the assistant deliver a message the
user must see, mid-turn, without ending the agentic loop.
The message renders as regular chat content; the assistant receives a short
acknowledgement and continues working.

The tool itself is small.
Most of this RFD activates generic mechanisms that [RFD 058] defines but does
not yet act on: honoring MCP `audience` annotations at JP's two view boundaries,
rendering `text/markdown` content through the markdown pipeline, and one new
style knob (`style.header`).
`tell_user` is the first consumer of these mechanisms, not a special case in
jp-core.

## Motivation

A response containing only chat text is terminal: the turn ends.
A response containing tool calls keeps the agentic loop going — JP executes the
tools and sends the responses back.
This couples "say something to the user" with "have unrelated tool work in the
same response": the assistant cannot deliver a progress update, a partial
deliverable, or a direct answer to a mid-loop question and then continue
working, unless it happens to also call another tool.

`tell_user` removes that coupling.
The message rides in a tool call, so the response is non-terminal; the
acknowledgement keeps the loop alive.
This matches the `send_to_user` pattern Anthropic documents for long-running
agents ([Prompting Claude Fable]), including its observation that tool inputs
are delivered verbatim while free text may be treated as summarizable narration.

Secondary benefit: the message carries an explicit "addressed to the user"
marker, so every consumer of the conversation — terminal, web viewer, future
frontends — can identify user-facing deliverables without heuristics.

## Design

### User-visible behavior

The assistant calls `tell_user` like any other tool:

```jsonc
{
  "name": "tell_user",
  "arguments": {
    "message": "Phase 1 done: all 14 call sites migrated. Starting on the test suite next; this will take a few more minutes.",
  },
}
```

The user sees the message rendered as markdown chat content — no "Calling tool"
header, no arguments block, no result chrome.
The assistant receives `"Message shown to user."` as the tool result and the
loop continues.
Turn-end semantics are unchanged: a text-only response remains terminal, and
`tell_user` keeps the loop alive exactly like any other tool call.

### Tool response shape

`tell_user` returns two content blocks in the [RFD 058] format:

```jsonc
{
  "content": [
    {
      "type": "text",
      "text": "<the message, verbatim>",
      "mimeType": "text/markdown",
      "annotations": { "audience": ["user"] },
    },
    {
      "type": "text",
      "text": "Message shown to user.",
      "annotations": { "audience": ["assistant"] },
    },
  ],
}
```

Two notes on this shape:

- `audience` is MCP's standard annotation (`user` / `assistant`, per block).
  [RFD 058] carries the annotation types for MCP compatibility but does not act
  on them; this RFD adds the first behavior.
- `mimeType` on `text` blocks is the optional field [RFD 058] defines as a JP
  extension (MCP's `TextContent` has no mimeType), following [RFD 065]'s
  MCP-compatible superset principle: MCP tools never set it, and absence means
  plain text.
  [Alternatives](#carry-the-message-in-a-resource-block) covers why the message
  does not ride in a `resource` block, which has `mimeType` natively.

### Audience honoring

The two view boundaries apply a symmetric filter:

- **Provider view** ([RFD 058]'s shared block-to-string conversion): includes
  blocks whose `audience` is absent or contains `assistant`.
  The `ToolCallResponse` sent back to the provider carries only the
  acknowledgement — the message is not duplicated in the tool result.
  The assistant-authored `ToolCallRequest` retains the `message` argument in the
  replayed transcript, as with every tool call.
- **User view** (chat-style terminal rendering, the plugin host's `user` events
  view): includes blocks whose `audience` is absent or contains `user`.
  The acknowledgement is not part of the user view.

If the provider filter removes every block from a successful result, JP
substitutes a neutral placeholder — `"Tool executed successfully."` — so the
request/response pairing stays wire-valid.
The placeholder does not hint at withheld content; telling the model that hidden
content exists invites retries and speculation.
A failed result keeps `isError` and gets `"Tool failed."` in the same case.

This also fixes a latent spec-compliance gap: MCP servers can send `audience`
annotations today, and JP silently ships user-only blocks to the LLM.
After this RFD, JP honors the annotation for every tool, not just `tell_user`.

### The user view as a canonical API

"Which content is addressed to the user" must have exactly one definition, or
every consumer improvises it (and a chat-only view silently drops `tell_user`
messages).
`jp_conversation` exposes a single accessor — the **user view** — that yields
user-addressed content regardless of the carrying event: `ChatResponse` content
and user-audience blocks from tool call responses.
Chat-style consumers use it directly — the plugin host's `ReadEvents` handler
when a plugin requests the `user` view, and any chat-only rendering.
The terminal's tool-call rendering reads the same audience data through
`inline_results`' per-audience settings.

The layering is strict: the user view is computed from event data alone —
audience annotations — and knows nothing of configuration.
Display policy (`style.hidden`, `style.header`, `inline_results`) belongs to the
renderers.
Terminal style does not propagate to plugins.

`ReadEvents` gains a `view` selector: `raw` (the default) returns the full
serialized event stream unchanged; `user` returns the user view.
Both shapes have real consumers — chat-style frontends want the projection,
audit and tooling paths want every block — so the choice is explicit in the
request rather than implied by the consumer. serve-web requests the `user` view
for its chat rendering.

The `user` view is the raw shape, filtered: the same event kinds, IDs, and
response envelope, no synthetic types (consistent with the rejection of event
synthesis in [Alternatives](#event-synthesis-user-projection)).
The filtering rules are defined per event family; this RFD defines the chat,
turn, and tool-call families:

- `turn_start` and `chat_request` events pass through unchanged — a chat-style
  consumer needs the user's own messages and turn boundaries, not only
  assistant-side content.
- `chat_response` events pass filtered by variant: `Message` and `Structured`
  pass (both are the assistant's answer, addressed to the user); `Reasoning` is
  dropped — thinking content is not user-addressed, and its terminal display is
  already an opt-in style choice.
  Consumers that want reasoning use the `raw` view.
- `tool_call_response` events pass with their content reduced to user-audience
  blocks; responses left empty by the reduction are dropped, along with their
  paired `tool_call_request`.
- `tool_call_request` events are dropped (arguments are not user-addressed
  content).
- Inquiry events are deferred: not every persisted inquiry pair is user-facing
  — inquiries resolved by the inquiry backend record exchanges the user never
  saw — and the source attribution that distinguishes them belongs to [RFD
  082].
  Inquiry-family rules land with that work.
- All remaining event kinds are dropped.

`raw` is the only view that preserves conversation-stream invariants: in the
`user` view a `tool_call_response` may have no paired request, so the result is
a projection in event shape, not a valid conversation stream.
Consumers must not feed it back into anything that consumes streams — provider
conversion, storage repair, compaction, or event validators.

```jsonc
// request
{ "type": "read_events", "conversation": "<id>", "view": "user" }

// response: the same EventsResponse envelope as `raw`, filtered
{ "data": [
  { "type": "turn_start", /* … */ },
  { "type": "chat_request", "content": "…" },
  { "type": "tool_call_response", "id": "call_1", "content": [
    { "type": "text", "text": "Phase 1 done.", "mimeType": "text/markdown",
      "annotations": { "audience": ["user"] } }
  ] },
  { "type": "chat_response", "message": "…" }
] }
```

The ubiquitous language gains two entries: *Audience* and *User View*.

### Markdown rendering for content blocks

Blocks carrying `mimeType: text/markdown` render through the `jp_md` pipeline —
the same rendering chat content receives — instead of a fenced code block.
This is generic: any tool returning a report, summary, or explanation as
markdown benefits.
It makes the block renderer a second consumer of `jp_md`'s public rendering API;
the existing streaming-identity and comrak cross-validation suites become shared
contract tests for both consumers.

### `style.header`

A new knob on the per-tool display style: whether to render the "Calling tool
`<name>`" header line.
Accepts a boolean or `"on"` / `"off"`, following the bool-or-string pattern
`inline_results` and `results_file_link` already use (their string vocabularies
differ; `header` introduces `"on"`).
Defaults to `true`.
Setting it off also suppresses the streaming temp line shown while the tool's
arguments are being received.
The `style.error` overlay cannot override `header`: the header renders before
the result exists, so an error-conditional header is structurally meaningless.
With `parameters = "<command>"`, setting `header = "off"` prints the formatter's
output with no header line above it.

`style.hidden` is unchanged and remains the absolute kill switch: `hidden =
true` renders nothing for the tool in the terminal, including user-audience
blocks.
A user who hides a tool has opted out of its terminal output entirely; the raw
stream and plugin views are unaffected.

### Result display and audience

No new style knob is needed: `inline_results` remains the single control for
what a tool call's results display inline.
When [RFD 058]'s block model lands, the value gains a per-audience map form; the
existing scalars stay and apply to both audiences:

```toml
[conversation.tools.my_tool.style.inline_results]
user = "full" # blocks addressed to the user: show fully
assistant = "off" # blocks addressed to the assistant: show nothing
```

Each audience key takes the values the scalar form takes today — `off`, `full`,
or a line count — plus one map-only value: `chat`.
`chat` renders the selected blocks as assistant speech through the chat
pipeline, untruncated.
It is valid only on the `user` key; `assistant = "chat"` is rejected.
An omitted key keeps the default (`10`).
A scalar value (`off`, `full`, `<N>`) is shorthand for setting both audiences to
that value — `off` is off for everything, exactly as `hidden` suppresses
everything.
The pre-existing serialized forms all remain valid — booleans, `off` / `full`
strings, numbers, and the `{ truncate = { lines = N } }` object — and are
distinguished from the audience map by their keys.

A block with no `audience` annotation is addressed to both audiences and renders
under the more permissive of the two settings.
Permissiveness is a total order: `off` \< a line count \< `full`, the larger of
two line counts wins, and `0` is accepted as a line count equivalent to `off`.
`chat` counts as `full` in this comparison, but the speech classification
applies only to blocks explicitly annotated `user`: an unannotated block
admitted by a `chat` setting renders as ordinary result display, in full.

| `user`  | `assistant` | unannotated block renders |
| ------- | ----------- | ------------------------- |
| `off`   | `off`       | hidden                    |
| `10`    | `off`       | first 10 lines            |
| `5`     | `10`        | first 10 lines            |
| `full`  | `10`        | full                      |
| `chat`  | `off`       | full, as result display   |
| omitted | `off`       | first 10 lines (default)  |

Presentation is mimeType-driven wherever a block renders — a `text/markdown`
block is pretty-printed through the markdown pipeline inside result display just
as it is inside chat speech.
Channels follow [RFD 048]: the terminal renders `chat`-classified blocks on
stdout, like any other assistant speech; all other tool-call rendering —
headers, arguments, and result display, including user-addressed non-`chat`
blocks — stays on stderr.
Display styles govern terminal text rendering only; structured output modes
(`--format json`) serialize events and are unchanged by this RFD. serve-web
renders `chat`-classified blocks through its existing chat path.

Today's behavior is preserved everywhere: no existing tool emits audience
annotations, and the scalar default (`10`) applies to all blocks exactly as now.
The `style.error` overlay composes unchanged.

### Tool configuration

Registered in `jp_cli::cmd::query::tool::builtins::all()`, using the existing
`describe_tools` registration path, with the `if_named` enable policy [RFD 083]
proposes for `ask_user` (083 is expected to merge before this RFD):

- `source`: builtin.

- `enable`: `{ state = true, allow_toggle = "if_named" }` (per [RFD 081]) —
  enabled by default, immune to bare `-T`, disableable by name.
  The reasoning matches `ask_user`: this is a core conversational capability,
  the only in-band way for the assistant to surface a deliverable mid-turn.

- `run: Unattended`, `result: Unattended` — displaying a message needs no
  permission or delivery prompt.

- Style:

  ```toml
  [conversation.tools.tell_user.style]
  header = "off"
  parameters = "off"
  inline_results = { user = "chat", assistant = "off" }
  results_file_link = "off"
  ```

  The message (user-addressed) renders as assistant speech; the acknowledgement
  (assistant-addressed) is hidden.
  Failures stay visible without an error overlay: error text is unannotated —
  addressed to both audiences — so the more-permissive rule renders it in full,
  as result display rather than speech.

- `parameters` schema: a single required `message` string.

- `description` (model-visible; guards against overuse).
  The positive cases follow Anthropic's published `send_to_user` guidance
  ([Prompting Claude Fable]); the final-answer guard is JP-specific, because in
  JP a text-only response is the proper terminal channel:

  > Display a message directly to the user.
  > Use this for progress updates with specific numbers, partial deliverables,
  > or a direct reply to a question the user asked mid-task — content the user
  > must see exactly as written before the task finishes.
  > The message is rendered verbatim as chat content; you receive an
  > acknowledgement and your turn continues.
  > Do not route narration or internal reasoning through this tool, and do not
  > use it for your final response — end your turn with a normal message for
  > that.

Defining the tool is not sufficient on its own: Anthropic documents that Claude
Fable 5 rarely calls `send_to_user` without an instruction in the system prompt
([Prompting Claude Fable]).
JP does not ship default elicitation text — that is persona and workspace
configuration — but the recommended snippet, adapted from Anthropic's guidance,
is:

> Between tool calls, when you have content the user must read verbatim (a
> partial deliverable, a direct answer to their question), call the `tell_user`
> tool with that content.
> Use `tell_user` only for user-facing content, not for narration or reasoning.

The tool description carries the usage contract either way.

### Local-tool equivalence

Nothing in this design requires jp-core treatment.
The same tool is expressible as a local tool — a script emitting the content
JSON above — plus the TOML style block.
The builtin is packaging, chosen for out-of-the-box availability, no external
binary dependency, and no per-message process spawn.
The builtin's module comment states this; when a tool registry lands (the
direction sketched in [RFD 072]'s plugin registry), distribution of
non-privileged bundled tools can move there.

This surfaces a distinction JP does not draw today: built-ins that are merely
*bundled* (`tell_user` — generic mechanisms, shipped for convenience) versus
built-ins that are *privileged* (`describe_tools` — reads tool metadata no
external tool can access).
Formalizing that taxonomy is future work, out of scope here.

## Drawbacks

- **One more built-in with overuse potential.** An assistant that narrates every
  step through `tell_user` degrades the experience.
  The description discourages this; real usage should be monitored.
- **The message is stored twice on disk** (request arguments and response
  block).
  It is never duplicated on the wire: the provider sees it once, in the replayed
  tool-call arguments the assistant authored; the tool result carries only the
  acknowledgement.
- **Audience-split responses can diverge.** A tool may show the user one thing
  and the assistant another; both are right and neither knows.
  The raw stream persists every block with its annotation, the raw `ReadEvents`
  view and `inline_results = "full"` expose the divergence on demand, and the
  documented convention is that audience-split content must be additive or
  reformulated, never contradictory.
  For `tell_user` the assistant-facing content is a boilerplate ack, so the risk
  here is nil; the convention exists for future adopters.
- **Consumers must adopt the user view.** A consumer that pattern-matches raw
  event kinds misses user-addressed tool content.
  Post-[RFD 058] every consumer must become block-aware regardless; the
  canonical accessor makes the correct behavior the easy path.

## Alternatives

### Strip the tool call and persist a `ChatResponse` instead

Rejected.
The acknowledgement is the loop-continuation mechanism itself: without a tool
result, the next request has nothing legal to send — the alternatives are
fabricating a user message or relying on assistant-prefill continuation, which
does not compose with tool use across providers.
Providers also validate the replayed shape: Anthropic thinking signatures and
Google thought signatures are tied to the exact event structure, and JP already
carries recovery machinery for when they drift.
Rewriting a tool call into text at the position providers validate manufactures
that failure mode.
Finally, the swap erases the record's semantics: a mid-loop note and a final
answer become indistinguishable.

### Persist the pair plus a display-only `ChatResponse` event

Rejected.
Materializing the view duplicates content for every consumer (`grep`,
compaction, export) and requires a provider-invisibility marker that fails
silently when a code path forgets it.
The audience filter computes the same view with nothing to keep in sync.

### Event-synthesis user-projection

Swap `tell_user` pairs into synthetic `ChatResponse` events inside a projection
applied at view boundaries.
Rejected: its one advantage — consumers stay unchanged — is void once [RFD
058] changes the serialized `ToolCallResponse` shape, which forces every
consumer to become block-aware anyway.
Audience filtering achieves the result without synthesizing events.

### Carry the message in a `resource` block

`resource` blocks have `mimeType` natively, which avoids the text-block
extension.
Rejected: a resource is *identified* content — the URI is required and
load-bearing.
Resource identity feeds URI canonicalization, checksums, and the deduplication
work built on [RFD 058] ([RFD 066], [RFD 067]); a `tell_user` message has no
identity, so every message would carry a fabricated URI and drag ephemeral prose
into machinery designed for files.
The optional `mimeType` field on `text` blocks ([RFD 058]) states the actual
semantics — unidentified text with a presentation hint — in the same pattern
as the JP extension fields [RFD 065] already defines on `Resource`.

### A typed `ToolCallResponse.user_message` field

Rejected.
A JP-only parallel mechanism for something the block model expresses natively
once `audience` annotations are honored — two mechanisms for one job.

### Custom argument formatter

Render the message via `style.parameters = "<command>"`, the `fs_modify_file`
pattern.
Rejected: formatter output renders verbatim (never through the markdown
pipeline), the mechanism is terminal-only by contract, and a builtin whose
default rendering shells out to an external command is a portability wart.

### A `ParametersStyle::Markdown` variant

Rejected: a documented, user-facing config value with a single plausible
consumer.
Superseded by mimeType-driven block rendering, which puts presentation on the
response, where the content lives.

### Ship as a local tool instead of a builtin

Viable by construction — see [Local-tool equivalence](#local-tool-equivalence).
The builtin packaging wins on availability until a tool registry provides
distribution for bundled tools.

## Non-Goals

- **Cross-frontend rendering of custom argument formatters.** Custom formatter
  output remains terminal-only; changing that is a separate RFD.
- **Notification routing.** Queueing, deduplicating, or escalating
  user-addressed messages is [RFD 011] territory.
- **Acting on other MCP annotations.** `priority` and `lastModified` remain
  carried-but-inert.
- **The bundled-vs-privileged builtin taxonomy.** Named above; deserves its own
  decision RFD alongside the registry work.
- **Default elicitation language.** Persona and workspace configuration own when
  the assistant is encouraged to call `tell_user`.

## Risks and Open Questions

- **[RFD 058] is in Discussion.** This RFD tracks its content block model,
  including the optional text-block `mimeType` field defined there.
- **Live/replay equivalence.** The live path renders blocks at outcome time;
  terminal replay renders from raw events, applying the same display policy
  (`hidden`, `header`, `inline_results`) per turn.
  The user view plays no part in terminal replay; it serves chat-style
  consumers.
  One required appearance, two code paths — pinned by an equivalence test
  comparing live terminal output against terminal replay, in the spirit of the
  streaming-identity suite.
- **Elicitation.** Models may under-call the tool without system-prompt
  encouragement.
  This is a utilization gap, not a correctness one: the tool description (always
  model-visible) carries the usage contract, and an assistant that never calls
  `tell_user` degrades to today's behavior — ending the turn to speak.
  Monitor during rollout; if under-use proves chronic, a future RFD can explore
  builtin-contributed prompt sections that ship with the tool and follow its
  enable state, rather than JP injecting per-tool text into system prompts ad
  hoc.
- **serve-web.** The web viewer needs the block-aware update [RFD 058] forces
  anyway; rendering user-audience markdown blocks reuses its existing markdown
  path.
  Under the `user` view its tool rendering keys on responses rather than folding
  responses into requests; that inversion folds into the same update.
- **Inquiry events and the user view.** User-facing inquiry exchanges belong in
  the user view — the user's own answers are user content — but
  assistant-resolved inquiries do not, and the distinction requires [RFD 082]'s
  source attribution.
  The inquiry-family rules are deferred to that work.
- **Compaction.** Tool-call compaction policies drop request/response pairs
  wholesale, so a compacted history loses `tell_user` messages from its user
  view.
  Whether user-audience blocks deserve retention through compaction (the way
  [RFD 058] retains resource metadata when content is dropped) is an open
  question, deferred until compaction and this design coexist.

## Implementation Plan

### Phase 1: Audience honoring and the user view

Depends on [RFD 058] phases 1–2 (types, `ToolCallResponse` migration, MCP
passthrough).
Add the user-view accessor to `jp_conversation`; apply the audience filter in
the provider conversion; add the per-audience map form to `inline_results`
(scalars preserved, applying to both audiences; `chat` valid on the `user` key);
add the `view` selector (`raw` default, `user`) to `ReadEvents` with the
filtering rules above; implement the empty-provider-result placeholder.
Tests: assistant-audience blocks never reach the user view, user-audience blocks
never reach a provider payload, an all-user-audience success reaches the
provider as the neutral placeholder (one test per provider), unannotated blocks
render under the more permissive of the two audience settings, `{ user = 10,
assistant = "off" }` truncates user-addressed blocks while hiding
assistant-addressed ones, `{ user = "chat" }` renders explicitly user-annotated
blocks as speech on stdout while unannotated blocks stay in result display, and
the `user` view passes message and structured chat responses, drops reasoning
responses, and drops tool pairs without user-audience blocks.
Add the *Audience* and *User View* glossary entries.

### Phase 2: Markdown block rendering

Route `text/markdown` text blocks (the optional `mimeType` field from [RFD 058])
through `jp_md`'s public rendering entry point; extend the shared contract
tests.

### Phase 3: `style.header`

Add the knob (bool or `"on"`/`"off"`) with the temp-line suppression, including
`PartialConfigDelta` / `FillDefaults` / `ToPartial` coverage plus config
snapshots.

### Phase 4: The `tell_user` builtin

Executor, registration, description, parameter schema.
Tests: argument validation, response block shape, provider view carries only the
ack, user view carries only the message, loop continuation, live/replay
equivalence.
The module comment records that the builtin uses only generic mechanisms and is
a builtin for packaging reasons alone.

## References

- [RFD 058] — typed content blocks; defines the block model, the optional
  text-block `mimeType`, and carries `audience` annotations type-level.
  This RFD adds the first audience and mimeType behaviors.
- [RFD 065] — the MCP-compatible superset principle governing the text-block
  `mimeType` extension.
- [RFD 081] — the `enable = { state, allow_toggle }` shape.
- [RFD 082] — unified inquiry event recording; its source attribution is the
  prerequisite for the user view's inquiry-family rules, deferred to that work.
- [RFD 083] — the built-in registration pattern and the `if_named` reasoning
  this RFD mirrors.
- [RFD 072] — the plugin registry direction referenced for future distribution
  of bundled tools.
- [RFD 048] — the four-channel output model; the terminal maps
  `chat`-classified blocks to stdout under its contract.
- [RFD 011] — system notification queue; adjacent, deliberately not addressed.
- [Prompting Claude Fable] — Anthropic's `send_to_user` guidance for
  long-running agents.

[Prompting Claude Fable]: https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-fable-5#create-a-send-to-user-tool
[RFD 011]: 011-system-notification-queue.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 058]: 058-typed-content-blocks-for-tool-responses.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 067]: 067-resource-deduplication-for-token-efficiency.md
[RFD 072]: 072-command-plugin-system.md
[RFD 081]: 081-decompose-tool-enable-into-state-and-allow_toggle.md
[RFD 082]: 082-unified-inquiry-event-recording.md
[RFD 083]: 083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
