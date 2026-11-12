# Add a take script for reviewing the planner's changes

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=tooling
- **Label**: type=task
- **Parent**: T-1e92tft

Copy uncommitted changes from the chat bridge's server checkout into a local
review worktree on a maintainer's machine, then reset the server checkout so it
can resume pulling.

`just chat-take HOST DIR` does this.
It sends `crates/plugins/command/serve-chat/take.sh` to the server over SSH and
runs it there twice:

1. `snapshot`: under the bridge's lock (`flock` on `$(git rev-parse --git-path
   jp-serve-chat.lock)`), print the checkout's `HEAD` and a binary diff of every
   change (`git add -A && git diff --cached --binary -M HEAD`), which keeps
   renames (retitled tickets) and deletions.
2. Locally, create a worktree at that base commit and `git apply --index` the
   diff there, rather than onto `origin/main`: changes from an earlier take that
   are still in review may not be on the server's base yet, so rebasing onto
   `origin/main` afterwards resolves any `.board.json` overlap with a three-way
   merge.
3. `reset`: under the same lock, check that `HEAD` and the diff's hash are
   unchanged since the snapshot, then `git reset --hard` and `git clean -fd`.
   If either changed, a turn wrote in the meantime, and the checkout is left
   alone to be taken again.

The script runs under `set -eu` with `pipefail` where the shell has it, so a
failing `git add` (a full disk, say) stops it before anything is discarded.
`crates/plugins/command/serve-chat/tests/take.rs` runs it against real
repositories, including that failure.

- [x] Recipe and server-side script
- [ ] First real take from the server

## Comments

-----

- **From**: jp
- **Date**: 2026-11-07T12:41:59Z

Written as `just chat-take HOST DIR [WORKTREE]` (commit 1d01fbc62).
It takes the diff in one ssh call under the lock, applies it in a worktree at
the bridge's base commit, and only then resets the server, in a second locked
call that checks HEAD and the diff's sha256 are unchanged.
Not yet run against a real server: close this once a take has gone through end
to end.
