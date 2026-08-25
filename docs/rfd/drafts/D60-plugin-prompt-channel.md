# RFD D60: Plugin Prompt Channel

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-25
- **Extends**: [RFD 072]

## Summary

This RFD adds a `prompt` request to the command plugin protocol ([RFD 072]).
A plugin describes a question — confirm, select, text line, or secret — and
the host renders it through its own prompt facilities, then replies with the
answer.
This mirrors how tool inquiries already work: the component that needs an answer
describes the question; the component that owns the terminal asks it.

## Motivation

A command plugin's stdin and stdout carry the host protocol, so a plugin cannot
read from the terminal.
The protocol lets a plugin *emit* output (`print`, `log`) but not *request*
input.
There is no way to ask the user a question: a confirmation before a destructive
action, a choice between alternatives, a pasted value.

A plugin that prunes stale data has no way to ask "delete 14 entries?"
Today it must either proceed silently, refuse to run, or reimplement a prompt
outside the protocol — which means driving the terminal directly, breaking the
[RFD 072] contract that only the host touches the user's terminal, and losing
the host's styling, keybindings, and interrupt handling.

The host already has everything needed to ask: `PromptBackend`
(`jp_inquire::prompt`) with `inline_select`, `text`, and `select`, the same
primitives every host-owned prompt uses.
The missing piece is a way for the plugin to reach them.

## Design

### Wire format

A plugin sends a `prompt` request and blocks until the host replies with
`prompt_response`.
This slots into [RFD 072]'s existing request/response grain — like
`read_config` → `config` — including the optional echoed `id` for plugins that
pipeline concurrent requests.

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
`select`, the entered string for `text` and `secret`.
`cancelled` means the user dismissed the prompt (ESC).
`unavailable` means the host had no interactive terminal to render into; the
plugin converts it into its own diagnostic naming its non-interactive
alternative, so plugins stay scriptable by default rather than blocking.

### One request, a tagged kind

The `kind` enum mirrors the host's own model of a question.
`jp_tool::AnswerType` (`Boolean` / `Select` / `Text`, plus `Secret` from [RFD
082]) is how `ToolPrompter::prompt_question` already dispatches inquiries.
Reusing that shape at the protocol boundary keeps one ubiquitous concept — "a
question with an answer type" — rather than inventing a second vocabulary, and
makes a future kind a single variant instead of a new message type, a new
handler, and a new response.

The kinds map directly onto the existing backend: `confirm` → `inline_select`
(y/n), `select` → `select`, `text` → `text`, `secret` → the no-echo
`password` method added by [RFD 082].
The terminal styling, keybindings, and interrupt behavior are inherited for
free.

### Rendering stays in the host shell

`jp_plugin` defines only the message types; it stays a pure protocol-types
crate.
All rendering lives in `jp_cli`, which owns the terminal.

The host's plugin message loop (`cmd/plugin/dispatch.rs`) currently has no
printer and no prompt backend — it holds only the workspace, config, and
shutdown flag.
Rendering a prompt requires threading a narrow prompt context — `Arc<Printer>`,
`Arc<dyn PromptBackend>`, and the host's interactivity signal — into the loop.
This is deliberately the minimum surface: the loop does not gain access to the
whole `Ctx`.

Threading the printer in also closes an existing gap: `PluginToHost::Print` is a
Phase-1 placeholder that writes raw bytes to stdout with a TODO to route through
the `Printer`.
The same context that renders prompts routes `print` correctly, so the cost is
paid once.

### Outcomes are typed, not errors

`cancelled` and `unavailable` are modelled as outcomes on `prompt_response`, not
as [RFD 072]'s shared `error` message.
Cancellation is a normal result, not a failure, and a single-threaded shell
plugin branches on `outcome` with one `jq` check instead of distinguishing "my
prompt was refused" from "my earlier `read_events` failed."

`unavailable` is gated on the host's interactivity signal, which today is
`Ctx::term.is_tty`.
This RFD does not build a new policy for it.
When [RFD 049] lands (`route_prompt()` / `has_client`, `--non-interactive`),
this call site becomes one more consumer of that router; the seam is left in
place, not depended on.

### Secret prompts

A `secret` prompt uses no-echo input and its answer is never logged.
Secret-ness is a `kind`, not a `secret: bool` flag on `text` — the same
decision [RFD 082] makes for inquiries, where encoding it as an
`AnswerType::Secret` variant keeps the ambiguous "secret boolean" and "secret
select" shapes unrepresentable.
The `secret` kind consumes the no-echo `password` method [RFD 082] adds to
`PromptBackend`.

[RFD 082]'s redaction machinery does not apply here.
That machinery — `InquiryResponse::Redacted { id }`, the `<redacted>` rendering
— concerns secret answers persisted to the *conversation stream*.
A plugin prompt is ephemeral request/response and never becomes an
`InquiryEvent`.
The only never-logged obligation on this channel is that a secret answer must
not flow through the loop's `trace!(?msg)` message logging.

### Interrupt handling

The dispatcher already participates in the layered interrupt handler stack ([RFD
045]): `run_plugin` holds an interrupt guard, and the shutdown thread relays
`Shutdown` to the plugin on escalation.
A plugin prompt renders through `TerminalPromptBackend`, which puts the terminal
in raw mode, so per [RFD 045]'s dual-delivery model a Ctrl-C *during the prompt*
arrives as byte `0x03` to the prompt library rather than as SIGINT to the
router.
The blocking prompt cancels itself; the host does not need the shutdown thread
to unblock it.

A plugin prompt is a *non-interrupt* prompt, in the same category as a tool
question, so it reuses the existing cancel-versus-interrupt mapping:

- ESC (`OperationCanceled`) → reply `outcome: "cancelled"`.
- Ctrl-C (`OperationInterrupted`) → escalate to graceful shutdown, which sends
  the plugin its existing `Shutdown`.

The one implementation note: the loop must not hold the plugin-stdin write lock
across the blocking terminal read, so the router's normal-mode path and the
shutdown thread stay unobstructed in the brief window before raw mode engages.
The lock is acquired only to write `prompt_response`.

## Drawbacks

- **Single-threaded plugins block while the prompt is up.** A shell script that
  sends `prompt` and reads the response blocks until the user answers.
  This is the intended behavior — the plugin asked a question — but it means a
  plugin cannot do other work while a prompt is pending.
  Multi-threaded plugins that need concurrency use the `id` field.

- **Social-engineering surface.** A plugin can render arbitrary prompt text,
  including text that impersonates the host ("JP: enter your password").
  This is not gated further here: `RunPolicy` already governs whether a plugin
  runs at all, and a plugin the user has chosen to run can already print
  arbitrary text.

- **Another synchronous round-trip.** Each prompt is a request, a blocking
  render, and a response.
  For interactive use this is free (the human is the bottleneck); it is not a
  batch-throughput path.

## Alternatives

### Secret as a `secret: bool` flag on `text`

Rejected for the same reason [RFD 082] rejects it for inquiries: a boolean on a
separate axis makes "secret boolean" and "secret select" representable and
meaningless.
A `kind` variant keeps the type space honest and matches the host's
`AnswerType`.

### Overload the shared `error` message for cancel and unavailable

Rejected because cancellation is a normal outcome, not a failure, and folding
both into `error` forces every plugin to disambiguate a refused prompt from an
unrelated failed request.
A typed `outcome` keeps prompt semantics self-contained.

### One message type per shape (`confirm` / `select` / `prompt_text`)

Rejected because it triples the handler and response surface and makes each new
shape a new message type rather than a new enum variant.
One `prompt` with a tagged `kind` composes better.

### Reuse [RFD D18]'s interactive events

[RFD D18] adds interactive events, but in the opposite direction: the *host*
pushes its own inquiry to the plugin to render in a browser (`respond:
"inquiry"`).
This RFD is the reverse — the *plugin* has a question and wants the *host's
terminal* to ask it.
Different direction, different audience (a shell script can use `prompt` with
`read`; [RFD D18] needs a multi-threaded plugin with subscriptions), and
independently shippable.
The two are complementary axes on [RFD 072], not the same feature.

## Non-Goals

- **Host-to-plugin interactive events.** Pushing the host's own tool-approval
  and inquiry prompts to a plugin frontend is [RFD D18]'s concern, not this one.
- **Recording prompts in the conversation stream.** A plugin prompt is
  ephemeral.
  It is not an `InquiryEvent` and does not persist.
- **Multi-question forms.** One `prompt` asks one question.
  Batching, branching, and cancel-UX for multi-field forms are out of scope.
- **A non-interactive answer policy.** Beyond replying `unavailable`, deciding
  *how* a non-interactive host should answer (auto-confirm, default, fail) is
  [RFD 049]'s concern.

## Risks and Open Questions

- **Interrupt window.** The correctness of the interrupt behavior rests on the
  loop releasing the plugin-stdin write lock across the blocking read and on the
  raw-mode dual-delivery path from [RFD 045].
  This needs a test that a Ctrl-C during a plugin prompt escalates rather than
  wedging the terminal.

- **Stdout hygiene.** The prompt render writes to the terminal, not to the
  plugin's stdout, so it does not corrupt the JSON-lines stream.
  This holds only as long as rendering goes through the `Printer` / prompt
  backend and never through the plugin's own stdout handle.

- **Secret leakage.** The never-logged guarantee depends on the `secret` answer
  bypassing the loop's `trace!(?msg)`.
  This is a discipline the implementation must enforce and a test must pin.

## Implementation Plan

### Phase 1: Core prompt (confirm / select / text)

- Add `PluginToHost::Prompt` and `HostToPlugin::PromptResponse` (with the
  `outcome` enum) to `jp_plugin::message`.
- Thread a narrow prompt context (`Arc<Printer>`, `Arc<dyn PromptBackend>`,
  interactivity signal) into the dispatch message loop.
- Render `confirm` / `select` / `text` through the existing `PromptBackend`;
  gate `unavailable` on `Ctx::term.is_tty`.
- Route `unavailable`, `cancelled`, and `answered` outcomes; reuse the
  tool-question ESC/Ctrl-C mapping from [RFD 045].
- Route `PluginToHost::Print` through the same `Printer` context, closing the
  existing placeholder.
- Depends on [RFD 072] Phase 1 (protocol core).
  Reviewable and mergeable on its own.

### Phase 2: Secret prompt

- Add the `secret` kind and render it through [RFD 082]'s no-echo `password`
  method on `PromptBackend`.
- Ensure the secret answer bypasses the loop's message tracing.
- Depends on [RFD 082] landing the `password` backend method.
  Sequenced after Phase 1; does not block it.

## References

- [RFD 072: Command Plugin System][RFD 072] — the protocol this extends.
- [RFD 082: Unified inquiry event recording][RFD 082] — the
  `AnswerType::Secret` model and the `password` backend method.
- [RFD 045: Layered Interrupt Handler Stack][RFD 045] — the interrupt stack the
  dispatcher already uses; the raw-mode dual-delivery and cancel/escalate
  mapping.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049] — the
  future home of the interactivity policy `unavailable` is gated on.
- [RFD D18: Plugin Event Subscriptions and Query Delegation][RFD D18] — the
  host-to-plugin interactive-events axis, complementary to this one.

[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 072]: ../072-command-plugin-system.md
[RFD 082]: ../082-unified-inquiry-event-recording.md
[RFD D18]: D18-plugin-event-subscriptions-and-query-delegation.md
