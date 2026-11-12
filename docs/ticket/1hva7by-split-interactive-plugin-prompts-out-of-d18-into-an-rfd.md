# Split interactive plugin prompts out of D18 into an RFD

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=049
- **Label**: type=pre-rfd

D18 (Plugin Event Subscriptions and Query Delegation) bundles two features: live
event streaming to plugins, and forwarding interactive prompts from a
plugin-delegated turn back to the plugin.
Prompt forwarding is needed independently by T-1hetcnt to support tool-call
approvals over the chat bridge.
Split interactive plugin prompts into a dedicated RFD and narrow D18 to event
subscriptions.

What is settled so far:

- **Delegated turns run interactively through the plugin.** The host sends every
  prompt it would show at a terminal to the plugin, and the plugin chooses which
  prompts to present to the user and which to answer automatically or decline.
- **Structured prompt kinds.** Each prompt carries its kind (tool approval, tool
  question, or result review), the question text, and the candidate answers.
  Each answer has a name, an optional description, and a short key so a terminal
  UI can display keys while a chat UI displays names, rather than exposing raw
  `y/Y/n/N/p/r/e` terminal menu keys.
- **Declining.** Every prompt can be answered with `decline`.
  A question with a default uses that default; otherwise the host applies the
  detached prompt fallback from RFD 049 (refusing the tool approval and
  continuing the turn).
- **Secrets stay on the host.** Secret prompts are never sent over the plugin
  protocol; the host declines them directly.
- **Protocol-version gated.** The host sends prompts only to plugins that
  declare the protocol version introducing them.
  `jp-serve-web` moves to that version and declines all prompts, preserving its
  current behavior.

Open questions: the exact response payloads (D18 uses `approve` / `reject` /
`modify` for approvals and `answer` for inquiries), the host timeout before
declining on the plugin's behalf, and which other `interactive` checks in the
turn loop need a dedicated prompt kind.

Related: RFD 049 (non-interactive prompt policy), D60 (the reverse direction: a
plugin asking the host), D18.
