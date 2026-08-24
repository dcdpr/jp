# Decide the ordering policy for `extends` graphs with conflicting precedence

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-24
- **Implements**: 035

\#887 made the `extends` graph resolve fully before any file is loaded, then
reduced it to one entry per file keeping each file's **last** position
(`dedup_keep_last` in `jp_config::util`).
Keep-last was chosen because it preserves `main`'s observable resolution: under
last-wins merging the winner of a field is whichever setter sits at the highest
sequence position, and keep-last preserves the relative order of those maxima,
so replace-merged fields resolve identically with and without the dedup pass.

That leaves an ordering question the PR deliberately did not answer.

## The case

For `a -> [b, c]`, `b -> d`, `c -> d`, DFS produces `[d, b, d, c, a]`, which
collapses to `[b, d, c, a]`.
So `d` merges *after* `b`, and `d` can override the file that extended it.

This is pre-existing `extends` semantics rather than something the dedup
introduced — `main` loads `[d, b, d, c, a]` and the second `d` already clobbers
`b`.
`test_load_partial_at_path_diamond_keeps_shared_file_last` pins it as a
characterization test.

The alternative is a topological order that respects each `before` edge (a file
extended by `b` merges before `b`).
That would change resolution for existing workspaces.

## The conflict that needs a policy

A topological order has no obvious answer when the declared array order and the
`before` edge disagree:

```toml
# a.toml
extends = ["b.toml", "d.toml"]   # declared order says d overrides b
```

```toml
# b.toml
extends = ["d.toml"]             # before-edge says d merges before b
```

`test_load_partial_at_path_repeat_visit_keeps_last_position` pins the current
answer (declared order wins, `d` overrides `b`).
A topological rewrite needs an explicit rule for this, not just a different
traversal.

## Scope

Design work with a user-visible outcome.
It belongs with the rest of the `extends` semantics rather than in a dedup fix
— see draft D35 (Extends Overrides and Loader Namespace).

## Context

Raised in review on \#887 (comment 3657547063).
The reviewer accepted the compatibility argument for keep-last and agreed the
`before`-edge semantics belong with the broader `extends` design work.
