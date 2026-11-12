# Chat bridge for planning the work over Signal

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=tracking

A chat bridge lets people who follow the project ask questions and steer planned
work from their phone without opening a terminal session.
`jp serve chat` is a long-running command plugin on an always-on server, which
holds a clone of this repository.
It relays messages from a chat transport (Signal first, through `signal-cli`) to
the host as delegated turns under a `planner` persona, and sends the assistant's
answer back.
Everything about tickets lives in the persona; the plugin is a generic chat
bridge and knows nothing about tickets.

Example requests:

- "What is being worked on?" reads the In Progress column.
- "Is anything planned for feature X?" searches tickets, RFDs, and the wiki.
- "Have this bug fixed in about two weeks" files a ticket and ranks it after the
  ticket the assistant expects to be done by then.

## Decisions

- **A plugin, not a core feature.** It uses existing plugin protocol messages:
  `query` (with `new`, `title`, `cfg`, `expires_in`), `interrupt` with `action =
  "reply"`, and `read_events`, as `jp-serve-web` does.
  Turn-level `--cfg` overrides reach the plugin-served ticket, RFD, and wiki
  tools (T-0vm44ks), so per-sender author attribution requires no host changes.
- **The tool list is the permission boundary.** A delegated turn runs
  non-interactively, so a tool at `run = "ask"` runs without prompting
  (`dispatch.rs`, `prepare_turn`; `coordinator.rs`, `decide_permission`).
  The persona explicitly enables read-only code and documentation tools plus the
  `ticket_*`, `rfd_*`, and `wiki_*` tools (which write under `docs/ticket`,
  `docs/rfd`, and `docs/wiki`).
  Tools that execute arbitrary commands or fetch URLs remain disabled.
- **The server never commits.** Changes written by a turn stay uncommitted in
  the working tree.
  A maintainer pulls the base commit and diff into a local worktree, edits and
  commits them as their own, and merges them via a pull request; raw bot output
  never reaches GitHub.
  The server pulls only when `git status --porcelain` is empty; a non-empty
  status indicates uncommitted changes waiting for review.
- **One turn at a time.** A message arriving during a turn is sent into it as an
  `interrupt` reply; one arriving between turns waits for the next `query`.
- **One conversation per chat per day**, so a persona change reaches it within a
  day (a conversation keeps the config it was created with) and context stays
  small.
  `conversation.start_local = true` keeps them out of the repository.
- **Transport-neutral.** Each person is configured with a per-transport
  identifier, and each chat (direct or group) maps to its own conversation and
  reply target.
  Adding another chat platform only requires a new transport implementation and
  config entry.
- **A dedicated Signal account**, not a device linked to a person's account.

## Known limits

- Changes a maintainer has taken into a review worktree remain invisible to the
  bridge until the pull request merges and the server pulls; the persona notes
  this possibility rather than filing duplicates.
- An in-turn reply that interrupts a tool call cancels it, though fast local
  ticket writes usually complete before cancellation; the persona re-reads the
  board before retrying.
- The board on `main` lags behind in-flight status changes until their pull
  requests merge.
- RFD draft slots and numbers are allocated in sequence, so a bot draft can
  clash with one on a maintainer's branch; `rfd_renumber` fixes it in review.

## Order

1. `ticket_move`.
2. The `planner` persona, which can be tried in a terminal before any bridge
   code exists.
3. The plugin's transport-neutral turn loop.
4. The checkout guard.
5. The Signal transport.
6. The take script.

Done when someone can, over Signal, ask what is in progress, search the planned
work, and file and rank a ticket, and a maintainer can take the result into a
worktree for review.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T12:42:00Z

The bridge, guard, Signal transport, and take recipe have landed on `jp-signal`
(1ad20a12a, 1d01fbc62).
What's left before it can be used: the `planner` persona (T-1e932pa), since
without one a turn has no tools, and a first end-to-end run on the server.
