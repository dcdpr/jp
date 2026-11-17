# Make dash a jp command plugin

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-10
- **Label**: domain=plugins
- **Label**: package=jp_plugin
- **Label**: relevant-rfd=072
- **Label**: relevant-rfd=114
- **Label**: type=pre-rfd

`dash` (`crates/internal/dash`) is an internal CLI: a terminal dashboard of a
repository's worktrees, running `jp` processes, sessions, and pull requests.
It already works in other JP workspaces without code changes, as long as the
repository is laid out the way it expects (worktrees as siblings in a container
directory, `.jp/.id` committed on the base branch, `origin/HEAD` naming the base
branch).
Making it a `jp dash` command plugin that other people can install needs design
work first.
This ticket records the analysis so it outlasts the conversation it came from.

## Blocker: plugins cannot own the terminal

`spawn_plugin` (`jp_cli/src/cmd/plugin/dispatch.rs`) pipes stdin, stdout, and
stderr, and starts the plugin in its own process group.
RFD 072 states that a plugin never writes to the user's terminal.
A full-screen TUI cannot run under that:

- stdout is the protocol pipe, so frames would land in the host's JSON reader.
- Opening `/dev/tty` does not help: the plugin's process group is not the
  terminal's foreground group, so raw mode raises SIGTTOU and reading keys
  raises SIGTTIN.

Proposed: a manifest claim (e.g. `terminal: exclusive`) read at admission, so a
run without a terminal refuses the plugin up front.
For such a plugin the host saves the terminal state, stops its own printer
status region and interrupt handler, hands the foreground group to the plugin
with `tcsetpgrp`, and restores both when the plugin exits.
The plugin draws to and reads from `/dev/tty`; stdin and stdout stay
protocol-only.
The host restoring the terminal also covers a plugin that crashes in raw mode.

## Plugin shape

Follow `jp-ticket` and `jp-gui`: library `run(stdin, stdout)`, thin `main.rs`,
`jp_plugin::manifest!` with `command: ["dash"]`, `describe` support.

- Take the directory from `init.workspace.root` instead of `env::current_dir()`.
- Paint to a `/dev/tty` handle.
  Check whether crossterm 0.29's raw mode and event reading fall back to
  `/dev/tty` when stdin is not a terminal.
- Read protocol messages on a thread and turn `Shutdown` into an update that
  ends the event loop, then send `exit`.
- Rename the binary: `dash` collides with `/bin/dash` on macOS.

Options under `plugins.command.dash.options`, in the style of
`jp-ticket/src/options.rs`, for what is hard-coded today: worktree directory,
base branch, whether to auto-update the base branch (default off for other
repositories, since it runs `fetch` and `merge --ff-only`), whether to read
GitHub, terminal backend (`ghostty` or `none`).
Leave poll intervals as constants.

## Protocol coverage vs. what dash reads today

None of RFD 114 is built yet (tracking ticket T-0t06608 is Todo).
Only its Phase 1 matters to dash, and it does not depend on RFD 113.
Dash can stay a `single`-scope plugin: every worktree reads the same user-local
store.

| What dash needs                                     | Today                                                     | Protocol today                                                 | RFD 114 adds                                 | Still missing                                        |
| --------------------------------------------------- | --------------------------------------------------------- | -------------------------------------------------------------- | -------------------------------------------- | ---------------------------------------------------- |
| Worktree list                                       | `git worktree list`                                       | none                                                           | `list_workspaces` roots (JP-registered only) | Nothing; git stays the authority                     |
| Title, pinned, event count                          | `load_conversation_metadata_batch`                        | `list_conversations`                                           | none                                         | `last_event_at` on summaries                         |
| Whether a turn is running                           | lock file PID, checked with `ps`                          | `read_events.lock`, one conversation per request, whole stream | none                                         | Lock state on `list_conversations` entries           |
| Worktree where a conversation runs or last ran      | `lsof` cwd of the lock holder, plus dash's own cache file | none                                                           | `last_root` (Phase 1)                        | Nothing; replaces both                               |
| Whether a turn is waiting for user input            | loads the stream, `pending_inquiries()`                   | raw events only                                                | none                                         | Derived `waiting_since` on entries and `read_events` |
| Sessions: leader PID, active conversation, history  | `Workspace::session_overviews()`                          | none                                                           | none                                         | A `list_sessions` request; no RFD covers it          |
| Projection-only conversations in a sibling worktree | not covered                                               | none                                                           | Phases 2 to 4                                | Nothing needed for dash                              |

OS-level data stays in the plugin regardless: process table, shell cwd and tty,
foreground-group idle check, Ghostty, `git`, `gh`.

## Linking JP crates directly

JP intends for other Rust tools to build on its crates (`jp_workspace`,
`jp_storage`, `jp_conversation`, and others), so linking them is not wrong in
principle.
They are not ready for it yet: their public surface is wide, shaped by `jp`'s
own internal needs, and changes without notice.
Until they expose a narrow, stable API, the plugin protocol is the only stable
surface.
A `jp-dash` plugin would read the store directly as an explicit temporary
exception and move each read to the protocol once supported.

The API is not the only risk.
`jp` and a plugin are installed separately, so a plugin can read lock files,
session mappings, and metadata written by a different `jp` version.
A stable crate API alone does not fix that; the on-disk formats would need the
same promise.
The protocol avoids it because the host reads its own files.
Revisit if a second plugin wants session or lock data: that should be a protocol
message, not a second plugin linking the store.

## Proposed RFDs

1. Terminal hand-off for command plugins (extends RFD 072).
   Blocks the rest.
2. jp-dash (requires 1): the plugin, its options, the table above, and the crate
   exception.
3. Session and run state over the plugin protocol (extends RFD 072):
   `list_sessions`, plus `last_event_at`, lock state on entries, and
   `waiting_since`.
   With RFD 114 Phase 1, this lets jp-dash drop the direct crate dependency.

RFD 114 is not on the priority board; dash is a reason to rank its Phase 1.
