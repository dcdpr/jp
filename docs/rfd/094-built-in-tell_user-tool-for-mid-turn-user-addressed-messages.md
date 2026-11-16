# RFD 094: Built-in tell_user Tool for Mid-Turn User-Addressed Messages

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-02
- **Extends**: [RFD 058]
- **Summary**: Adds a built-in `tell_user` tool for mid-turn user messages and
  activates MCP `audience` filtering and markdown tool rendering.

## Summary

This RFD adds a built-in `tell_user` tool that lets the assistant display a
markdown message to the user mid-turn and receive a brief acknowledgement
without ending the turn loop.
To support `tell_user` without special-casing, JP filters tool content by MCP
`audience` at the user and provider view boundaries, renders `text/markdown`
content blocks through the markdown pipeline, and adds a `style.header` tool
display setting.

## Motivation

A response containing only chat text ends the turn.
A response containing tool calls continues the turn loop: JP executes the tools
and sends their results back to the model.
Because of this, the assistant cannot send the user a progress update, a partial
deliverable, or a direct answer to a mid-turn question and then keep working,
unless it also happens to call another tool in the same response.

`tell_user` decouples speaking to the user from ending the turn.
The assistant sends the message as a tool call and receives an acknowledgement
as the tool result, which continues the turn loop.
This follows the `send_to_user` pattern Anthropic documents for long-running
agents ([Prompting Claude Fable]), which notes that models treat tool inputs as
verbatim deliverables while treating free-form chat text as summarizable
narration.

Routing mid-turn deliverables through `tell_user` also marks them explicitly as
user-addressed, so the terminal, web viewer, and other frontends can identify
user-facing content without heuristics.

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

The terminal renders the message as markdown chat content, without a "Calling
tool" header, arguments block, or result chrome.
The assistant receives `"Message shown to user."` as the tool result and the
turn continues.
Turn-end semantics do not change: a text-only response still ends the turn.

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

`audience` is the standard MCP per-block annotation (`user` or `assistant`).
[RFD 058] defines the annotation types for MCP compatibility without acting on
them; this RFD implements audience filtering.

`mimeType` on `text` blocks is an optional JP extension defined in [RFD 058]
(MCP's `TextContent` has no `mimeType` field), following [RFD 065]'s
MCP-compatible superset principle: MCP tools omit it, and omitted `mimeType`
defaults to plain text.
See [Alternatives](#carry-the-message-in-a-resource-block) for why the message
uses a `text` block rather than a `resource` block, which has `mimeType`
natively.

### Audience filtering

JP filters content blocks by `audience` at two boundaries:

- **Provider view** ([RFD 058]'s shared block-to-string conversion): keeps
  blocks whose `audience` is absent or includes `assistant`.
  For `tell_user`, the `ToolCallResponse` sent to the provider carries only the
  acknowledgement, avoiding duplicating the message in the tool result.
  The `ToolCallRequest` retains the assistant's `message` argument in the
  replayed transcript, like any other tool call.
- **User view** (chat-style terminal rendering and the plugin host's `user`
  events view): keeps blocks whose `audience` is absent or includes `user`.
  The acknowledgement is excluded from the user view.

If the provider filter removes every block from a successful result, JP
substitutes `"Tool executed successfully."` so the request/response pair remains
valid on the wire.
The placeholder is neutral and does not mention withheld content, which would
invite the model to retry or speculate about hidden output.
When all blocks are filtered from a failed result, JP preserves `isError` and
substitutes `"Tool failed."`.

Audience filtering applies to every tool, not just `tell_user`.
This closes an MCP compliance gap where user-only blocks from MCP servers were
forwarded to the LLM.

### The user view

To keep chat-only consumers from each improvising their own filter (or silently
dropping `tell_user` messages), `jp_conversation` exposes a single **user view**
accessor.
The user view yields all user-addressed content regardless of the carrying
event: `ChatResponse` content and user-audience blocks from tool call responses.
Chat-style consumers, including any chat-only renderer and the plugin host's
`ReadEvents` handler when a plugin requests the `user` view, call this accessor
directly.
Terminal tool-call rendering reads the same audience annotations through the
per-audience `inline_results` settings.

The user view is computed from event data alone (event variants and `audience`
annotations) and is independent of configuration.
Display policy (`style.hidden`, `style.header`, `inline_results`) belongs to the
renderers, and terminal style settings do not affect plugins.

The plugin protocol's `ReadEvents` request gains a `view` selector: `raw` (the
default) returns the full serialized event stream unchanged, while `user`
returns the user view.
Because chat frontends want the filtered projection while audit and tooling
paths need every block, the request selects the view explicitly.
`serve-web` requests the `user` view for its chat rendering.

The `user` view filters the raw stream in place: it uses the same event kinds,
IDs, and `EventsResponse` envelope without introducing synthetic event types
(see [Alternatives](#event-synthesis-user-projection)).
Filtering is defined per event family:

- `turn_start` and `chat_request` events pass through unchanged, since a
  chat-style consumer needs turn boundaries and the user's own messages
  alongside assistant content.
- `chat_response` events are filtered by variant: `Message` and `Structured`
  pass (both are user-addressed assistant replies), while `Reasoning` is
  dropped.
  Thinking content is not addressed to the user, and its terminal display is an
  opt-in style setting; consumers that need reasoning use the `raw` view.
- `tool_call_response` events retain only their user-audience blocks.
  Responses with no user-audience blocks are dropped.
- `tool_call_request` events are dropped, because tool arguments are not
  user-addressed content.
- Inquiry events are deferred to [RFD 082].
  Inquiries resolved by the inquiry backend record exchanges the user never saw,
  and distinguishing them from user-facing inquiries requires [RFD 082]'s source
  attribution.
- All other event kinds are dropped.

Only the `raw` view preserves conversation-stream invariants.
In the `user` view, a `tool_call_response` has no paired `tool_call_request`, so
the output is a filtered projection in event shape rather than a valid
conversation stream.
Consumers must not pass it to stream consumers such as provider conversion,
storage repair, compaction, or event validators.

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

Content blocks with `mimeType: text/markdown` render through the `jp_md`
pipeline, matching chat content instead of rendering inside a fenced code block.
Any tool that returns markdown reports, summaries, or explanations benefits from
this behavior.
The block renderer becomes a second consumer of `jp_md`'s public rendering API,
sharing the existing streaming-identity and comrak cross-validation test suites.

### `style.header`

`style.header` is a per-tool display setting that controls whether JP renders
the "Calling tool `<name>`" header line.
It defaults to `true` and accepts a boolean or `"on"` / `"off"`, following the
boolean-or-string pattern used by `inline_results` and `results_file_link`
(whose string vocabularies differ; `header` introduces `"on"`).
Disabling `header` also suppresses the temporary streaming line shown while JP
receives the tool's arguments.
With `parameters = "<command>"`, setting `header = "off"` prints the custom
formatter's output without a header line above it.
The `style.error` overlay cannot override `header`, because the header renders
before the tool result exists.

`style.hidden` is unchanged and overrides all other terminal style settings:
`hidden = true` suppresses all terminal output for the tool, including
user-audience blocks, while leaving the raw stream and plugin views unaffected.

### Result display and audience

`inline_results` remains the single setting for inline tool result display.
Alongside its scalar values, which apply to both audiences, `inline_results`
accepts a per-audience map:

```toml
[conversation.tools.my_tool.style.inline_results]
user = "full" # blocks addressed to the user: show fully
assistant = "off" # blocks addressed to the assistant: show nothing
```

Each audience key accepts the scalar values (`off`, `full`, or a line count)
plus one map-only value, `chat`.
`chat` renders the selected blocks untruncated as assistant speech through the
chat pipeline; it is valid only on `user` (`assistant = "chat"` is rejected).
An omitted audience key defaults to `10` lines.
A scalar value (`off`, `full`, `<N>`) sets both `user` and `assistant` to that
value.
All existing serialized forms remain valid (booleans, `off` and `full` strings,
numbers, and `{ truncate = { lines = N } }`), distinguished from the audience
map by their keys.
Because tools without `audience` annotations use the scalar default (`10`),
existing tool display behavior and `style.error` overlay composition are
unchanged.

A block without an `audience` annotation is addressed to both audiences and
renders under the more permissive of the two settings.
Permissiveness follows the total order `off` < line count < `full`, where `0` is
equivalent to `off` and the larger of two line counts wins.
`chat` ranks as `full` in this comparison, but only blocks explicitly annotated
with `user` render as assistant speech; an unannotated block admitted by `user =
"chat"` renders in full as ordinary tool result output.

| `user`  | `assistant` | unannotated block renders |
| ------- | ----------- | ------------------------- |
| `off`   | `off`       | hidden                    |
| `10`    | `off`       | first 10 lines            |
| `5`     | `10`        | first 10 lines            |
| `full`  | `10`        | full                      |
| `chat`  | `off`       | full, as result display   |
| omitted | `off`       | first 10 lines (default)  |

Block formatting is driven by `mimeType` regardless of where the block appears:
a `text/markdown` block renders through the markdown pipeline both in tool
result output and in chat speech.
Output channels follow [RFD 048]: the terminal writes `chat`-classified blocks
to stdout like any other assistant speech, while headers, arguments, and all
non-`chat` result blocks (including user-addressed ones) stay on stderr.
Display styles govern terminal text rendering only; structured output modes such
as `--format json` serialize events unchanged.
`serve-web` renders `chat`-classified blocks through its existing chat path.

### Tool configuration

`tell_user` is registered in `jp_cli::cmd::query::tool::builtins::all()` through
the existing `describe_tools` registration path, using the `if_named` enable
policy [RFD 083] proposes for `ask_user` (083 is expected to merge before this
RFD):

- `source`: builtin.

- `enable`: `{ state = true, allow_toggle = "if_named" }` (per [RFD 081]), so
  the tool is enabled by default, unaffected by bare `-T`, and disableable by
  name.
  As with `ask_user`, this is a core conversational capability and the only
  in-band way for the assistant to surface a deliverable mid-turn.

- `run: Unattended`, `result: Unattended`: displaying a message requires no
  permission or delivery prompt.

- Style:

  ```toml
  [conversation.tools.tell_user.style]
  header = "off"
  parameters = "off"
  inline_results = { user = "chat", assistant = "off" }
  results_file_link = "off"
  ```

  The user-addressed message renders as assistant speech, and the
  assistant-addressed acknowledgement is hidden.
  Failures remain visible without an error overlay: error text is unannotated
  (addressed to both audiences), so the more-permissive rule renders it in full
  as tool result output rather than speech.

- `parameters` schema: a single required `message` string.

- `description` (model-visible, guarding against overuse).
  The positive cases follow Anthropic's `send_to_user` guidance ([Prompting
  Claude Fable]), while the final-answer guard is specific to JP, where a
  text-only response is the terminal channel:

  > Display a message directly to the user.
  > Use this for progress updates with specific numbers, partial deliverables,
  > or a direct reply to a question the user asked mid-task: content the user
  > must see exactly as written before the task finishes.
  > The message is rendered verbatim as chat content; you receive an
  > acknowledgement and your turn continues.
  > Do not route narration or internal reasoning through this tool, and do not
  > use it for your final response; end your turn with a normal message for
  > that.

Registering the tool is not enough on its own: Anthropic notes that Claude Fable
5 rarely calls `send_to_user` without a system-prompt instruction ([Prompting
Claude Fable]).
JP leaves elicitation text to persona and workspace configuration rather than
shipping a default prompt, though the tool description carries the usage
contract either way.
The recommended prompt snippet, adapted from Anthropic's guidance, is:

> Between tool calls, when you have content the user must read verbatim (a
> partial deliverable, a direct answer to their question), call the `tell_user`
> tool with that content.
> Use `tell_user` only for user-facing content, not for narration or reasoning.

### Local-tool equivalence

Nothing in `tell_user` requires privileged access to JP internals.
The same tool can be written as a local tool (a script that emits the content
JSON above) paired with the TOML style configuration.
Shipping it as a builtin is a packaging choice that provides out-of-the-box
availability without an external binary dependency or a process spawn per
message.
The builtin's module comment records this so that non-privileged bundled tools
can move to a tool registry once one exists (following the plugin registry
direction in [RFD 072]).

This distinguishes two kinds of builtins: *bundled* builtins like `tell_user`,
which use only generic tool mechanisms and ship in the binary for convenience,
and *privileged* builtins like `describe_tools`, which read internal tool
metadata that external tools cannot access.
Formalizing that taxonomy is left to a future RFD.

## Drawbacks

- **Overuse potential.** An assistant that narrates routine steps through
  `tell_user` adds noise to the conversation.
  The tool description discourages narration, and actual usage should be
  monitored.
- **Duplicate storage on disk.** The message is persisted both in the
  `ToolCallRequest` arguments and in the `ToolCallResponse` content block.
  It is not duplicated on the wire: the provider sees the message only in the
  replayed tool-call arguments, while the tool result carries only the
  acknowledgement.
- **Divergence between audience views.** A tool can show one block to the user
  and a different block to the assistant without either party seeing both.
  The raw event stream persists all blocks with their annotations, and both the
  `raw` `ReadEvents` view and `inline_results = "full"` expose the full output
  on demand.
  By convention, audience-split blocks must be additive or reformulated, never
  contradictory; for `tell_user`, the assistant block is a fixed
  acknowledgement, so divergence is not a concern.
- **Consumers must adopt the user view.** Consumers that pattern-match raw event
  kinds will miss user-addressed tool blocks.
  Because [RFD 058] already requires consumers to handle content blocks, the
  canonical user-view accessor gives chat-style consumers a single entry point.

## Alternatives

### Strip the tool call and persist a `ChatResponse` instead

The tool result acknowledgement is what continues the turn loop: without a tool
result, the next provider request has nothing valid to send short of fabricating
a user message or relying on assistant-prefill continuation, which does not
compose with tool calls across providers.
Providers also validate the replayed transcript structure: Anthropic thinking
signatures and Google thought signatures are bound to the exact event sequence,
and rewriting a tool call into chat text breaks signature validation.
Replacing the tool call with a `ChatResponse` also erases the distinction
between a mid-turn update and a final answer in the persisted record.

### Persist the pair plus a display-only `ChatResponse` event

Materializing the user view as a separate event duplicates the message for every
stream consumer (`grep`, compaction, export) and requires a provider-ignore
marker that silently leaks duplicate content if any code path omits the check.
Filtering by `audience` computes the same view on read without duplicate state.

### Event-synthesis user-projection

Another option is to rewrite `tell_user` request/response pairs into synthetic
`ChatResponse` events at view boundaries.
Its only advantage, keeping downstream consumers unchanged, disappears once [RFD
058] changes the serialized `ToolCallResponse` shape and requires every consumer
to handle content blocks anyway.
Audience filtering achieves the same result without synthesizing events.

### Carry the message in a `resource` block

`resource` blocks already have a `mimeType` field, which would avoid extending
`text` blocks.
However, a resource represents identified content with a required URI that
drives URI canonicalization, checksums, and content deduplication ([RFD 058],
[RFD 066], [RFD 067]).
A `tell_user` message has no resource identity, so wrapping it in a `resource`
block would require a fabricated URI and feed ephemeral prose into file-oriented
deduplication machinery.
The optional `mimeType` field on `text` blocks ([RFD 058]) matches the actual
semantics (unidentified text with a presentation hint) and follows the JP
extension pattern [RFD 065] establishes on `Resource`.

### A typed `ToolCallResponse.user_message` field

Adding a dedicated `user_message` field on `ToolCallResponse` would introduce a
JP-specific field for a concept that [RFD 058] content blocks already express
through `audience` annotations.

### Custom argument formatter

Rendering the message through `style.parameters = "<command>"` (the pattern used
by `fs_modify_file`) fails on three counts: custom formatter output is printed
verbatim rather than through the markdown pipeline, argument formatters only run
in the terminal, and a builtin tool should not depend on spawning an external
command for its default rendering.

### A `ParametersStyle::Markdown` variant

Adding a `ParametersStyle::Markdown` config variant would expose a user-facing
setting with only a single consumer.
Driving markdown rendering from the response block's `mimeType` places the
presentation hint on the content itself and works for any tool.

### Ship as a local tool instead of a builtin

Shipping `tell_user` as a local tool is viable (see [Local-tool
equivalence](#local-tool-equivalence)), but packaging it as a builtin makes it
available out of the box until a tool registry exists for bundled tools.

## Non-Goals

- **Cross-frontend rendering of custom argument formatters.** Custom formatter
  output remains terminal-only.
- **Notification routing.** Queueing, deduplicating, and escalating
  user-addressed messages belong to [RFD 011].
- **Acting on other MCP annotations.** `priority` and `lastModified` remain on
  the annotation types without runtime behavior.
- **Formalizing the bundled-vs-privileged builtin taxonomy.** That distinction
  belongs in a separate decision RFD alongside the tool registry work.
- **Default elicitation prompts.** Persona and workspace configuration control
  when the system prompt encourages the assistant to call `tell_user`.

## Risks and Open Questions

- **Dependency on [RFD 058].** [RFD 058] is still in Discussion; this RFD builds
  on its content block model and optional text-block `mimeType` field.

- **Live and replay rendering equivalence.** Live rendering formats blocks when
  the tool finishes, while terminal replay formats raw events per turn using the
  same display settings (`hidden`, `header`, `inline_results`; terminal replay
  does not use the user view).
  Because two code paths must produce identical terminal output, an equivalence
  test will compare live terminal output against replayed output, following the
  existing streaming-identity test suite.

- **Model elicitation.** Models may under-use `tell_user` without system-prompt
  guidance.
  This affects utilization rather than correctness: the tool description is
  always visible to the model, and an assistant that never calls `tell_user`
  falls back to ending the turn to speak.
  If under-use persists during rollout, a future RFD can add tool-contributed
  prompt sections tied to the tool's enable state instead of hardcoding per-tool
  instructions in the system prompt.

- **`serve-web` adaptation.** Updating the web viewer for [RFD 058] content
  blocks can reuse its existing markdown renderer for user-audience blocks.
  When rendering the `user` view, `serve-web` keys tool output on
  `tool_call_response` events directly rather than pairing them with
  `tool_call_request` events, which are omitted from that view.

- **Inquiry events in the user view.** User-facing inquiry exchanges belong in
  the user view because they contain the user's own answers, whereas
  assistant-resolved inquiries do not.
  Distinguishing the two requires [RFD 082]'s source attribution, so
  inquiry-family filtering rules are deferred to that RFD.

- **Compaction.** Tool-call compaction policies drop request/response pairs
  entirely, which removes `tell_user` messages from the compacted user view.
  Whether compaction should retain user-audience blocks (similar to how [RFD
  058] retains resource metadata when dropping content) remains an open
  question.

  > [!WARNING]
  > With a built-in compaction tool, the assistant can select compaction ranges
  > itself rather than relying on user-chosen ranges.
  > Compacting a range that contains user-audience blocks would remove those
  > messages from the user view without the user choosing that range, so this
  > question must be resolved before promoting this RFD.
  > Because raw events are always preserved, this affects view fidelity rather
  > than data durability.

## Implementation Plan

### Phase 1: Audience filtering and the user view

Depends on [RFD 058] phases 1 and 2 (types, `ToolCallResponse` migration, and
MCP passthrough).

- Add the user-view accessor to `jp_conversation` and the *Audience* and *User
  View* glossary entries.
- Filter blocks by `audience` in provider conversion, substituting the neutral
  placeholder when all blocks are filtered out.
- Add the per-audience map form to `inline_results` (preserving scalar values
  for both audiences and allowing `chat` on `user`).
- Add the `view` selector (`raw` by default, or `user`) to `ReadEvents`.
- Test that assistant-audience blocks never reach the user view, user-audience
  blocks never reach provider payloads, and all-user-audience successes produce
  the neutral placeholder across each provider.
- Test `inline_results` audience precedence: unannotated blocks use the more
  permissive setting, `{ user = 10, assistant = "off" }` truncates
  user-addressed blocks and hides assistant-addressed blocks, and `{ user =
  "chat" }` renders user-annotated blocks as speech on stdout while keeping
  unannotated blocks in result display.
- Test that the `user` view retains `Message` and `Structured` chat responses,
  drops `Reasoning` responses, and drops tool calls without user-audience
  blocks.

### Phase 2: Markdown block rendering

Route `text/markdown` text blocks (using the optional `mimeType` field from [RFD
058]) through `jp_md`'s public rendering API, and extend the shared contract
tests to cover block rendering.

### Phase 3: `style.header`

Add the `style.header` setting (boolean or `"on"` / `"off"`) and suppress the
temporary streaming line when disabled, including `PartialConfigDelta`,
`FillDefaults`, `ToPartial`, and config snapshot tests.

### Phase 4: The `tell_user` builtin

Implement the `tell_user` executor, registration, description, and parameter
schema, and document in the module comment that the tool uses only generic
mechanisms and ships as a builtin for packaging alone.
Test argument validation, response block structure, provider and user view
filtering, turn-loop continuation, and live-versus-replay rendering equivalence.

## References

- [RFD 058]: Typed content blocks; defines the block model, the optional
  text-block `mimeType` field, and the `audience` annotation types.
- [RFD 065]: The MCP-compatible superset principle governing the text-block
  `mimeType` extension.
- [RFD 081]: The `enable = { state, allow_toggle }` tool configuration shape.
- [RFD 082]: Unified inquiry event recording, whose source attribution is a
  prerequisite for filtering inquiry events in the user view.
- [RFD 083]: The built-in tool registration pattern and `if_named` enable
  policy.
- [RFD 072]: The command plugin registry direction for distributing bundled
  tools.
- [RFD 048]: The four-channel output model under which the terminal routes
  `chat`-classified blocks to stdout.
- [RFD 011]: System notification queue (out of scope for this RFD).
- [Prompting Claude Fable]: Anthropic's `send_to_user` guidance for long-running
  agents.

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
