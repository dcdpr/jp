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

A turn the bridge starts runs non-interactively, so every tool the persona
enables runs without asking: `run = "ask"` is approved automatically for a
delegated turn (`dispatch.rs`, `prepare_turn`; `coordinator.rs`,
`decide_permission`).
The persona's tool list is the only control, and a write the person didn't mean,
such as a duplicate ticket from a correction that arrived mid-call, lands before
anyone can stop it.

What the chat should look like:

```text
JP: I'm going to file "Search crashes on an empty query" (type=bug) and rank
    it after T-0abc123. Go ahead?
Ada: yes, but make it a feature
```

The answer is free text, so the decide API (`decide.url`) turns it into a
decision: approve, deny, or approve with an edit the turn reads as a reply.

What this needs:

- **Host side.** A delegated turn routes a permission prompt to the plugin that
  started it, instead of approving it, when the plugin says it can answer.
  Nothing does this today; the plugin protocol only goes the other way (a plugin
  asking the host to `compose`).
  It is a protocol change and touches RFD 049's detached prompt policy, so it
  may need its own RFD.
- **Bridge side.** Render the call (tool, arguments) as a short question, send
  it to the chat the turn belongs to, wait for the next message from the person
  the turn runs as, decide it, and answer the host.
  A timeout denies.
- **Persona.** Which tools ask: the writing tools set to `run = "ask"`, the
  reading tools left at `allow`.

This replaces delaying writes (T-1hetcnv) as the fix for corrections that arrive
mid-call: an approval waits as long as the person needs.
