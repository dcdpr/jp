# Shareable RFD, Ticket, and Site Tooling

- **Status**: Done
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-10-14
- **Label**: tracking-rfd=116
- **Label**: type=tracking

Tracking ticket for [RFD 116].

## Implementation plan

- **Add ticket config, commands, and plugin tools**: `jp ticket` reads `dir`,
  `assistant`, and `board.done_limit` from its options.
  It gains `rename`, `status`, `block`, `board`, `rank`, and `init`, drops
  `migrate`, and requires `--repo` on `import`.
  The `ticket_*` tools are served as plugin tools with `tools init` and `tools
  sync`, and `ticket.rs` and `just ticket-labels-sync` are removed.
  The label vocabulary stays in `.labels.json` for now.
  Mergeable on its own.
- **Create jp-rfd with read-only commands**: Creates the `jp-rfd` plugin,
  reading `dir` and `categories` from its options.
  Ports the reading side of the board from `rfd-shared.mjs` and adds `list`,
  `show`, `tools init`, `tools sync`, and the tools for `list` and `show`.
  Depends on Phase 1.
- **Port the RFD lifecycle commands**: Writes the shared id resolver,
  metadata-header editor, and relationship writer.
  Ports `draft` (including `--from-ticket`), `rename`, `renumber`, `extend`,
  `require`, `supersede`, `abandon`, and `promote` with their tools, and adds
  `rank` and `init`.
  JP's recipes become one-line calls into `jp rfd`, and the `rfd_extend` tool
  parameters follow the new argument order.
  Depends on Phase 2.
- **Port RFD tracking commands**: Ports `track`, `start`, and `stop` with their
  tools, where `track` reads the Implementation Plan through the host's `query`.
  The dev server's `/__rfd-development` endpoint is pointed at `jp rfd`.
  Depends on Phase 3.
- **Create jp-vite and sync the site from the binary**: Creates `jp-vite` with
  the site files embedded from `crates/plugins/command/vite/site/`, plus `init`,
  `sync`, `sync --check`, the settings snapshot, and `serve` with its
  version-skew checks.
  The ticket board and priority writers go through `jp ticket` and the
  snapshot's RFD directory.
  The label vocabulary moves into `plugins.command.ticket.options.labels`, and
  CI runs `sync --check`.
  Depends on Phase 2 and is independent of Phases 3 and 4.
- **Retire the ported justfile recipes**: Removes the ported `rfd-*` recipes and
  `ticket-promote` from the justfile and points JP's RFD tool configuration at
  `plugin.command.rfd`.
  Updates the references in RFD 001 and RFD 100 to the new commands and the
  vocabulary location.
  Depends on Phases 1 to 5.

[RFD 116]: ../rfd/116-shareable-rfd-ticket-and-site-tooling.md
