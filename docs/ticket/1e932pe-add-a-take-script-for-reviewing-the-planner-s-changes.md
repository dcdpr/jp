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

Under `jp-serve-chat.lock` over SSH, capture the server's `HEAD` commit and `git
add -A && git diff --cached --binary -M HEAD`.
Locally, create a worktree at that base `HEAD` commit and apply the patch with
`git apply --index` rather than applying directly onto `origin/main`: changes
from an earlier take that are still in review may not be in the server's base
commit yet, so rebasing the worktree onto `origin/main` afterwards resolves any
`.board.json` overlap via a three-way merge.
Using a binary git diff also preserves file renames (such as retitled tickets)
and deletions.

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
