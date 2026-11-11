# Add a take script for reviewing the planner's changes

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=tooling
- **Label**: type=task
- **Parent**: T-1e92tft

Moves what the bot wrote on the server into a fresh worktree on a maintainer's
machine, and leaves the server clean, so it can pull again.

One `ssh` call does the server half under the bridge's lock: print `HEAD`, then
`git add -A && git diff --cached --binary -M HEAD`, then `git reset --hard` and
`git clean -fd`.
Locally, the worktree starts at that `HEAD` and the diff is applied there with
`git apply --index`, not on `origin/main`: work taken earlier and still in
review isn't in the server's base, so applying the diff on `main` would undo it
in `.board.json`.
Rebasing onto `origin/main` afterwards puts any real overlap through a three-way
merge.
A diff also carries renames (a retitled ticket) and deletions, which copying
changed files misses.

```sh
ssh "$host" "cd $dir && flock \"\$(git rev-parse --git-path jp-serve-chat.lock)\" sh -c '
  git rev-parse HEAD
  git add -A && git diff --cached --binary -M HEAD
  git reset -q --hard && git clean -qfd'" > take.patch
git fetch origin
git worktree add -b planner-take ../jp-take "$(head -1 take.patch)"
tail -n +2 take.patch | git -C ../jp-take apply --index
```

Untested as written.
A `just` recipe taking the host and the remote directory is one place for it.
Nothing is taken when the server's tree is clean.

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
