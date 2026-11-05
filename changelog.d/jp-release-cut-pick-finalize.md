---
type: added
---
`jp release cut` starts the next release on a branch of its own,
`release/<major>.<minor>`, and opens a pull request that moves the unreleased
fragments into the release's directory on `main`, where its notes are prepared.
`jp release pick <version> <commit>...` adds commits from `main` to a release in
preparation, or starts the next patch release with them; a conflict stops it in
a worktree it names, and `jp release pick --continue` finishes it.
`jp release finalize <version>` tags the release.
All three work from what `origin` holds, in worktrees of their own, so your
checkout is left alone.
`plugins.command.release.options.bump` names the command that writes a new
version, and `change_request_labels` the labels their change requests carry.
