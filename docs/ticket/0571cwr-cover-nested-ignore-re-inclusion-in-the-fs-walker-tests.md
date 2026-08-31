# Cover nested .ignore re-inclusion in the fs walker tests

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-19

The fs tools support a two-level ignore arrangement that nothing tests: a root
`.ignore` excludes a subtree, and a `.ignore` *inside* that subtree re-includes
it with `!**`.

The result is that an unscoped walk from the workspace root does not descend
into the subtree, while a walk scoped to that subtree (or deeper) sees it in
full.
That asymmetry is useful — it keeps a large, irrelevant directory out of broad
searches without making it unreachable — but it is entirely emergent.

## Why it works

`WalkBuilder` in `.config/jp/tools/src/fs/list_files.rs` is built with
`standard_filters(false)`, `.ignore(true)`, `.parents(true)`.
Only `.ignore` files are consulted; `.gitignore` and `.git/info/exclude` are
not.
On a broad walk the root `.ignore` prunes before descent, so the nested file is
never read.
On a scoped walk the nested file is picked up through `parents(true)` and its
negation applies.

## Why it needs a test

PR \#727 rewrote this code path (`walk_spec` now "scopes the workspace walk with
a path filter rather than re-rooting") and the behaviour survived, but nothing
would have caught it if it hadn't.
The failure mode is silent: searches quietly stop seeing a directory, and the
tool reports success with fewer results.

## Suggested test

In `.config/jp/tools/src/fs/list_files_tests.rs`: build a temp tree with a root
`.ignore` excluding `sub/`, a `sub/.ignore` containing `!**`, and a file at
`sub/file.rs`.
Assert an unscoped listing omits it and a listing with prefix `sub` includes it.

Worth a matching case in `grep_files_tests.rs`, which delegates to the same
walker for directory targets.
