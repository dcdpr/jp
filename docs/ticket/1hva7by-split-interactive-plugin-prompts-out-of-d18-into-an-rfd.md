# Split interactive plugin prompts out of D18 into an RFD

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=049
- **Label**: type=pre-rfd

D18 (Plugin Event Subscriptions and Query Delegation) bundles two things: live
event streaming to plugins, and letting a plugin answer the prompts a turn it
started would put to a person.
The second is wanted on its own: it is what T-1hetcnt needs to ask for tool-call
approval over the chat bridge.
Take it out into its own RFD, and leave D18 with the event subscriptions.

What is settled so far:

- **Interactive means someone answers every question.** A turn a plugin starts
  runs as interactive, and the plugin is that someone.
  It is asked every question the host would ask a person at a terminal, and may
  answer any of them with a fixed reply: answering some for real and the rest
  with fixed replies is the plugin's choice.
- **A question says what it is.** A prompt carries its kind (a tool approval, a
  tool's own question, a result review), the question, and its answers.
  Each answer has a name, an optional description, and a short key, so a
  terminal can show keys and a chat can show names.
  A raw menu of `y/Y/n/N/p/r/e` keys means nothing to a plugin.
- **Declining.** Every prompt can be declined.
  A question with a default takes it; one without fails the way RFD 049
  describes for nobody being there (a tool approval is refused, and the turn
  continues).
- **Secrets never leave the host.** A plugin is never sent a secret prompt; the
  host declines it.
- **No opt-in flag.** The host sends prompts only to plugins built for the
  protocol version that adds them.
  `jp-serve-web` moves to that version and declines everything, which keeps its
  behaviour unchanged.

Open: the answer shapes (D18 has `approve` / `reject` / `modify` for approvals
and `answer` for inquiries), how long the host waits before declining on the
plugin's behalf, and which other paths that read `interactive` need a prompt
kind of their own.

Related: RFD 049 (non-interactive prompt policy), D60 (the reverse direction: a
plugin asking the host), D18.
