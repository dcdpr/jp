`jp release cut` starts a release on a branch of its own, with its notes on a
second branch to merge into `main`; `jp release pick` adds commits from `main`
to it, and `jp release finalize` tags it.
Each prints the `git push` that publishes what it made, so a release can be
rehearsed before anything is pushed.
