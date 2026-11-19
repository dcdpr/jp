# RFD 137: Stable Storage for Link Files

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-09
- **Summary**: Stores link files for tool results and compaction summaries in a
  per-conversation directory with age-based pruning, replacing OS temp files.

## Summary

A **link file** is a plain copy of conversation content that JP writes so a
printed path or `file://` link has something to open: the full tool result
behind `results_file_link`, and the summary `jp conversation compact` reports.
This RFD moves link files from the OS temp directory to a per-conversation
directory in the User-Workspace Directory, names them by their content, and
prunes them by age.
What the files contain, and which links JP prints, stay the same.

## Motivation

Two code paths write link files, both into `env::temp_dir()`:

- `ToolRenderer::render_result` writes each tool result it renders to
  `tool_call_<millis>.<ext>`, and `results_file_link` decides whether a path or
  an OSC 8 link to that file is printed.
- `jp conversation compact` writes each generated summary to
  `<conversation_id>-summary-<from>-<to>.md` and prints the path in its
  timeline.

The tool result path has four defects:

1. **The location is not JP's.** The OS clears its temp directory on its own
   schedule, so a link in scrollback stops working at a time JP does not
   control.
2. **Names collide.** `<millis>` is the sub-second part of the clock, a number
   from 0 to 999.
   Names repeat every second, so a later result overwrites the file an earlier
   link points at.
3. **A failed write still prints a link.** The result of the write is discarded,
   so a link can be dead from the start.
4. **Files are written that nothing links to.** With `results_file_link = "off"`
   and inline results on, every rendered result still writes a file.
   `jp conversation print` uses the same renderer, so every replay writes every
   result again.

[RFD 115] (Accepted) raises the stakes.
It caps a tool response before the response is persisted, while the renderer
still receives the full output, and it relies on this file to keep the part the
assistant never saw.
Once it lands, the link file is the only copy of that output.
How long it lives should be JP's decision.

## Design

### What the user sees

The chrome is the same.
The path behind it is different:

```
see: /home/user/.local/share/jp/workspace/jp-a1b2c/tmp/conversations/jp-c17457886043/fs_read_file-9f2c4e7a1b3d5f60.rs
```

A link keeps working for the retention window, 30 days by default, counted from
the last time JP printed it.
`jp conversation print` prints links to the files a live run wrote, and writes a
file only for content that has none yet.

### Location

```
<user-workspace directory>/tmp/conversations/<conversation_id>/<label>-<hash>.<ext>
```

The User-Workspace Directory (W-00dxybd#user-workspace-directory) is the
workspace's per-user data directory, `~/.local/share/jp/workspace/<slug>-<id>/`
on Linux.
JP resolves it at startup, and this RFD only joins a path onto the result
(`Ctx::user_storage_path`).
It adds no method to the storage backend traits ([RFD 073]): link files are a
rendering convenience, not workspace data.

The directory has three properties this design relies on:

- It is outside the project, so link files stay out of version control and away
  from tools that work on project files.
- It is per workspace and already holds the conversations the files are copies
  of ([RFD 031]).
  Command plugins that receive the directory's path can read link files, as they
  can read those conversations.
- JP owns it, so JP can prune it.

`tmp/` is expendable.
Deleting it at any time loses link files and nothing else.
Storage operations give it no protection: when JP merges two user-workspace
directories of one workspace, it keeps one `tmp/` and may drop the other.

Link files sit under `tmp/conversations/`, one directory per conversation ID.
The `conversations/` level leaves `tmp/` free for other expendable files.
A conversation's ID never changes, so the path survives a title change and an
archive.
Link files are not placed in the conversation's storage directory (see
[Alternatives](#alternatives)).

### Names

| Part      | Value                                                                 |
| --------- | --------------------------------------------------------------------- |
| `<label>` | The tool's name for a tool result, `summary` for a compaction summary |
| `<hash>`  | The first 16 hex characters of the SHA-256 of the file's bytes        |
| `<ext>`   | The extension the writer detects today, or none                       |

The model names the tool it calls, so a tool name is reduced to ASCII letters,
digits, `_`, and `-` before it becomes part of a file name.
A result whose tool is unknown uses the label `tool`.

Naming a file by its content has three consequences:

- **The same content is one file.** A replay finds the file a live run wrote and
  links to it.
- **Different content is a different file.** Nothing is overwritten.
  Once [RFD 115] lands, a live run writes the full output and a replay writes
  the capped response under another name, and both exist.
- **No identifier has to reach the renderer** beyond the conversation ID.
  A fanned-out tool call writes one file per operation live and one for the
  folded response on replay, with no index to keep them apart.

### Writing a link file

- A link file is written only when a path or link to it is printed.
  A result rendered with `results_file_link = "off"` writes nothing.
- When the file exists, JP updates its modification time and writes nothing.
  Otherwise JP writes to a temporary sibling and renames it into place, so a
  reader never opens a partial file.
- When the write fails, JP logs a warning and prints no link.
  The result still renders inline.

Link URLs are percent-encoded.
The renderer builds `file://` and `copy://` URLs by formatting the path into a
string, which is correct only for paths without reserved characters.
The default user data directory on macOS is `~/Library/Application Support/jp`,
and `<slug>` is the checkout's directory name, so the path to a link file can
contain spaces and non-ASCII characters.

The path the `full` style prints is not escaped.
A backslash before a space makes the path paste into a shell, but a terminal
that detects paths in plain text then stops at the backslash.
A `file://` URL is detected whole, but it does not paste as a path.

### Fallback

JP writes the file to the OS temp directory, as
`<conversation_id>-<label>-<hash>.<ext>`, when:

- the run uses `--no-persist`, which promises to leave nothing in JP's storage;
- the workspace has no User-Workspace Directory.

JP never prunes the OS temp directory.

### Pruning

| Contract    | Value                                                                    |
| ----------- | ------------------------------------------------------------------------ |
| Config key  | `conversation.link_files.retention_days`                                 |
| Default     | `30`                                                                     |
| Never prune | `"unlimited"`                                                            |
| Rejected    | `0`, with a validation error that points at `"unlimited"`                |
| Run point   | The end of every run that loads a workspace, unless it is `--no-persist` |
| Scope       | Files under `tmp/conversations/`, then directories left empty            |
| On failure  | A warning in the trace log; the run's result is unaffected               |

The sweep removes a file whose modification time is older than the window.
Printing a link refreshes that time, so the window counts from the last time a
link to the file was printed, not from the first.

The sweep also removes the directory of a conversation that no longer exists,
whatever the age of its files.
It checks against a fresh scan of the stored conversation IDs, live and
archived, as the session mapping cleanup beside it does: another process may
have created a conversation after this run loaded its index.
`jp conversation rm` and the expiry of a temporary conversation both happen
before this cleanup, so a removed conversation's link files are gone when that
run ends.
A directory whose name is not a conversation ID is pruned by age only.

The sweep joins the cleanup `jp_cli::run` already performs at the end of a run,
which removes orphaned lock files and stale session mappings.
Every command that can write a link file passes through it.

`"unlimited"` and the rejected `0` follow `size_threshold` in [RFD 115].
A zero-day window would delete a file at the end of the run that linked to it.

[RFD 130] fixes its retention window for session records at a 30-day constant.
This window is configurable because link files can be large and, once [RFD 115]
lands, hold content that exists nowhere else: one user wants them gone sooner,
another wants them kept.

### Relation to `events.json` and the blob store

Three places hold a tool result, for three purposes:

| Place                      | Holds                                  | Purpose                        |
| -------------------------- | -------------------------------------- | ------------------------------ |
| `events.json`              | The persisted response, base64-encoded | The conversation's record      |
| Blob store ([RFD 066]) | The same payload, gzip-compressed      | Moves payloads out of the file |
| Link file                  | The rendered result as plain text      | Something a link can open      |

The blob store does not replace link files.
It names a blob by hash and compresses it so that text searches skip it, which
also means an editor cannot open it.
A link needs an uncompressed file with a usable extension, whether the bytes
come from `events.json` or from a blob.

The retention window bounds how long the full output of a capped response
exists.
Until the blob store or a later design keeps that output durably, it is gone
when its link file is pruned.

### Where the code lives

A `LinkFiles` type in `jp_cli` holds the directory for one conversation, or the
fallback, and offers one write operation that returns the path of the file or
nothing.
Both writers take it.
The sweep is a function beside it, called from `jp_cli::run`.

## Drawbacks

- **Plain-text copies live longer.** `events.json` stores tool content as
  base64; a link file is plain text.
  In the OS temp directory such a copy lasts until the OS clears it; here it
  lasts for the retention window, or until its conversation is removed.
- **Disk use moves to the user data directory.** It grows with the distinct
  results rendered within the window.
- **Paths are longer, and can contain a space.** The `full` link style prints
  the path, and the default path on macOS has a space in it.
  A terminal that ends a detected path at whitespace links only the part before
  the space, and a path pasted into a shell needs quoting.
  The `osc8` style is unaffected.
- **Every run pays for a sweep.** One directory walk over `tmp/conversations/`
  and one scan of conversation IDs at the end of each run.

## Alternatives

- **Keep the OS temp directory and fix the names.** Removes the collisions.
  Links still die on the OS's schedule, and after [RFD 115] so does the full
  output.
- **The platform cache directory.** Cache cleaners delete it, it is not scoped
  to a workspace, and it is a second JP-managed root to document.
- **The conversation's storage directory.** Its name carries the conversation's
  title and changes when the title does, and archiving moves it, so either
  operation breaks every link printed before it.
  A title generated for a new conversation is applied when its first query ends,
  so the links that query printed would break as it exits.
  Storage also merges and removes these directories and decides file by file
  what survives.
  Version control is not the concern: the files would sit in the user-local
  copy, and the workspace projection receives only the three managed files.
  If conversation directories later get a path that survives both a title change
  and an archive, link files can move there; [T-1n0d3w0] records that problem
  and the options for it.
- **A top-level `[storage]` config section** for the retention key.
  It would hold this one key; `conversation.link_files` sits beside the other
  conversation settings.
- **The project's `.jp/` directory.** Inside the checkout: the files show up in
  version control and in front of tools that read project files.
- **One directory per run.** A link file depends only on the content it copies,
  so each replay of a conversation would store identical copies again.
  If JP later keeps a record per run for diagnostics, that record can reference
  the link files a run printed.
  Revisit if such a record has to carry the full output of a capped response.
- **Names from the Event ID of the response.** Stable and meaningful, but the
  renderer needs the ID, a fanned-out call needs an operation index, and a
  replay must be stopped from overwriting a live run's full output with the
  capped response.
  Content hashes need none of the three.
- **A link that names the stream entry instead of a file.** A custom-scheme link
  resolved by the user's click handler needs no file, but it does nothing until
  the user installs a handler.
  `file://` opens in every terminal that supports OSC 8.
- **Link to the blob.** See [Relation to `events.json` and the blob
  store](#relation-to-eventsjson-and-the-blob-store).

## Non-Goals

- **A schema for link files.** A file holds what the writer renders today.
  `events.json` is the machine interface.
- **Durable storage for conversation content.** That is the blob store's ([RFD
  066]).
- **Files the assistant can read.** [RFD 115] defers that to the blob store.
- **Changes to which links are printed,** to their text, or to
  `results_file_link` and its error overlay.
- **Other files JP writes to the OS temp directory.** The trace log and the
  request and configuration payloads the trace refers to are diagnostics, with
  their own lifetime questions.
- **A size limit.** Pruning is by age only.

## Risks and Open Questions

- **One sweep, one configuration.** The sweep prunes every conversation's files
  with the window of the run that performs it.
  A conversation that sets a short window shortens it for the whole workspace
  whenever that conversation runs.
- **A conversation only another checkout holds.** A conversation that exists
  only in one checkout's workspace projection is unknown to a run from another
  checkout of the same workspace.
  That run's sweep treats the conversation as removed and deletes its link
  files.
  The links printed for it stop working early, and the next print writes the
  files again.
- **Sweep cost.** The walk touches every link file on every run.
  If it shows up in measurements, a marker file can limit the sweep to once a
  day.
- **Modification times.** A filesystem that does not record them makes the sweep
  prune too early or never.
  The loss is link files.
- **A sweep racing a link.** One process can sweep a file that is past the
  window while another is printing a link to it: the sweep reads the old
  modification time, the other process refreshes it, the sweep deletes the file.
  The result is one dead link, and the next render writes the file again.
- **Truncated hashes.** Sixteen hex characters are 64 bits within one
  conversation's directory.
  A collision would link one result to another's file; at the number of files a
  conversation produces, it is not expected to happen.

## Implementation Plan

1. **`LinkFiles`.** Add the type to `jp_cli`: directory resolution with the OS
   temp fallback, content-hashed names, write-if-absent through a rename, and
   the modification-time update.
   Tested against a temporary root with fixed content and fixed names.
   Mergeable alone while nothing uses it.
2. **Move the writers.** `ToolRenderer::render_result` and `jp conversation
   compact` write through `LinkFiles`.
   `render_result` takes the tool's name for the label, writes only when it
   prints a link, prints no link when the write fails, and percent-encodes link
   URLs.
   `TurnRenderer` keeps the `LinkFiles` across its per-turn rebuild of
   `ToolRenderer`.
   Depends on phase 1.
3. **Config and pruning.** Add `conversation.link_files.retention_days` with its
   validator, and the sweep to the end-of-run cleanup, including the rule for
   removed conversations.
   Depends on phase 1.
   Until it lands, link files accumulate.
4. **Docs.** Add a Link File entry to the glossary, mention `tmp/` in the
   User-Workspace Directory entry (W-00dxybd), and document the key.

## References

- [RFD 031]: user-local storage, where the User-Workspace Directory comes from.
- [RFD 066]: the content-addressable blob store, the durable home for payloads.
- [RFD 073]: the storage backend traits, which gain nothing here.
- [RFD 115]: bounded tool output, which leaves the full output in the link file.
- [RFD 130]: session record retention, the fixed-window precedent.
- `crates/jp_cli/src/render/tool.rs`: `render_result`, the tool result writer.
- `crates/jp_cli/src/cmd/conversation/compact.rs`: `write_summary_file`.
- `crates/jp_cli/src/lib.rs`: the end-of-run cleanup the sweep joins.
- `crates/jp_cli/src/ctx.rs`: `Ctx::user_storage_path`.
- `crates/jp_config/src/fs.rs`: how the user data directory is resolved.

[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 115]: 115-bounded-tool-output.md
[RFD 130]: 130-session-record-retention-and-read-surface.md
[T-1n0d3w0]: ../ticket/1n0d3w0-give-user-local-conversation-directories-a-stable-path.md
