# Ask for tool-call approval over the chat bridge

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=049
- **Label**: type=feature
- **Parent**: T-1e92tft

A turn the bridge starts runs as if nobody is there to ask, so every tool the
persona enables runs without asking: `prepare_turn` in `dispatch.rs` passes
`interactive = false`, and `decide_permission` in `coordinator.rs` approves `run
= "ask"` and `run = "edit"` when nobody is there.
The persona's tool list is the only control, and a write nobody meant, such as a
duplicate ticket from a correction that arrived mid-call, lands before anyone
can stop it.

What the chat should look like:

```text
JP: I'm going to file "Search crashes on an empty query" (type=bug) and rank
    it after T-0abc123. Go ahead?
Ada: yes, but make it a feature
```

## Design

A turn a plugin starts runs as interactive, and the plugin is the one at the
other end.
It is asked every question the host would ask a person at a terminal, and it may
answer any of them with a fixed reply: answering some for real and the rest with
fixed replies is the plugin's choice.

This is the "answer interactive prompts" part of RFD D18, without D18's live
event subscriptions.
Use D18's message shapes, a `tool_approval` answered `approve`, `reject`, or
`modify` with new arguments, and an `inquiry` answered with an `answer`, so the
two designs don't drift apart.

- **Protocol.** A host-to-plugin prompt that says what kind of question it is (a
  tool approval, a tool's own question, a result review) and carries what the
  plugin needs to show it, and a plugin-to-host answer.
  Every kind can also be answered `decline`, which makes the host do what it
  does today with nobody there; a plugin that doesn't answer in time declines.
- **Version.** The host sends prompts only to a plugin built for the protocol
  version that adds them, so an older installed plugin keeps today's behaviour
  instead of failing on a message it can't read.
- **`jp-serve-web`.** Moves to that version and declines every prompt, which is
  exactly today's behaviour; its UI doesn't change.
- **Host.** A delegated turn runs with `interactive = true` and passes its
  prompts to the plugin.
  Forward them where the host knows what it is asking (the tool prompter and the
  inquiry handling, which D18 also names), not as raw menus: a menu of
  `y/Y/n/N/p/r/e` keys means nothing to a plugin.
  Check everything that reads `interactive` on this path; each prompt it guards
  has to reach the plugin as one of the kinds above.
- **Bridge.**
  - While a turn waits for approval, the next message from the person the turn
    runs as is the answer, not a reply or a new turn.
  - The question is a short chat message: which tool, and what it will do.
  - One more decide API question reads the answer: approve, deny, or approve
    with changes, which go back as `modify` or as a reply.
  - No answer in time denies.
  - Secret prompts are always declined, so a password never goes into a chat;
    other kinds are declined at first.
- **Persona.** Write tools to `run = "ask"`: T-1hg18ev.

This replaces delaying writes (T-1hetcnv) as the fix for corrections that arrive
mid-call: an approval waits as long as the person needs.

Noted in RFD 049 (a delegated turn gets someone to ask), and in D60 and D18.

- [ ] Protocol messages and version
- [ ] Host: interactive delegated turns that pass prompts to the plugin
- [ ] `jp-serve-web` declines every prompt
- [ ] Bridge: approval state, question text, decide, timeout
