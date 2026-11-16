# Ask for tool-call approval over the chat bridge

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-11
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=049
- **Label**: type=feature

A delegated turn started by the bridge runs non-interactively, so every tool
enabled by the persona executes without prompting: `prepare_turn` in
`dispatch.rs` passes `interactive = false`, and `decide_permission` in
`coordinator.rs` auto-approves `run = "ask"` and `run = "edit"` when no
interactive client is present.
The persona's tool list is the only guard, so an unintended write (such as a
duplicate ticket when a correction arrives mid-call) executes before the sender
can confirm or cancel it.

What the chat should look like:

```text
JP: I'm going to file "Search crashes on an empty query" (type=bug) and rank
    it after T-0abc123. Go ahead?
Ada: yes, but make it a feature
```

## Design

Run plugin-delegated turns as interactive, with the plugin acting as the prompt
client.
The host forwards every prompt it would show in a terminal to the plugin, and
the plugin can either present the prompt to its user or return a declined
response so the host applies its non-interactive fallback.

This is the interactive prompt portion of RFD D18 without D18's live event
subscriptions, tracked as a standalone RFD in T-1hva7by.
Reuse D18's message shapes (`tool_approval` answered with `approve`, `reject`,
or `modify` with updated arguments, and `inquiry` answered with `answer`) so the
two designs stay aligned.

- **Protocol.** Add a host-to-plugin prompt message carrying the prompt kind
  (tool approval, tool question, or result review) and its options, paired with
  a plugin-to-host response.
  Any prompt kind can be answered with `decline`, which tells the host to apply
  its non-interactive fallback; if the plugin does not reply before the timeout,
  the host treats the prompt as declined.
- **Version.** Send prompts only to plugins that negotiate the protocol version
  introducing them, so older plugins keep their existing non-interactive
  behavior.
- **`jp-serve-web`.** Bump its protocol version and decline all prompts so its
  current behavior and UI remain unchanged.
- **Host.** Run delegated turns with `interactive = true` and route prompts to
  the plugin through the structured `PromptBackend` and `InquiryBackend` seams
  rather than sending terminal key menus (`y/Y/n/N/p/r/e`).
  Audit all paths that branch on `interactive` during a delegated turn so every
  prompt reaches the plugin as one of the typed prompt kinds.
- **Bridge.**
  - While a turn awaits approval, treat the next message from the turn's sender
    as the approval response rather than an in-turn reply or a new turn.
  - Format the approval prompt as a brief chat message naming the tool and
    intended action.
  - Classify the sender's response via the decide API (`approve`, `deny`, or
    `approve` with modifications passed back as `modify` or an in-turn reply).
  - Deny the tool call if no response arrives before the timeout.
  - Always decline secret prompts so credentials are never requested in chat;
    decline other non-approval prompt kinds initially.

Noted in RFD 049 (a delegated turn gets someone to ask), and in D60 and D18.

- [ ] Protocol messages and version
- [ ] Host: interactive delegated turns that pass prompts to the plugin
- [ ] `jp-serve-web` declines every prompt
- [ ] Bridge: approval state, question text, decide, timeout
