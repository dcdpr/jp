# RFD D60: Plugin Prompt Channel

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-25
- **Extends**: [RFD 072]
- **Summary**: Adds a `prompt` request to the command plugin protocol so plugins
  can ask confirm, select, text, and secret questions.

## Summary

This RFD adds a `prompt` request to the command plugin protocol ([RFD 072]).
A plugin sends a structured question (`confirm`, `select`, `text`, or `secret`),
and the host renders it on the terminal through `PromptBackend` and returns the
user's answer.

## Motivation

A command plugin's stdin and stdout carry the host protocol, so the plugin
cannot read from the terminal directly.
The protocol lets a plugin emit output (`print`, `log`) but provides no way to
request user input such as a confirmation before a destructive action, a choice
between alternatives, or a pasted value.

For example, a plugin that prunes stale data cannot ask "delete 14 entries?"
It must proceed silently, refuse to run, or prompt outside the protocol by
driving the terminal directly.
Direct terminal access breaks the [RFD 072] contract that only the host touches
the user's terminal and loses the host's styling, keybindings, and interrupt
handling.

The host already provides `PromptBackend` (`jp_inquire::prompt`) with
`inline_select`, `text`, and `select`, the same primitives every host prompt
uses.
Plugins only lack a protocol message to reach them.

## Design

### Wire format

A plugin sends a `prompt` request and blocks until the host replies with
`prompt_response`.
Like `read_config` and `config` in [RFD 072], the request and response support
an optional echoed `id` so plugins can pipeline concurrent requests.

The request carries a tagged `kind`:

```json
{
  "type": "prompt",
  "kind": "confirm",
  "message": "Delete 14 stale entries?",
  "default": false
}
```

```json
{
  "type": "prompt",
  "kind": "select",
  "message": "Which branch?",
  "options": ["main", "develop", "release"]
}
```

```json
{
  "type": "prompt",
  "kind": "text",
  "message": "Commit message"
}
```

```json
{
  "type": "prompt",
  "kind": "secret",
  "message": "Registry token"
}
```

The host replies with an `outcome`:

```json
{"type": "prompt_response", "outcome": "answered", "value": true}
{"type": "prompt_response", "outcome": "cancelled"}
{"type": "prompt_response", "outcome": "unavailable", "reason": "no interactive terminal"}
```

`value` is typed by `kind`: a bool for `confirm`, the chosen string for
`select`, and the entered string for `text` and `secret`.
`cancelled` means the user dismissed the prompt (ESC).
`unavailable` means the host has no interactive terminal; the plugin can then
emit its own diagnostic naming a non-interactive alternative, so plugins stay
scriptable by default instead of blocking.

### Question kinds

The `kind` tag mirrors `jp_tool::AnswerType` (`Boolean`, `Select`, `Text`, and
`Secret` from [RFD 082]), which `ToolPrompter::prompt_question` uses to dispatch
inquiries.
Reusing that model at the protocol boundary keeps a single concept of a question
with an answer type, and adding a new kind requires only a new enum variant
rather than a separate request and response pair.

Each kind maps directly to a `PromptBackend` method:

- `confirm` calls `inline_select` (y/n).
- `select` calls `select`.
- `text` calls `text`.
- `secret` calls the no-echo `password` method added by [RFD 082].

Because every kind renders through `PromptBackend`, plugin prompts share the
host's terminal styling, keybindings, and interrupt handling.

### Rendering in the host shell

`jp_plugin` defines only the protocol message types.
All terminal rendering lives in `jp_cli`.

The plugin message loop in `cmd/plugin/dispatch.rs` holds only the workspace,
config, and shutdown flag.
Rendering a prompt requires passing a narrow prompt context (`Arc<Printer>`,
`Arc<dyn PromptBackend>`, and the host's interactivity signal) into the loop
rather than exposing the full `Ctx`.

Passing `Arc<Printer>` into the loop also completes `PluginToHost::Print`, which
is a Phase-1 placeholder that writes raw bytes to stdout with a TODO to route
through `Printer`.
Both `prompt` and `print` use the same `Printer` handle.

### Typed outcomes

`cancelled` and `unavailable` are `outcome` variants on `prompt_response` rather
than [RFD 072]'s shared `error` message.
Cancellation is a normal user action, not a protocol failure, and a
single-threaded shell plugin can branch on `outcome` with one `jq` check instead
of distinguishing a refused prompt from an earlier `read_events` failure.

The host gates `unavailable` on its interactivity signal (`Ctx::term.is_tty`).
This RFD does not define a new non-interactive policy or depend on [RFD 049];
when [RFD 049] lands (`route_prompt()`, `has_client`, `--non-interactive`), the
dispatch loop can call that router without changing the wire format.

### Secret prompts

A `secret` prompt reads input without terminal echo (via [RFD 082]'s
`PromptBackend::password` method) and omits the answer from logs.
Representing secrets as a dedicated `kind` rather than a `secret: bool` flag on
`text` matches `AnswerType::Secret` in [RFD 082] and prevents invalid
combinations such as a secret `confirm` or `select`.

The conversation redaction machinery in [RFD 082] (`InquiryResponse::Redacted {
id }` and `<redacted>` rendering) does not apply here because it handles secret
answers persisted to the conversation stream.
A plugin prompt is an ephemeral request/response exchange and never produces an
`InquiryEvent`.
The dispatch loop only needs to exclude `secret` answers from its `trace!(?msg)`
logging.

### Interrupt handling

The plugin dispatcher already participates in the layered interrupt handler
stack ([RFD 045]): `run_plugin` holds an interrupt guard, and the shutdown
thread relays `Shutdown` to the plugin on escalation.
`TerminalPromptBackend` puts the terminal in raw mode while rendering a prompt.
Under [RFD 045]'s dual-delivery model, pressing Ctrl-C during the prompt
delivers byte `0x03` to the prompt library rather than `SIGINT` to the signal
router, so the blocking prompt cancels itself without waiting on the shutdown
thread.

Like a tool question, a plugin prompt is a non-interrupt prompt and reuses the
existing cancel-versus-interrupt mapping:

- ESC (`OperationCanceled`) replies with `outcome: "cancelled"`.
- Ctrl-C (`OperationInterrupted`) escalates to graceful shutdown, which sends
  `Shutdown` to the plugin.

The message loop must not hold the plugin-stdin write lock across the blocking
terminal read, so the router's normal-mode path and the shutdown thread remain
unobstructed before raw mode engages.
The loop acquires the lock only when writing `prompt_response`.

## Drawbacks

- **Single-threaded plugins block while the prompt is up.** A shell script that
  sends `prompt` and waits for `prompt_response` cannot do other work while the
  user answers.
  Multi-threaded plugins that need concurrency can pipeline requests using the
  `id` field.

- **Social-engineering surface.** A plugin can render arbitrary prompt text,
  including text that impersonates the host ("JP: enter your password").
  `RunPolicy` already governs whether a plugin may run, and an allowed plugin
  can already print arbitrary text to the terminal.

- **Synchronous round-trip per question.** Each prompt requires a request, a
  blocking terminal read, and a response.
  Because the user is the bottleneck in interactive prompts, the protocol
  overhead is negligible, though the channel is unsuitable for batch throughput.

## Alternatives

### `secret: bool` flag on `text`

A separate boolean flag on `text` (rejected in [RFD 082] for inquiries as well)
would make meaningless combinations such as a secret `confirm` or secret
`select` representable.
Making `secret` a `kind` variant rules out those combinations in the schema and
matches `AnswerType`.

### Overload the shared `error` message for `cancelled` and `unavailable`

Returning [RFD 072]'s shared `error` message when a prompt is cancelled or
unavailable would treat normal outcomes as protocol failures and force plugins
to disambiguate a refused prompt from an unrelated failed request.
A typed `outcome` on `prompt_response` keeps prompt results self-contained.

### Separate message types (`confirm`, `select`, `prompt_text`)

Defining a distinct request type per question shape would triple the handler and
response surface, and each new question shape would require a new top-level
message type instead of a `kind` variant.

### Reuse [RFD D18]'s interactive events

[RFD D18] adds interactive events in the opposite direction: the host pushes its
own inquiry to a plugin to render in a custom UI such as a browser (`respond:
"inquiry"`).
This RFD handles the reverse, where the plugin has a question and asks the host
to render it on the terminal.
The two features target different plugins (a shell script can use `prompt` with
`read`, whereas [RFD D18] requires a multi-threaded plugin with event
subscriptions) and can ship independently as complementary extensions to [RFD
072].

## Non-Goals

- **Host-to-plugin interactive events.** Pushing the host's own tool-approval
  and inquiry prompts to a plugin frontend is covered by [RFD D18].
  T-1hetcnt plans that part for turns a plugin starts.
- **Recording prompts in the conversation stream.** Plugin prompts are ephemeral
  and are not persisted as `InquiryEvent` records.
- **Multi-question forms.** One `prompt` asks one question.
  Batching, branching, and cancel-UX for multi-field forms are out of scope.
- **A non-interactive answer policy.** Beyond replying `unavailable`, deciding
  how a non-interactive host resolves prompts (auto-confirm, default, or fail)
  belongs to [RFD 049].

## Risks and Open Questions

- **Interrupt window.** Interrupt handling depends on releasing the plugin-stdin
  write lock across the blocking terminal read and on [RFD 045]'s raw-mode
  dual-delivery path.
  A test must verify that pressing Ctrl-C during a plugin prompt escalates to
  shutdown rather than wedging the terminal.

- **Stdout hygiene.** Prompt rendering must write to the terminal through
  `Printer` and `PromptBackend`, never to the plugin's stdout handle, to avoid
  corrupting the JSON-lines protocol stream.

- **Secret leakage.** Excluding `secret` answers from logs requires bypassing
  the dispatch loop's `trace!(?msg)` call and covering that path with a test.

## Implementation Plan

### Phase 1: Core prompt (confirm / select / text)

- Add `PluginToHost::Prompt` and `HostToPlugin::PromptResponse` (with the
  `outcome` enum) to `jp_plugin::message`.
- Pass the prompt context (`Arc<Printer>`, `Arc<dyn PromptBackend>`, and the
  interactivity signal) into the dispatch message loop.
- Render `confirm`, `select`, and `text` prompts through `PromptBackend`, gating
  `unavailable` on `Ctx::term.is_tty`.
- Route `unavailable`, `cancelled`, and `answered` outcomes, reusing the
  tool-question ESC/Ctrl-C mapping from [RFD 045].
- Route `PluginToHost::Print` through the same `Printer` context to replace the
  raw-stdout placeholder.

Phase 1 depends on [RFD 072] Phase 1 (protocol core) and can be reviewed and
merged on its own.

### Phase 2: Secret prompt

- Add the `secret` kind and render it through [RFD 082]'s no-echo `password`
  method on `PromptBackend`.
- Exclude `secret` answers from the dispatch loop's message tracing.

Phase 2 depends on [RFD 082] landing the `password` backend method and follows
Phase 1 without blocking it.

## References

- [RFD 072: Command Plugin System][RFD 072]: Defines the command plugin protocol
  that this RFD extends.
- [RFD 082: Unified inquiry event recording][RFD 082]: Introduces
  `AnswerType::Secret` and the `password` backend method.
- [RFD 045: Layered Interrupt Handler Stack][RFD 045]: Defines the interrupt
  handler stack, raw-mode dual delivery, and the cancel-versus-interrupt
  mapping.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]: Defines
  the interactivity policy that will govern `unavailable`.
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18]: Defines
  host-to-plugin interactive events, complementing this plugin-to-host prompt
  channel.

[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 082]: ../082-unified-inquiry-event-recording.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
