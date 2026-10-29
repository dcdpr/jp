# Let a command plugin read what was piped into `jp`

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-29
- **Label**: domain=plugins
- **Label**: package=jp_cli
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=116
- **Label**: type=enhancement

A command plugin's stdin is the protocol channel to the host, so it can never
see what was piped into `jp`.
JP's justfile keeps `ticket-add` and `ticket-comment` only to read piped text
and pass it to `jp ticket` as `--body`.

## Proposal

- **`read_stdin` host request** (protocol 14).
  The plugin asks, and `jp` answers with the piped text, or with nothing when
  stdin is a terminal.
  - Read on request, so a command that doesn't ask never waits on stdin.
  - Stdin is read once; a second request gets nothing.
- **`jp ticket add` and `comment`** ask for it when no `--body` is given, and
  use the piped text as the body, as `jp query` does.
  Like `jp query`, a pipe that never closes makes the command wait.
- **`jp ticket comment T-02wt0kx#1`** replies to comment 1, as the
  `ticket-comment` recipe accepts today.
- **Drop `just ticket-add` and `just ticket-comment`.** `just ticket-add bug
  "Title"` becomes `jp ticket add "Title" --label type=bug`.
- A `jp` older than protocol 14 isn't asked, so piped text is ignored there.
  The plugins are installed with `jp`, so that's acceptable.

Piping doesn't take away the prompts: `jp` decides it can prompt from stdout
(`crates/jp_cli/src/ctx.rs`), and the prompt widgets read keys through
crossterm, which opens `/dev/tty` when stdin isn't a terminal.
So `pbpaste | jp ticket add` should still ask for the title.
Confirm that by running it as part of this work.
