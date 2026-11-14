# Let the git tools read files at a past commit

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-14
- **Label**: domain=tooling
- **Label**: relevant-rfd=123
- **Label**: type=feature

The local git tools in `.config/jp/tools` show what a commit changed
(`git_show`, `git_diff_commit`) but can't read the code as it was at that
commit.
An assistant that has to judge a past change from its context, such as the
release backfill drafting notes or user docs for an old release (RFD 123, "JP's
history"), sees only the diff.

The backfill reads past code through a worktree of the period's last commit,
mounted into the drafting conversation with `jp query --mount`.
These tools are the fallback if that approach doesn't work out in practice:

- [ ] `git_read_file`: a file's contents at a revision (`git show
      <rev>:<path>`), with the same line-range options as `fs_read_file`.
- [ ] `git_grep_files`: search the tree of a revision (`git grep <pattern> <rev>
      -- <paths>`), with the same output shape as `fs_grep_files`.
- [ ] `git_list_files`: list the files of a revision (`git ls-tree -r
      --name-only <rev> -- <prefix>`), with the same filters as `fs_list_files`.

Each tool takes the revision as a required parameter, so every call names the
commit it reads and never mixes past and present code.
They respect the workspace's suppress patterns, like the `fs_*` tools.
