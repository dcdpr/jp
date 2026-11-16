# Guard the planner checkout with a turn lock and clean pulls

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=feature
- **Parent**: T-1e92tft

The bridge never commits changes: files modified by a turn remain uncommitted in
the working tree until a maintainer pulls them into a review worktree.
`jp-serve-chat` guards the repository checkout around turns:

- **Lock.** Hold an exclusive `flock` on `git rev-parse --git-path
  jp-serve-chat.lock` during each turn and pull.
  `just chat-take` acquires the same lock so a turn cannot modify the tree
  between capturing the diff and resetting the checkout.
- **Pull.** Run `git pull --ff-only` before each turn and on an idle timer, but
  only when `git status --porcelain` (including untracked files) is empty.
  Log a warning if fast-forward pull fails rather than forcing it.
- **Scope.** After each turn, verify that every modified or untracked path is
  under `checkout.writable` (default `["docs/ticket", "docs/rfd",
  "docs/wiki"]`).
  If any path is outside those directories, pause new turns, notify the chat and
  the configured admin, and wait until the working tree is clean again.
- Messages arriving while the lock is held for a pull wait for the next turn.

A monitoring check on the server is the same `git status --porcelain` test,
outside this plugin.

- [x] Tests on a temporary repository: no pull with a dirty tree, an untracked
      file counts as dirty, a turn waits for a pull holding the lock, an
      out-of-scope change pauses turns
