# `jp c use ?a --grep` reports no matches for archived conversations

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-09-05

`jp conversation use` advertises `?a` as composable with `--grep`
(`use_.rs:38`), but the combination never matches anything:

```sh
jp c label add crate=jp_config    # on some conversation
jp c archive <id>
jp c use ?a --grep jp_config      # "no conversations match the filter"
```

The conversation exists, carries the text, and `jp c use ?a` alone offers it in
the picker.
Adding `--grep` makes it vanish with no indication that the filter could not
have matched.

`jp conversation grep` has the same root cause with a louder symptom: `jp c grep
--id +archived PATTERN` fails during handle resolution with a not-found error
naming an ID the user can see in `jp c ls --archived`.

## Cause

Archived conversations are deliberately absent from the live workspace index.
`archive_conversation` removes the entry (`jp_workspace/src/lib.rs:895`), and
`archived_conversations` reads a separate partition through the loader, which is
documented as "not cached in the workspace index" (`lib.rs:959-981`).

`acquire_conversation` is the only door into a conversation's metadata and
events, and it errors on anything missing from that index (`lib.rs:748-750`).
Both commands hit it:

- `search::id_matches` opens with `let Ok(handle) = ctx.workspace
  .acquire_conversation(&id) else { return false }`, so every archived ID is
  reported as "does not match" rather than "cannot be read".
  `Use::run_filtered` sources archived IDs from `source_ids`, hands them to
  `search::filter_ids`, gets an empty set back, and reports the generic no-match
  error.
- `resolve_request` maps every resolved ID through `acquire_conversation` and
  propagates the error (`cmd/target.rs:278-281`), so `c grep` fails before
  `Grep::run` is reached.

Nothing here is scope-specific: title, chat, reasoning, structured output, tool
calls, tool results, inquiry, and labels are all equally unreachable, because
the failure is upstream of scope selection.

## Scope

Predates the label scope.
`git log -G 'acquire_conversation(&id) else'` on `shared/search.rs` puts the
guard in cb036f69 ("Add `--grep` and `--from`/`--until` to `c use`", \#679), so
`--grep` has never worked against the archive partition.

## Possible directions

Two shapes, and the choice is a workspace-layer decision rather than a
search-layer one:

1. Teach the search path to read archived metadata and events.
   That means a read path that resolves against either partition — the archive
   is already loadable, it just has no handle type.
   This is what makes the advertised `?a --grep` composition true.
2. Reject archived targets explicitly.
   Cheaper, and turns a silent false negative into a real message, but leaves
   the help text at `use_.rs:38` promising something JP does not do, so the help
   would need amending too.

Whichever is chosen should cover both commands, since fixing one leaves the
other broken by the same mechanism.
