# A malformed `.jp/.id` silently reassigns the workspace ID

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-19

`Workspace::open_with_storage_dir` reads the workspace ID with:

```rust
let id = Id::load(&storage)
    .transpose()
    .ok()
    .flatten()
    .unwrap_or_default();
```

`Id::load` returns `Option<Result<Id>>` (`id.rs:41`): `None` when the file
cannot be read, and `Some(Err(_))` when its last line fails `from_str`, which
rejects anything that is not five characters of `[0-9a-z]` (`id.rs:96-106`).
`.ok()` discards that `Err` arm, and `.unwrap_or_default()` then mints a fresh
random ID.

The consequence is not local.
`with_user_storage(&user_root, slug, id.to_string())` keys the user-local silo
by that ID, so a fresh ID selects a different silo, and `id().store(&storage)`
immediately afterwards overwrites `.jp/.id` with the replacement.
Every `--local` conversation in the original silo disappears from listings, and
the ID that would find them again is gone from disk.

## Why it is reachable

`.jp/.id` is a tracked file in this repository, so a conflicted merge in it is
ordinary.
Conflict markers fail both the length and the alphabet check.
A truncated write does the same.

Nothing is reported.
The run succeeds and simply lists fewer conversations.

## Fix

Treat only a genuinely missing ID as the new-workspace case, and propagate a
malformed one before any user storage is touched:

```rust
let id = match Id::load(&storage) {
    None => Id::new(),
    Some(result) => result?,
};
```

`Error::Id` already exists (`id.rs:97`), so `open` needs no new variant and
`jp_cli`'s `From<jp_workspace::Error>` already covers it.

## Test

A workspace whose `.jp/.id` holds invalid content: `open` returns `Error::Id`,
and the file is left exactly as it was.

## Scope

Not introduced by the move into `jp_workspace` — the same chain lived in
`jp_cli::load_workspace` beforehand and was relocated verbatim, which is why it
was left alone in that PR.
`open`'s doc comment there was widened to say an ID file that is "missing,
unreadable, or malformed" gets a fresh ID, so the behaviour is at least written
down until this lands.
That sentence should go back to describing only the missing case once it does.
