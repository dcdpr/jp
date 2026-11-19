# RFD 137: Stable Storage for Link Files

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-09
- **Summary**: Stores link files for tool results and compaction summaries in a
  per-conversation directory with age-based pruning, replacing OS temp files.

## Summary

A **link file** is a disposable plain-text copy of conversation content that JP
writes to disk so a printed path or `file://` hyperlink has a target to open:
the full tool result behind `results_file_link`, and the summary reported by `jp
conversation compact`.
This RFD moves link files from the OS temp directory to a per-conversation
directory inside the User-Workspace Directory, names them by content hash, and
prunes them by age.
File contents and printed links are unchanged.

## Motivation

Two code paths write link files into `env::temp_dir()`:

- `ToolRenderer::render_result` writes every rendered tool result to
  `tool_call_<millis>.<ext>`, and `results_file_link` controls whether JP prints
  a filesystem path or an OSC 8 hyperlink to that file.
- `jp conversation compact` writes each generated summary to
  `<conversation_id>-summary-<from>-<to>.md` and prints the path in its
  timeline.

The tool result path has four defects:

1. **The OS controls file lifetime.** The OS clears its temp directory on its
   own schedule, breaking links in terminal scrollback outside JP's control.
2. **Filenames collide.** `<millis>` is the sub-second component of the system
   clock (`0..=999`), repeating every second.
   A later tool result that lands on the same millisecond and extension
   overwrites the file an earlier link points to.
3. **Failed writes still print a link.** `render_result` ignores the write
   result, so a printed link can point to a missing or stale file.
4. **Unreferenced files are written.** Even with `results_file_link = "off"`,
   rendering an inline tool result writes a temp file.
   Because `jp conversation print` uses the same renderer, replaying a
   conversation rewrites every tool result file.

The summary path has a separate collision bug: its filename depends only on the
conversation ID and turn range.
Compacting a range, removing that compaction with `jp conversation compact
--reset`, and compacting the same range again overwrites the file, so the
earlier printed path opens the newer summary.

## Design

### User-Facing Behavior

Terminal output format is unchanged; only the underlying file path moves:

```
see: /home/user/.local/share/jp/workspace/jp-a1b2c/tmp/conversations/jp-c17457886043/fs_read_file-9f2c4e7a1b3d5f60.rs
```

Each link remains valid for the retention window (30 days by default), measured
from the last time JP printed a link to that file.
`jp conversation print` reuses link files written during live runs and writes a
file only when none exists for that content.

### Storage Location

```
<user-workspace directory>/tmp/conversations/<conversation_id>/<label>-<hash>.<ext>
```

The User-Workspace Directory (W-00dxybd#user-workspace-directory) is the
workspace's per-user data directory (`~/.local/share/jp/workspace/<slug>-<id>/`
on Linux).
JP resolves this directory at startup (`Ctx::user_storage_path`), and link file
resolution simply appends a relative path to it.
No methods are added to the storage backend traits ([RFD 073]) because link
files are rendering artifacts rather than workspace data.

Storing link files in the User-Workspace Directory provides three properties:

- **Outside the project tree.** Link files stay out of version control and are
  invisible to tools that scan project files.
- **Scoped to the workspace.** The directory already holds the user-local
  conversations from which link files are derived ([RFD 031]).
  Command plugins that receive the user-workspace path can access the directory,
  though link files are not a plugin interface (see [Relationship to
  `events.json` and the Blob
  Store](#relationship-to-events-json-and-the-blob-store)).
- **Managed by JP.** JP owns the directory lifecycle and can prune expired files
  on its own schedule.

The `tmp/` subdirectory holds expendable data: deleting it removes only link
files.
Storage operations do not preserve `tmp/`; when merging two user-workspace
directories for the same workspace, JP keeps one `tmp/` directory and may
discard the other.

Nesting under `tmp/conversations/<conversation_id>/` reserves `tmp/` for other
expendable files and groups link files by conversation.
Because a conversation ID is immutable, link file paths survive title changes
and archiving.
Link files are not stored inside the conversation's own storage directory (see
[Alternatives](#alternatives)).

### File Naming

| Component | Value                                                                        |
| --------- | ---------------------------------------------------------------------------- |
| `<label>` | Sanitized tool name for a tool result, or `summary` for a compaction summary |
| `<hash>`  | First 16 lowercase hex characters of the content's SHA-256 digest            |
| `<ext>`   | File extension detected by the writer, or omitted if none                    |

Because tool names come from model output, `<label>` restricts the tool name to
ASCII alphanumeric characters, `_`, and `-`.
If the tool name is unknown, `<label>` defaults to `tool`.

Content-addressed filenames have three consequences:

- **Identical content shares one file.** Replaying a conversation with `jp
  conversation print` resolves to the file written during the live run.
- **Distinct content never collides.** Distinct tool results get distinct
  filenames, as do distinct summaries of the same turn range.
- **The renderer needs only the conversation ID.** It does not need an event ID
  or operation index.
  A fanned-out tool call writes one file per operation during a live run and one
  file for the merged response on replay without index bookkeeping.

### Writing Link Files

- JP writes a link file only when printing a path or hyperlink to it.
  Rendering a tool result with `results_file_link = "off"` writes no file.
  Because the content remains in the event stream, a subsequent render that
  enables links (such as `jp conversation print --style full`) writes the file
  at that time.
- If the target file already exists, JP updates its modification time (`mtime`)
  without rewriting the content.
  Otherwise, JP writes to a temporary sibling file and atomically renames it
  into place so readers never observe a partial write.
- If writing fails, JP logs a warning, omits the link, and still renders any
  inline result.

Link URLs (`file://` and `copy://`) percent-encode path segments.
Formatting a raw filesystem path into a URL breaks when the path contains spaces
or non-ASCII characters, which occurs on macOS where the default user data
directory is `~/Library/Application Support/jp` and `<slug>` comes from the
checkout directory name.

Plain paths printed by `results_file_link = "full"` are not escaped.
Backslash-escaping spaces would help shell pasting but breaks terminal path
auto-detection at the first backslash, whereas `file://` URLs remain clickable
as a single span.

### OS Temp Directory Fallback

JP falls back to writing `<conversation_id>-<label>-<hash>.<ext>` in
`env::temp_dir()` when:

- the command runs with `--no-persist`, which must not write to JP's storage; or
- the workspace has no User-Workspace Directory.

JP never prunes files in the OS temp directory.

### Pruning

| Contract    | Value                                                                     |
| ----------- | ------------------------------------------------------------------------- |
| Config key  | `conversation.link_files.retention_days`                                  |
| Default     | `30`                                                                      |
| Never prune | `"unlimited"`                                                             |
| Rejected    | `0` (fails validation with an error suggesting `"unlimited"`)             |
| Run point   | End of every workspace-loading invocation, except `--no-persist` runs     |
| Scope       | Expired files under `tmp/conversations/`, then any directories left empty |
| On failure  | Logs a trace warning without failing the command                          |

The pruning sweep deletes any link file whose `mtime` is older than the
retention window.
Because printing a link refreshes `mtime`, the retention window is measured from
the most recent print rather than file creation.

The sweep also removes the link-file directory of any conversation that no
longer exists, regardless of file age.
To avoid deleting link files for a conversation created by a concurrent `jp`
process after the current run loaded its index, the sweep re-scans stored
conversation IDs (both live and archived) on disk, matching the session-mapping
cleanup pass.
Because `jp conversation rm` and temporary-conversation expiration run before
the end-of-run cleanup, a deleted conversation's link files are removed in the
same invocation.
Any subdirectory under `tmp/conversations/` whose name does not parse as a
conversation ID is pruned by age only.

The sweep runs inside the existing end-of-run cleanup in `jp_cli::run` alongside
orphaned lock-file and stale session-mapping removal, covering every command
that writes link files.

`0` is rejected because a zero-day window would delete newly written link files
at the end of the run that created them; `"unlimited"` disables pruning.

Unlike [RFD 130], which fixes session-record retention at 30 days, link-file
retention is configurable because link files are plain-text copies of tool
output that can grow large, and users differ on how they trade disk usage
against scrollback link longevity.

### Relationship to `events.json` and the Blob Store

Tool results can exist in three storage locations, each serving a distinct
purpose:

| Location                   | Content                            | Purpose                                     |
| -------------------------- | ---------------------------------- | ------------------------------------------- |
| `events.json`              | Persisted response, base64-encoded | Canonical conversation stream               |
| Blob store ([RFD 066]) | Persisted payload, gzip-compressed | Offloads large payloads from `events.json`  |
| Link file                  | Rendered output as plain text      | Directly openable target for terminal links |

The blob store does not replace link files.
Blobs are content-addressed and gzip-compressed so workspace text searches skip
them, which prevents editors from opening them directly.
Terminal links require an uncompressed file with an appropriate extension,
whether the underlying payload lives inline in `events.json` or in the blob
store.

Link files are disposable rendering artifacts, and the event stream remains the
source of truth:

- **A rendered result may have no link file.** JP writes a file only when
  printing a link, so no file exists when links are disabled, the tool is
  hidden, or the write fails.
- **Link files can be deleted at any time.** JP prunes expired files
  automatically, and removing `tmp/` manually is always safe.
- **Filenames and paths are not a programmatic interface.** Both can change
  without notice, and nothing in JP reads link files back.

Features that consume tool output must read the event stream; features that
require output omitted from the event stream must provide their own storage.

### Code Organization

A `LinkFiles` type in `jp_cli` encapsulates the link-file directory (or OS temp
fallback) for a single conversation and exposes one write method that returns
`Option<PathBuf>`.
Both `ToolRenderer::render_result` and `jp conversation compact` write through
`LinkFiles`, and `jp_cli::run` invokes the sibling pruning function at the end
of each run.

## Drawbacks

- **Plain-text copies persist longer.** `events.json` base64-encodes tool
  content, whereas link files store plain text.
  Moving link files out of the OS temp directory keeps plain-text copies on disk
  for the full retention window (or until the conversation is removed).
- **Disk usage moves to the user data directory.** Storage under the
  User-Workspace Directory grows with the volume of distinct tool results and
  summaries rendered within the retention window.
- **Printed paths are longer and may contain spaces.** On macOS, the default
  user data path contains a space (`Application Support`).
  Under `results_file_link = "full"`, terminals that stop path auto-detection at
  whitespace only link the prefix before the space, and pasting the path into a
  shell requires quoting.
  The `osc8` link style is unaffected.
- **End-of-run sweep overhead.** Every persistent workspace invocation walks
  `tmp/conversations/` and scans stored conversation IDs at exit.

## Alternatives

- **Keep using the OS temp directory with content-addressed filenames.** This
  eliminates filename collisions, but the OS still deletes link files on its own
  schedule.
- **Use the platform cache directory.** System cache cleaners can purge it at
  any time, it is not scoped per workspace, and it introduces another JP-managed
  root directory to document.
- **Store link files inside the conversation's storage directory.** Conversation
  directory names include the title slug and change whenever the title is edited
  or auto-generated at the end of the first query, and archiving moves the
  directory under `.archive/`.
  Either move breaks every previously printed link.
  In addition, storage synchronization and directory merges manage files inside
  conversation directories explicitly.
  (Version control is not an issue because link files would live only in the
  user-local conversation directory, while workspace projection copies only the
  three managed JSON files.)
  If user-local conversation directories later adopt a stable path that survives
  renames and archiving ([T-1n0d3w0]), link files can move there.
- **Place the retention setting under a top-level `[storage]` section.** A
  top-level section would contain only this single key;
  `conversation.link_files` groups it with the rest of the conversation
  settings.
- **Store link files under `.jp/` in the project checkout.** Placing link files
  inside the checkout exposes them to version control and project-scanning
  tools.
- **Create one directory per invocation.** Because a link file depends only on
  its content, per-run directories would duplicate files across every `jp
  conversation print` replay.
  If JP later records per-run diagnostic manifests, those manifests can
  reference the content-addressed link files written during the run.
- **Name link files by the response's `EventId`.** Event-ID naming requires
  threading the `EventId` into the renderer and adding an operation index for
  fanned-out tool calls.
  It also gives each file a name that looks like a stable stream-entry address,
  inviting other features to depend on it.
  Content hashing avoids both costs and produces filenames with no meaning
  outside the link.
- **Emit custom-scheme links that resolve stream entries directly.** A custom
  URI scheme handled by a user script requires no files on disk, but does
  nothing until the user configures a scheme handler.
  `file://` links work out of the box in any terminal supporting OSC 8.
- **Link directly to the blob store.** Rejected for the reasons in [Relationship
  to `events.json` and the Blob
  Store](#relationship-to-events-json-and-the-blob-store).

## Non-Goals

- **Defining a machine-readable schema for link files.** Link files hold the
  human-readable text rendered by the writer; `events.json` remains the
  machine-readable source of truth.
- **Durable payload storage.** Durable storage belongs to the
  content-addressable blob store ([RFD 066]).
- **Exposing link files to the assistant or other features.** Link files are not
  a programmatic interface (see [Relationship to `events.json` and the Blob
  Store](#relationship-to-events-json-and-the-blob-store)).
- **Preserving tool output omitted from the event stream.** [RFD 115] describes
  the renderer's temp file as retaining untruncated tool output beyond its
  persistence cap.
  This RFD makes no such guarantee (when `results_file_link = "off"`, JP writes
  no link file), leaving untruncated output storage to RFD 115.
- **Changing link rendering options.** When links are printed, their display
  text, and `results_file_link` (including its error overlay) remain unchanged.
- **Relocating diagnostic temp files.** Trace logs and dumped request or config
  payloads in the OS temp directory have separate lifecycle requirements.
- **Size-based cache eviction.** Pruning is strictly age- and
  conversation-based.

## Risks and Open Questions

- **Workspace-wide pruning uses the active run's retention window.** The
  end-of-run sweep prunes `tmp/conversations/` across the entire workspace using
  the current invocation's resolved `retention_days`.
  Running a conversation configured with a shorter retention window prunes other
  conversations' link files to that shorter window.
- **Conversations present only in another checkout's projection.** If a
  conversation exists exclusively in checkout A's `.jp/conversations/` and has
  not been imported into user-local storage, a `jp` invocation in checkout B
  does not see that conversation ID during its sweep and deletes its link files.
  Existing links break early, though reprinting the conversation in checkout A
  recreates the files.
- **Directory walk cost at exit.** The sweep stats every link file in the
  workspace on every run.
  If profiling shows noticeable latency, a timestamp marker file can throttle
  the age-based sweep to once per day.
- **Filesystems without reliable `mtime`.** On filesystems that do not track
  modification times, link files may be pruned prematurely or never expire.
- **Sweep race with concurrent rendering.** One process's sweep can read an
  expired `mtime` just before a concurrent process refreshes it and prints a
  link, unlinking the file right after the link is printed.
  The next render recreates the file.
- **Hash truncation collisions.** Truncating SHA-256 to 16 hex characters yields
  a 64-bit space scoped to a single conversation directory, making collisions
  negligible at the number of tool results in a conversation.

## Implementation Plan

1. **Add `LinkFiles`.** Implement `LinkFiles` in `jp_cli` with directory
   resolution, OS temp directory fallback, content-addressed naming, atomic
   write-if-absent via rename, and `mtime` refresh.
   Unit-test against a temporary root with deterministic content and filenames.
   This phase can merge independently before any callers are wired up.
2. **Migrate link-file writers.** Route `ToolRenderer::render_result` and `jp
   conversation compact` through `LinkFiles`.
   Update `render_result` to pass the tool name as the label, write a file only
   when emitting a path or link, suppress the link on write failure, and
   percent-encode link URLs.
   Retain `LinkFiles` on `TurnRenderer` across per-turn `ToolRenderer` rebuilds.
   Depends on Phase 1.
3. **Add configuration and end-of-run pruning.** Add
   `conversation.link_files.retention_days` (with validation rejecting `0`) and
   wire the pruning sweep (including deleted-conversation cleanup) into the
   end-of-run cleanup in `jp_cli::run`.
   Depends on Phase 1.
4. **Update documentation.** Add a "Link File" entry to the Ubiquitous Language
   glossary, document `tmp/` under the User-Workspace Directory entry
   (W-00dxybd), and document `conversation.link_files.retention_days`.

## References

- [RFD 031]: Durable conversation storage and the User-Workspace Directory.
- [RFD 066]: Content-addressable blob store for durable payload storage.
- [RFD 073]: Layered storage backend traits (unchanged by this RFD).
- [RFD 115]: Bounded tool output, which describes the renderer's temp file as
  retaining output beyond its cap; this RFD does not guarantee that (see
  [Non-Goals](#non-goals)).
- [RFD 130]: Session record retention, which establishes the 30-day retention
  precedent.
- `crates/jp_cli/src/render/tool.rs`: `ToolRenderer::render_result`.
- `crates/jp_cli/src/cmd/conversation/compact.rs`: `write_summary_file`.
- `crates/jp_cli/src/lib.rs`: End-of-run cleanup in `jp_cli::run`.
- `crates/jp_cli/src/ctx.rs`: `Ctx::user_storage_path`.
- `crates/jp_config/src/fs.rs`: User data directory resolution.

[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 066]: 066-content-addressable-blob-store.md
[RFD 073]: 073-layered-storage-backend-for-workspaces.md
[RFD 115]: 115-bounded-tool-output.md
[RFD 130]: 130-session-record-retention-and-read-surface.md
[T-1n0d3w0]: ../ticket/1n0d3w0-give-user-local-conversation-directories-a-stable-path.md
