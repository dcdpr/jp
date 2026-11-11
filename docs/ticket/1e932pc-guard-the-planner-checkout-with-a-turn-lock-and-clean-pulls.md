# Guard the planner checkout with a turn lock and clean pulls

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

The server never commits: what the bot writes stays in the working tree until a
maintainer takes it.
`jp-serve-chat` keeps that checkout consistent.

- **Lock.** A lock file at `git rev-parse --git-path jp-serve-chat.lock`, held
  (`flock`) for each turn and for each pull.
  The take script takes the same lock, so a turn can't write between the copy
  and the reset that follows it.
- **Pull.** Before each turn and on an idle timer, `git pull --ff-only`, only
  when `git status --porcelain` is empty, untracked files included.
  A pull that isn't a fast-forward is reported, not forced.
- **Scope.** After each turn, every changed path has to be under a configured
  prefix (`writable = ["docs/ticket", "docs/rfd", "docs/wiki"]`).
  Otherwise the bridge stops starting turns, tells the chat that changes are
  paused, and messages the admin, until the tree is clean again.
- Messages arriving while the lock is held for a pull wait for the next turn.

A monitoring check on the server is the same `git status --porcelain` test,
outside this plugin.

- [x] Tests on a temporary repository: no pull with a dirty tree, an untracked
      file counts as dirty, a turn waits for a pull holding the lock, an
      out-of-scope change pauses turns
