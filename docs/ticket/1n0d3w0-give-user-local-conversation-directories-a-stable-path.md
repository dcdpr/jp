# Give user-local conversation directories a stable path

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: domain=conversation
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=pre-rfd

The path of a conversation's user-local directory changes during the
conversation's life, and everything that kept the path breaks when it does.
This ticket records the problem and the options discussed so far.
The answer is a design argument, so it should become an RFD before
implementation.

## Problem

A conversation directory is named `<timestamp>-<title-slug>`, where
`<timestamp>` is the decisecond part of the conversation ID.
Two operations move it:

- A title change renames it (`reconcile_conversation_dir` in
  `crates/jp_storage/src/lib.rs`).
  `conversation.title.generate.auto` defaults to `true`, so a new conversation
  is renamed when its first query ends.
- Archiving moves it under `conversations/.archive/`, and unarchiving moves it
  back.

JP finds the directory by its ID prefix, so JP's own lookups are unaffected.
What breaks is anything that kept the path:

- a path or `file://` link printed to the terminal;
- an editor or shell opened on the directory (`jp conversation edit`, `jp
  conversation path`);
- a file another feature keeps beside the three managed files.

The last group is growing.
The directory holds the query draft (`QUERY_MESSAGE.md`) today, RFD 114 adds
`metadata.local.json`, and draft D56 (Conversation-Local External File
Attachments) adds attachment snapshots.
Four tickets are about such files being lost when the directory is renamed,
merged, or replaced: T-0sqf1cs (done), T-0sr3x0v, T-0sr3zez, and T-0sr3zey.

RFD 137 (Stable Storage for Link Files) works around the problem.
It keeps link files in `tmp/conversations/<conversation_id>/` instead of the
conversation directory, because a link printed during a conversation's first
query would break when that query ends.

## Constraint

The title is in the directory name on purpose.
JP's storage is meant to be usable with `ls`, `cat`, and `grep`, and the title
shows at a glance which conversation lives where.
A solution keeps that, or says what it gives up.

## Options

1. **Name the user-local copy by ID alone.** The user-local directory never
   changes its name; the workspace projection in `.jp/conversations/` keeps
   `<timestamp>-<title-slug>`.
   No bookkeeping, and a title change no longer renames anything in user-local
   storage.
   Costs: a listing of the user-local store shows no titles, and archiving still
   moves the directory.
   The leading option so far, accepted with reservations about the lost titles.
2. **An index file, `conversations/.metadata.json`,** that maps every
   conversation ID to its current path, titled or not, archived or not.
   Keeps the titles and needs no symlink.
   Costs: every rename, archive, unarchive, and removal updates one shared file,
   and concurrent `jp` processes update it by read-modify-write.
   RFD 087 chose a directory of per-root files for its roots registry to avoid
   that.
3. **A link file per conversation, `conversations/<id>.link`,** that holds the
   absolute path of the conversation's directory.
   Keeps the titles and has no shared file.
   Costs: one more file per conversation to keep in step with each move, and an
   absolute path goes stale when user-workspace directories are merged or the
   user data directory moves.
4. **A symlink per conversation** from a stable name to the current directory.
   Keeps the titles, and a path that was already printed keeps working through
   both a rename and an archive.
   Costs: every operation that moves the directory updates the link, and
   creating a symlink needs a privilege on Windows, which is why RFD 087 dropped
   the `storage` symlink.

## Open questions

- What has to be stable: the path, or the lookup?
  Options 2 and 3 give a stable way to find the directory.
  JP already finds a live conversation's directory by ID prefix, and `jp
  conversation path` prints it.
  A path that was printed or opened earlier still breaks under them.
  Options 1 and 4 keep the path itself valid.
- Does the path have to survive archiving?
  The archive partition exists so that archived conversations are skipped at
  startup.
  Option 1 does not cover it; option 4 does.
- Can the title be shown some other way than the directory name, so one name
  does not have to serve both a person reading a listing and a link?
- Does the workspace projection need the same guarantee, or only the user-local
  copy?
- How are existing directories migrated, and what does an older `jp` do with the
  new layout?

## Related

- T-0sr3x0v, T-0sr3zez, T-0sr3zey: open bugs where files beside the managed ones
  are lost in a merge, a reconcile, or an archive.
- RFD 031: user-local storage as the source of truth, and the workspace
  projection.
- RFD 071: conversation archiving and the `.archive/` partition.
- RFD 087: the roots registry that replaced the `storage` symlink.
- RFD 114 and draft D56: more files in the conversation directory.
- RFD 137 (Stable Storage for Link Files): link files could move into the
  conversation directory once its path is stable.

## Next step

Promote to an RFD.
Nothing waits on it: RFD 137 does not depend on the outcome.
