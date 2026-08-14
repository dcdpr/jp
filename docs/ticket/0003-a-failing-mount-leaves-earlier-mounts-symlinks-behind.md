# T0003: A failing `--mount` leaves earlier mounts' symlinks behind

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-14

`create_mount_effects` (`crates/jp_cli/src/cmd/query.rs:2203`) walks the
`--mount` specs in a loop, creating each symlink as it goes.
A spec that fails part-way leaves the symlinks for every preceding mount on
disk, and the command reports failure.

## Reproduction

```sh
jp q --mount ok=/some/real/path --mount typo=/does/not/exist "hello"
```

The first mount's symlink is created at `<workspace>/ok`.
The second fails `canonicalize_utf8` and the whole command errors.
`<workspace>/ok` stays.

Any per-spec failure does it: an unparseable `MountSpec`, a name that escapes
the workspace, an unresolvable target, a target *inside* the workspace, or a
symlink path already occupied by a non-symlink.

## The state it leaves

Worse than "partially mounted", because the two side effects have different
granularity:

- Symlinks are created eagerly, one per loop iteration.
- The approval store is written **once**, after the loop (`store.save`).

So a mid-loop failure leaves symlinks on disk with *no* approval entries for any
of them.
The links exist but no tool can use them.

## Severity

Contained and visible, which is why it was not fixed in PR \#982 (raised in
review there, item 4).

- The command errors with a message naming the bad target.
- The stray symlink is inside the workspace tree, so `git status` shows it.
- Re-running with the typo fixed heals the state: `create_workspace_symlink`
  no-ops on an existing link with an identical target (`query.rs:2285-2300`),
  then `store.save` runs and seeds every approval.

The residue is a stray symlink in a tree the user may commit.
Not silent, not spreading.

## Suggested resolution

The code already has the right instinct one level down.
Within a single mount it resolves the target *before* creating the link, with
the comment: "Resolve the target before creating the link so a missing target
fails cleanly instead of leaving a broken symlink behind"
(`query.rs:2234-2238`).

Apply the same principle across the batch: parse every spec, resolve every name,
and canonicalize every target up front; only then create any symlink.
That turns the common failures (typo, escape, in-workspace target) into a clean
no-side-effect error and needs no unwind logic.

It does not cover a symlink creation that fails on the Nth link for a filesystem
reason (permissions, a name occupied by a real file).
Covering that too means tracking created links and removing them on failure —
worth deciding deliberately rather than by default, since an unwind that itself
fails is its own problem.

## Owner

`--mount` is specified by RFD D43, "Tool Access to External Paths via Workspace
Symlinks" (Draft), Phase 5.
Its "Effects of `--mount`" section enumerates the steps for a *single* mount and
does not say what happens when one of several fails.
Whichever resolution is chosen should be written back there.
