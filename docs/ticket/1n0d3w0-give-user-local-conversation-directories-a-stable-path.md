# Give user-local conversation directories a stable path

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: domain=conversation
- **Label**: domain=storage
- **Label**: package=jp_storage
- **Label**: type=pre-rfd

The path of a conversation's user-local directory changes over the
conversation's lifetime, breaking any external reference that recorded the
earlier path.
This ticket records the problem and the options evaluated so far; it should be
promoted to an RFD before implementation.

## Problem

A conversation directory is named `<timestamp>-<title-slug>`, where
`<timestamp>` is the decisecond timestamp of the conversation ID.
Two operations move the directory:

- Changing the title renames the directory (`reconcile_conversation_dir` in
  `crates/jp_storage/src/lib.rs`).
  Because `conversation.title.generate.auto` defaults to `true`, a new
  conversation is renamed as soon as its first query finishes.
- Archiving moves the directory under `conversations/.archive/`, and unarchiving
  moves it back.

JP resolves conversation directories by ID prefix, so internal lookups are
unaffected.
External references to the path break:

- paths or `file://` hyperlinks printed to the terminal;
- editors or shells opened in the directory (`jp conversation edit`, `jp
  conversation path`);
- auxiliary files stored alongside the three managed JSON files.

The third category is growing.
The directory holds the query draft (`QUERY_MESSAGE.md`), RFD 114 adds
`metadata.local.json`, and draft D56 (Conversation-Local External File
Attachments) adds attachment snapshots.
Four tickets track auxiliary files lost when a conversation directory is
renamed, merged, or replaced: T-0sqf1cs (done), T-0sr3x0v, T-0sr3zez, and
T-0sr3zey.

RFD 137 (Stable Storage for Link Files) works around this instability by storing
link files in `tmp/conversations/<conversation_id>/` instead of the conversation
directory; otherwise, a link printed during a conversation's first query would
break when the query finishes and the auto-generated title renames the
directory.

## Constraint

Including the title in the directory name is intentional: JP's storage is
designed to be navigable with standard shell tools (`ls`, `cat`, `grep`), and
the title slug makes conversations recognizable in directory listings.
Any change must either preserve human-readable listings or explicitly justify
giving them up.

## Options

1. **Name the user-local directory by conversation ID alone.** The user-local
   directory name never changes, while the workspace projection in
   `.jp/conversations/` keeps `<timestamp>-<title-slug>`.
   Requires no extra index or symlink bookkeeping, and title changes no longer
   rename user-local directories.
   Tradeoffs: listing the user-local store no longer shows conversation titles,
   and archiving still moves the directory under `.archive/`.
   This is the leading option so far, pending the loss of titles in user-local
   directory listings.
2. **Maintain an index file (`conversations/.metadata.json`)** mapping each
   conversation ID to its current directory path (titled or untitled, live or
   archived).
   Preserves titles in directory names without symlinks.
   Tradeoffs: every rename, archive, unarchive, and removal must update a single
   shared file, requiring read-modify-write synchronization across concurrent
   `jp` processes (the reason RFD 087 chose a directory of per-root files for
   its roots registry).
3. **Write a pointer file per conversation (`conversations/<id>.link`)**
   containing the absolute path to the conversation directory.
   Preserves titles without a shared index file.
   Tradeoffs: adds an extra file per conversation that must stay synchronized on
   every move, and absolute paths break when user-workspace directories are
   merged or the user data directory is relocated.
4. **Maintain a per-conversation symlink** from a stable ID path to the current
   titled directory.
   Preserves titles and keeps previously printed paths valid across both title
   renames and archiving.
   Tradeoffs: every move must update the symlink, and creating symlinks on
   Windows requires elevated privileges or Developer Mode (which led RFD 087 to
   remove the `storage` symlink).

## Open questions

- **Path stability vs. lookup stability.** Options 2 and 3 provide a stable
  lookup path, which JP already has via ID-prefix scanning and `jp conversation
  path`, but any path already printed to a terminal or opened in an editor still
  breaks on rename.
  Only Options 1 and 4 keep a previously resolved path valid.
- **Surviving archiving.** The `.archive/` subdirectory exists so startup scans
  can skip archived conversations.
  Option 1 keeps the path stable across title changes but not archiving; Option
  4 survives both.
- **Alternative ways to surface titles.** Can human-readable titles be surfaced
  without encoding them in the user-local directory name?
- **Workspace projection scope.** Does the workspace projection need a stable
  path, or only the user-local directory?
- **Migration and backward compatibility.** How are existing directories
  migrated, and how do older `jp` binaries handle the new layout?

## Related

- T-0sr3x0v, T-0sr3zez, T-0sr3zey: Open bugs where auxiliary files in the
  conversation directory are lost during merge, reconciliation, or archiving.
- RFD 031: User-local storage as the source of truth and workspace projections.
- RFD 071: Conversation archiving and the `.archive/` partition.
- RFD 087: Roots registry that replaced the `storage` symlink.
- RFD 114 and draft D56: Additional files stored in the conversation directory.
- RFD 137 (Stable Storage for Link Files): Link files could move into the
  conversation directory once its path is stable.

## Next step

Promote to an RFD.
RFD 137 does not depend on this ticket.
