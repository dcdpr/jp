# RFD 087: Session-Scoped Active Workspace

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-01
- **Extends**: [RFD 020]
- **Tracking Issue**: [\#793]
- **Extended by**: [RFD 112], [RFD 113], [RFD 114]
- **Summary**: Session-scoped active workspace lets JP commands run from
  anywhere after selecting a workspace with `jp w use`.

## Summary

Each terminal session can select an active workspace with `jp w use`, letting
`jp q` run against that workspace from any directory without `--workspace`.
This extends [RFD 020]'s per-session active conversation model to workspaces and
resolves which checkout a command targets when [RFD 031] maps one workspace ID
to multiple git worktrees.

## Motivation

[RFD 031] makes every git worktree of a repository share one user-local store
keyed by workspace ID, fixing durability and directory-collision crashes.
However, when a workspace ID maps to multiple checkouts on disk, JP has no way
to know which checkout a command should act on.

`--workspace=<id>` is ambiguous across checkouts, and JP cannot target a
workspace from outside its directory without it.
Users must `cd` into the target worktree before running `jp`, and multi-checkout
workspaces have no defined resolution behavior.

[RFD 020] solved the same problem for conversations by giving each terminal tab
its own active conversation.
Applying that model one level up gives each tab an active workspace, so `jp q`
works from anywhere and multi-checkout resolution becomes an explicit,
per-session choice.

## Design

### Workflow

A fresh terminal has no active workspace, so `jp` operates on the workspace in
the current working directory.

To drive a workspace from anywhere, select one for the session:

```console
$ cd ~/scratch
$ jp w use ?
? Select a workspace
> jp        ~/Projects/jp.git/my-feature
  jp        ~/Projects/jp.git/main
  dotfiles  ~/.dotfiles
$ jp q "summarize the last commit"   # runs in ~/Projects/jp.git/my-feature
```

The selection is scoped to the current tab, just like the active conversation in
[RFD 020]; another tab can select a different workspace without interference.
`jp w show` reports the current selection, and `jp w use cwd` clears it to fall
back to cwd resolution.

### Two-layer session model

- Session identity is unchanged from [RFD 020] (workspace-independent:
  `$JP_SESSION`, `getsid(0)` / console HWND, per-pane terminal vars).

- New **global layer**: maps a session to its active workspace in a user-global
  session store at `~/.local/share/jp/sessions/<source-key>.json`, above any
  `<id>`.
  This store mirrors [RFD 020]'s session mapping structure: a most-recent-first
  `history` of selected workspaces (each entry records the workspace `<id>`, its
  resolved checkout root, and a timestamp) and a session-level `sticky` flag.
  The active workspace is `history[0]`; the previous workspace is `history[1]`
  (the `session` / `s` target; see [The `jp workspace` command
  surface](#the-jp-workspace-command-surface)).
  Recording the `<id>` alongside the root allows recovery after the root is
  deleted (see [Reprompt on a missing active
  workspace](#reprompt-on-a-missing-active-workspace)).
  The filename encodes the full session *source*, not just its value:

  ```text
  getsid-<pid>.json
  hwnd-<handle>.json
  env-<KEY>-<hash(value)>.json   # e.g. env-JP_SESSION-9f86d0….json
  ```

  An automatic `getsid` or `Hwnd` session never aliases an `Env` session with
  the same numeric value, and distinct environment variables (`$JP_SESSION`,
  `$TMUX_PANE`) holding the same value map to separate files.
  Hashing the opaque environment variable value also keeps unsafe characters out
  of the filename.
  This reuses the `Session::storage_key` encoder from [RFD 020]'s per-workspace
  session store so both stores stay consistent and key collisions cannot route a
  command to the wrong workspace.
  Two tabs that set the same `$JP_SESSION` still share a record (same source,
  key, and value), which remains the supported way to link sessions.

- Existing **per-workspace layer**: maps a session to its active conversation at
  `<id>/sessions/<session-key>.json`, unchanged.

- **Composition**: to run `jp q` from anywhere, JP resolves the session to its
  active workspace (global), enters that workspace (see [Execution context:
  launch cwd, workspace root, and child
  cwd](#execution-context-launch-cwd-workspace-root-and-child-cwd)), resolves
  the session to its active conversation (per-workspace), and executes the
  command.

### Startup ordering

Selecting a workspace by ID happens *before* a `Workspace` exists, inverting the
prior startup sequence where `run_inner` loaded the workspace before resolving
session identity.
A dedicated bootstrap step in `jp_cli` runs before workspace construction:

1. Resolve session identity (`session::resolve`).
2. Inspect cwd for a workspace, if any.
3. Read the user-global session store for this session's active workspace.
4. Read the per-workspace roots registries under the user data directory.
5. Choose a concrete checkout root (see
   [Precedence](#precedence-and-the-cwd-vs-active-conflict)).
6. Construct `Workspace`, load config, load the conversation index, and run the
   command.

User-data scanning and root selection stay in this `jp_cli` bootstrap step.
`jp_workspace::Workspace` continues to manage an already-selected workspace and
does not know how the root was chosen.

The bootstrap step resolves the selection once (launch cwd, selected workspace
root, child cwd, resolution source, and session) and passes those values
explicitly to downstream consumers (workspace construction, config loading, MCP
and plugin spawns, local tools, and user-typed path parsing).
Consumers do not re-derive paths from the process cwd at each call site, keeping
launch cwd, workspace root, and child cwd distinct.

### Workspace bootstrap requirement

Not every command needs a selected workspace.
Each command declares its workspace bootstrap requirement, analogous to the
per-command `conversation_load_request` (`jp_cli::cmd`):

- **none**: no workspace is bootstrapped.
  `jp init` creates the workspace that other commands select.
- **subject**: the workspace is the command's *subject* rather than its context.
  `jp w` resolves its own target against the pre-workspace environment from
  outside any workspace (and possibly to no workspace at all), so bootstrap
  selects no root on its behalf.
  `jp w show <id>` reports *on* a workspace instead of running *inside* one; `jp
  w use <id>` records a selection for subsequent runs; `jp w ls` reads only the
  user-global registries.
- **load**: run the resolution ladder, construct `Workspace`, and load the
  conversation index.
  Used by `jp q` and all other workspace-consuming commands.

A fourth *resolve* mode (resolving a validated root without loading the
conversation index) was considered for `jp w use` but has no caller: a `subject`
command resolves its own target, and no other command needs a validated root
without conversations.

The bootstrap output therefore includes a *no workspace selected* state.
For `none` and `subject` commands (including `jp w show` run outside any
workspace with no active selection), downstream consumers that require a root
(config loading, MCP and plugin child cwd, path parsing) do not run.

### Bootstrap storage ownership

The `jp_cli` bootstrap step, not `Workspace`, owns the global session store and
the roots registries.
`Workspace::cleanup_stale_files` requires an `FsStorageBackend` bound to the
selected workspace, touches only that workspace's lock files and conversation
session mappings, and runs at the *end* of a command after a workspace exists.
It cannot access user-global records before workspace selection or inspect roots
for unselected workspace IDs.

Cleanup responsibilities split accordingly:

- **Workspace cleanup (unchanged):** prunes conversation locks and per-workspace
  conversation session mappings for the selected workspace.
- **Bootstrap cleanup (new):** manages user-global active-workspace session
  records and roots-registry pruning.
  Before workspace selection, it reads the active workspace, validates candidate
  roots, and prunes dead entries.
  After the command completes, it upserts the current root and removes the
  global session record if its session source is dead.

Dead-root pruning is opportunistic: bootstrap prunes only the roots it inspects
while resolving a selection, rather than scanning every known workspace on each
run.

### Execution context: launch cwd, workspace root, and child cwd

Running from anywhere requires distinguishing three directories that coincide
only when JP runs inside the target workspace:

- **launch cwd**: the directory where the user invoked `jp`, against which the
  shell completes relative path arguments.
- **workspace root**: the selected checkout root.
- **child cwd**: the working directory inherited by spawned MCP servers,
  plugins, and local tools.

Whenever JP operates on a workspace whose root differs from the launch cwd's
workspace (via a session-active selection, an explicit `-w <path>` or `-w <id>`,
or the fallback picker), **child cwd** is set to the selected workspace root.
Config loading, MCP servers, plugins, and local tools all run as if launched
from that root.
Without this rule, from-anywhere runs would mix bases, since config loading and
MCP/plugin spawns otherwise inherit the process cwd while local tools use
`workspace.root()`.

**User-typed relative path arguments resolve against the launch cwd, not the
workspace root.** When a user in `~/scratch` runs `jp q --attach ./foo.txt`, the
shell completes `./foo.txt` against `~/scratch`; re-rooting that path to the
workspace root would break shell completion expectations.

When JP is launched from *inside* the target workspace, child cwd remains the
launch cwd so subdirectory `.jp.toml` files continue to load as usual.

**Minimum path behavior for this RFD.** Resolving relative paths against the
launch cwd preserves the existing workspace-containment rule: a relative path
that resolves outside the selected workspace returns an error.
This RFD adds no mounting, snapshotting, or external `file://` resource handling
for outside-workspace paths; those are deferred to a dedicated path RFD (see
[Non-Goals](#non-goals)).
From-anywhere queries (`jp q "…"`) and attachments inside the selected
workspace work immediately, while outside-workspace attachments fail closed.

The full per-input-class resolution model (covering relative paths, mount specs,
cwd-targeted config edits, and containment policy for outside-workspace paths)
is deferred (see [Non-Goals](#non-goals)).

### Roots registry (one workspace ID, many checkouts)

A roots registry replaces the single `storage` symlink, mapping one workspace ID
to all of its checkouts on disk.
Because a single shared file updated via read-modify-write could lose concurrent
updates and drop a checkout, the registry is a *directory of per-root files*,
one per checkout, matching the layout used for sessions and locks:

```text
~/.local/share/jp/workspace/<id>/roots/<root-key>.json
```

- `<root-key>` is a stable hash of the checkout's canonical path, so each
  checkout owns one file and writes never contend.
- Each run upserts only its own file with the canonical path and a `last_used`
  timestamp, and no file is modified by more than one checkout.
  Updates are debounced: an entry refreshed within the last few minutes is left
  untouched, because `last_used` only drives display ordering and `recent`
  targeting, and rewriting on every invocation would churn the user data
  directory and trigger external file watchers.
- Liveness is **derived**, not stored: a root is live when workspace discovery
  from that path resolves a workspace whose loaded ID equals `<id>` (in the
  colocated layout, `<root>/.jp/.id`).
  A path that was deleted or recreated as a different workspace is not live, and
  bootstrap cleanup prunes its file (see [Bootstrap storage
  ownership](#bootstrap-storage-ownership)).
- Because entries are plain files, they avoid the Windows symlink privilege
  required by the legacy `storage` symlink.

<!-- end list -->

```json
// ~/.local/share/jp/workspace/<id>/roots/<root-key>.json
{ "path": "/Users/jean/Projects/jp.git/my-feature", "last_used": "2026-06-01T18:25:00Z" }
```

The roots registry is workspace-scoped under the user-workspace directory
(`<slug>-<id>`, located by ID suffix per [RFD 031]; `<id>` in the paths above
denotes that directory), whereas the global session store lives under
`sessions/` and maps a session to its active workspace.

### Migration from the `storage` symlink

Previously, the user-local back-pointer to a checkout was a single `storage`
symlink (`<id>/storage` → the checkout's `.jp`), which `-w <id>` used to
resolve a workspace root.
[RFD 031] Phase 1 renames the user-local directory to `<id>` without changing
that symlink, so RFD 087 replaces it with the roots registry.

Migration is best-effort and not required for correctness.
On the first bootstrap for an `<id>`, if a legacy `storage` symlink exists, JP
canonicalizes its target, verifies that it resolves to a workspace with ID
`<id>`, and seeds `roots/<root-key>.json` from it before ignoring or removing
the symlink.
Dead or mismatched symlink targets are pruned.
Beyond this one-time seed, each checkout upserts its own registry entry whenever
a command runs inside it, so an unseeded checkout is unavailable to `-w <id>`
only until the user next runs JP from that checkout.

This migration requires [RFD 031] Phase 1 to be *implemented*, not merely
accepted (see [Implementation Plan](#implementation-plan)).

### The `jp workspace` command surface

`jp workspace` is the canonical command and `jp w` is its short alias, mirroring
`jp conversation` / `jp c`.
The examples below use `jp w`.

- `jp w use <target>`: select the session's active workspace using the targeting
  grammar below.
  `jp w use ?` opens the picker, which lists known workspaces, expands each
  through the roots registry to its live checkouts, and prompts for one.
  Bare `jp w use` switches to `s`, falling back to `?` when the session has no
  live previous workspace, mirroring bare `jp c use`.
  `jp w use` is interactive-only in all forms (including `cwd`) because it
  mutates session state; scripts use `jp -w` instead (see [Non-interactive
  mode](#precedence-and-the-cwd-vs-active-conflict)).
  Selecting a checkout also registers it in the roots registry, making a
  workspace selected by path immediately reachable by `<id>` from anywhere, even
  before any command has run inside it.
- `jp w use <target> --always`: select the workspace *and* mark the session
  **sticky** to it, so the selection takes precedence over cwd and suppresses
  the conflict prompt.
  This sets the same state up front as choosing `A` at the conflict prompt.
  Every `jp w use` sets the session's full sticky state, so running `jp w use`
  *without* `--always` clears an existing sticky flag.
  Combining `--always` with `cwd` is rejected because `cwd` removes the session
  record that holds the flag.
- `jp w use cwd` (short `.`): clear the session's active workspace and return to
  cwd resolution, replacing a separate `--clear` flag.
- `jp w ls`: list known workspaces and their checkouts, mirroring `jp c ls`.
- `jp w show [<target>]`: report the session's active workspace when called
  without arguments, or report `<target>` (e.g. `jp w show <id>`), mirroring `jp
  c show`.
  The output shows how the workspace was resolved, whether the session is
  **sticky** to it, whether cwd overrides it, the conversation count, and the
  active conversation (if any, shown once per `<id>`).
  The conversation count is the union of conversation IDs across the user-local
  durable store and every live root's `.jp/conversations/` directory, including
  external (`ext`) conversations present in only one checkout (see [RFD 031]).
  To compute this without a full workspace load per root, `jp w show <id>` loads
  the conversation index (metadata only, not event contents) for one live root,
  which already merges the user-local store, and scans sibling roots'
  conversation directories to deduplicate any remaining IDs.
  When the target resolves to a single root (a path, or an `<id>` with one live
  root), the output shows that root.
  If a path target names a checkout not yet in the roots registry, `show` still
  lists it without a `last_used` timestamp rather than reporting "no live
  checkouts" for the path just queried.
  When an `<id>` has multiple live roots, `jp w show` lists all of them and
  marks the session-active root (if any) without prompting, keeping `show`
  read-only and script-friendly.
  `jp w show` performs no writes: it creates no user-workspace directory, runs
  no migration or import, writes no registry entry, and leaves `last_used`
  untouched so `r` / `recent` resolution never changes.
- `jp -w <target>`: override the workspace for a single invocation using the
  same targeting grammar, without changing the session's active workspace.
  On `jp w use` and `jp w show`, `-w` specifies the subcommand's target, making
  `jp -w foo w show` equivalent to `jp w show foo`; passing both is an error,
  and `jp w ls` rejects `-w` because it always lists all workspaces.
  A bare `<target>` is resolved as a filesystem path if that path exists, and
  otherwise parsed as a workspace ID (so a local directory with the same name
  shadows the ID).
  When an ID has one live root, JP uses it; when it has multiple live roots,
  interactive runs prompt and non-interactive runs fail with a list of candidate
  roots.

#### Workspace targeting grammar

`jp w use` and `jp -w` share a `WorkspaceTarget` grammar modeled on
`ConversationTarget` (`jp_cli::cmd::target`), reusing the same keywords and
single-letter aliases where applicable:

| Target           | Meaning                                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------------------------- |
| `<id>`           | a literal workspace ID                                                                                              |
| free text        | fuzzy-match known workspaces by slug / path / ID → picker (slug is cosmetic)                                        |
| `?`              | pick from all known workspaces                                                                                      |
| `?s`, `?session` | pick from this session's workspace history                                                                          |
| `s`, `session`   | the session's previously active workspace (like `cd -`)                                                             |
| `r`, `recent`    | the live root with the newest `last_used` across the roots registry (global recency, distinct from `s` / `session`) |
| `cwd`, `.`       | the cwd-derived workspace; as a `use` target, clears the session selection                                          |
| `-`              | read a workspace ID from stdin (for `jp -w` in non-interactive use)                                                 |
| `help`           | print keyword help and exit                                                                                         |

> [!WARNING]
> **`.` is already taken in `ConversationTarget`.** It is the short form of
> `active`, the session's active conversation.
> Mapping `.` to `cwd` here gives one character opposite meanings across the two
> grammars: in `ConversationTarget` it selects the session's active item, while
> in `WorkspaceTarget` it **clears** the session selection.
> A user who learns one grammar will predict the wrong behavior in the other.
>
> Suggested resolution: drop the `.` alias here and keep `cwd` as the only
> spelling.
> `cwd` is already short and unambiguous, whereas `ConversationTarget` has no
> alternative short alias for `active` (`a` is taken by `archived`).

Keywords that do not apply to workspaces are omitted: `newest` / `n` (workspaces
have no creation timeline), the `pinned` / `p` family (workspaces have no
listing pin; see [Relationship to `jp
conversation`](#relationship-to-jp-conversation)), and the archive keywords.
The multi-target keywords (`+session`, `+pinned`, `+archived`) are also omitted
because `jp w use` and `jp -w` each select a single workspace; they can be added
later if a multi-workspace command is introduced.

Session-history targets (`s`, `?s`) operate on concrete history entries
(workspace ID *plus* checkout root), not on the workspace ID alone.
`s` restores the exact previously active checkout (`cd -` semantics) rather than
re-resolving the previous workspace ID against its current roots, and multiple
checkouts of the same ID are distinct history entries.
If that exact checkout no longer exists, `s` fails instead of substituting a
sibling checkout and directs the user to the full picker (`?`) rather than `?s`,
which filters session history by the same exact-root rule.

The picker and fuzzy free-text matcher display each workspace by its **slug**:
the `<slug>` prefix of the `<slug>-<id>` user-workspace directory (see [RFD
031]), captured from the workspace directory name when first created.
The slug is cosmetic: if absent (a bare `<id>` directory), the `<id>` is
displayed instead; it is never renamed and need not be unique.
Fuzzy matching searches across slug, path, and ID, but resolution always uses
the workspace ID and concrete root, so a shared or stale slug affects only
display and search.

### Precedence and the cwd-vs-active conflict

Interactive resolution order:

1. Explicit `-w` wins.
2. Otherwise, if the session is **sticky** to its active workspace (`A`, below)
   and that workspace is still live, use it even when cwd resolves to a
   different workspace.
3. Otherwise, if a session-active workspace is set and cwd resolves to a
   *different* workspace (or a different checkout of the same ID), prompt.
4. Otherwise, cwd wins when present.
5. Otherwise, use the session-active workspace when live.
6. Otherwise, open the picker.

The step-6 picker resolves only the current run and does not record a session
selection.
Resolution may **repair** an existing session selection whose checkout was
removed, but it never creates a new selection; binding a workspace to a session
requires `jp w use` or choosing `C` or `A` at the conflict prompt.
A session with no recorded selection prompts again on the next run, matching the
conversation picker's behavior.

The conflict prompt appears whenever cwd and the active selection differ (by
workspace ID *or* by checkout root of the same ID):

```text
How to proceed? [c/C/a/A/q]
c - use current workspace
C - use current workspace and make it session-active
a - use active workspace
A - use active workspace and keep the session sticky to it
q - quit without running command
```

Choosing `A` sets the `sticky` flag on the session record, matching `jp w use
<target> --always`.
Stickiness applies only in interactive mode; running `jp w use` without
`--always` clears the flag, and `jp w use cwd` removes the session record
entirely (see [Session store and cleanup](#session-store-and-cleanup)).

**Non-interactive mode ignores the session-active workspace.** Here
"non-interactive" means JP cannot prompt the user, using the same promptability
check as plugin installation, lock-timeout handling, and tool-call prompts.
[RFD 049] defines the canonical promptability signal (controlling-terminal
availability rather than a TTY on stdout), and RFD 087 uses that shared check.
In non-interactive mode, a command with bootstrap requirement `load` must run
inside a workspace or pass an explicit `-w`, and errors otherwise.
Non-interactive `-w` accepts only concrete targets: a workspace `<id>`, a path,
`cwd` / `.` (resolve from the invocation directory), or `-` (read an ID from
stdin).
Session-derived targets (`s` / `session`, `?s` / `?session`) and pickers (`?`,
fuzzy free-text) require session state or a prompt and therefore fail
non-interactively.
Read-only commands (`jp w ls` and `jp w show`) work non-interactively because
they report registry and session state without prompting.
`jp w use` always fails non-interactively because it mutates session state;
scripts use cwd resolution by omitting `$JP_SESSION` rather than running `jp w
use cwd`.
This keeps scripts deterministic and independent of hidden session state.

### No session identity

A session-active workspace requires a session identity (`$JP_SESSION`, `getsid`
/ `Hwnd`, or a per-tab terminal variable; see [RFD 020]).
Lacking a session identity is distinct from having no active workspace:

- `jp w use` fails without a session identity, matching `jp c use` ("No session
  identity available.
  Set `$JP_SESSION` or run in a terminal with automatic session detection."),
  because there is no session record to persist the selection in.
- `jp q` run interactively outside a workspace without a session identity skips
  the session-record steps (2, 3, and 5) and falls through to the step-6 picker
  for that run only.
  Non-interactively, it fails with a message directing the caller to pass `-w`.

### Reprompt on a missing active workspace

If the recorded checkout root no longer exists (for example, after a worktree is
removed), recovery uses the `workspace_id` stored alongside it in the session
record: JP reads `<id>`, expands `workspace/<id>/roots/` to its live checkouts,
prunes dead entries, and prompts the user to choose among the remaining roots
(or uses the sole remaining root directly).
Storing `<id>` in the session record makes this possible because `<id>` can no
longer be read from `<root>/.jp/.id` once `<root>` is deleted.
This mirrors `session_active_conversation`, which returns `None` and falls back
to the picker when the active conversation has been deleted.

### Session store and cleanup

The global session store mirrors [RFD 020]'s session mapping: a
most-recent-first `history` of selected workspaces (active = `history[0]`,
previous = `history[1]`), a session-level `sticky` flag, and the session
`source`.
Because global session records outlive any single workspace, cleanup depends on
the session source:

```json
// ~/.local/share/jp/sessions/getsid-12057.json
{
  "history": [
    {
      "workspace_id": "...",
      "root": "/Users/jean/Projects/jp.git/my-feature",
      "selected_at": "2026-06-01T18:25:00Z"
    }
  ],
  "sticky": false,
  "source": { "type": "getsid" }
}
```

The `sticky` field stores the `A` choice from the conflict prompt (or
`--always`).

- **`getsid` / `Hwnd`**: reuses [RFD 020]'s process-liveness check and removes
  the session record when the originating process is dead.
- **`Env` (including `$JP_SESSION`)**: process liveness is unknown, so cleanup
  checks resource existence across the full history, mirroring [RFD 020]'s `Env`
  rule (which retains a mapping while *any* referenced conversation exists).
  A history entry is pruned only when neither its recorded checkout root nor any
  registered root for its `workspace_id` is live; the session file is removed
  only when no history entry remains.
  Keying cleanup on `workspace_id` rather than only the active root preserves
  the session record when the active root is deleted but sibling checkouts of
  that `<id>` remain, allowing missing-root recovery to prompt among them (see
  [Reprompt on a missing active
  workspace](#reprompt-on-a-missing-active-workspace)).
  Checking the recorded checkout path directly also protects a newly selected
  checkout that has not yet been written to the roots registry, preventing the
  same run from pruning a selection it just recorded.
- For `Env` sources, a `sticky` flag has no process lifetime and therefore
  persists until its `workspace_id` has no live roots.
  If the active root is removed while a sibling checkout survives, recovery
  prompts among the remaining roots rather than clearing the sticky selection.

### Relationship to `jp conversation`

`jp workspace` mirrors `jp conversation` so both commands share the same model:
the global session store uses [RFD 020]'s history structure (`history[0]` is
active), session identity detection (`getsid` / `Hwnd` / `Env`), and stale
cleanup rules; `jp w use`, `ls`, and `show` parallel `jp c use`, `ls`, and
`show`; and `WorkspaceTarget` reuses `ConversationTarget`'s keywords and aliases
(`?`, `?s`, `s` / `session`, `r` / `recent`, `-`, `help`).

Two behaviors differ because workspaces are rooted in the filesystem:

- **The cwd-vs-active conflict and the `sticky` flag have no conversation
  equivalent.** Conversations have no ambient directory-derived candidate
  competing with the session's active conversation, whereas workspaces have the
  current working directory.
  The conflict prompt, precedence ladder, and `sticky` flag exist to resolve
  that ambiguity.
  **Sticky** means the session continues using its active workspace even when
  cwd points to another workspace, which is distinct from a **Pinned
  Conversation** (an importance marker on a conversation in listings).
- **`cwd` is a workspace-only target.** `jp w use cwd` clears the session
  selection and restores cwd resolution; conversations have no cwd fallback.

Conversely, `ConversationTarget`'s multi-target keywords (`+session`, `+pinned`,
`+archived`) and pin/archive keywords are omitted because workspaces cannot be
pinned in listings or archived.

## Drawbacks

- Adds a user-global session store and cleanup pass, including an
  existence-based cleanup rule for `Env` session sources (see [Session store and
  cleanup](#session-store-and-cleanup)).
- Cold-start double prompt: running `jp q` from outside any workspace in a fresh
  session prompts first for a workspace and then for a conversation.
  This is acceptable for the initial implementation.
- The conflict prompt adds an interaction step when cwd and the active workspace
  differ, though only for users who have selected a session-active workspace.

## Alternatives

- **Always prefer cwd, with no session-active workspace.** Rejected: prevents
  running `jp` from outside a workspace directory, which is the primary goal of
  this RFD.
- **Always prefer the session-active workspace over cwd.** Rejected: silently
  targets the wrong workspace when the user `cd`s into another project.
- **Store a single global active workspace instead of per-session state.**
  Rejected: breaks independent parallel terminal tabs ([RFD 020]).
- **Use a multi-target symlink for the checkout back-pointer.** Rejected:
  symlinks are single-target filesystem primitives, whereas a per-root registry
  directory models one-to-many mappings directly.

## Non-Goals

- **Git awareness.** Consistent with [RFD 031], JP does not inspect git worktree
  topology.
- **Attachment portability across checkouts.** Continuing a conversation from a
  different checkout can re-resolve workspace-relative attachments against that
  checkout.
  Once [RFD 065]'s snapshot-at-attach model is implemented, attachment content
  will be captured at attach time rather than re-resolved on later turns.
  Until then, the precedence ladder and conflict prompt serve as a guardrail,
  and [RFD 065] is not a prerequisite for this RFD.
- **Re-embedding the workspace ID in conversation IDs.** Surfacing workspace IDs
  in conversation IDs helps users discover IDs for `jp w` and `jp -w`, but is an
  independent change.
- **A global fallback workspace outside any project.** Issue #144 proposes a
  "global local" workspace when running outside any project workspace.
  This RFD only allows commands launched outside a workspace to target an
  *existing* workspace via session state, the picker, or `-w`.
- **A unified path-resolution model.** Defining how every input class (relative
  path arguments, mount specs, cwd-targeted config edits) resolves across launch
  cwd, workspace root, and child cwd, along with containment policy for
  outside-workspace paths, is deferred to a dedicated RFD (and intersects issue
  #144).
  This RFD establishes only two rules: child processes run with the workspace
  root as their cwd, and user-typed relative path arguments resolve against the
  launch cwd.
- **Cross-machine synchronization.**

## Risks and Open Questions

- A workspace ID with no live checkouts (all worktrees deleted) cannot be
  selected by `jp -w <id>` or `jp w use`.
  Returning an error that asks the user to run JP from an existing checkout
  should be sufficient.
- `<root-key>` derivation must be deterministic across runs and
  collision-resistant across distinct canonical paths; hashing the canonical
  path satisfies both requirements.

## Implementation Plan

### Phase 1: Roots registry and `-w <id>` resolution

Replace the `storage` symlink with the per-root registry directory and resolve
`-w <id>` against it (using the sole live root, prompting or failing when
multiple roots exist, and failing when none exist).
Derive root liveness by verifying that the checkout's workspace ID matches
`<id>`.
Seed the registry from any legacy `storage` symlink on first bootstrap (see
[Migration from the `storage` symlink](#migration-from-the-storage-symlink)).

Depends on: [RFD 031] Phase 1 *implemented* (not merely accepted).
This phase only adds ID-to-root resolution and modifies no session state, so it
can merge independently of the session layer.

### Phase 2: Startup boundary and execution context

Move session resolution ahead of workspace construction and add the `jp_cli`
bootstrap step that selects the checkout root.
Set child cwd to the workspace root for from-anywhere runs, and add the
per-command workspace bootstrap requirement (`none`, `subject`, `load`)
analogous to `conversation_load_request`.

Depends on: Phase 1.

### Phase 3: User-global session store and `jp w` surface

Add the user-global session store (mirroring [RFD 020]'s history layout), the
`jp w` subcommands (`use ?`, `use cwd`, `ls`, `show`) with the shared
`WorkspaceTarget` grammar, from-anywhere `jp q` resolution, and source-specific
session cleanup.

Depends on: Phase 2.

### Phase 4: Precedence ladder, conflict prompt, and reprompt

Implement the interactive precedence ladder, the cwd-vs-active conflict prompt,
the persisted `sticky` (`A`) state, non-interactive workspace resolution, and
reprompting when the active checkout root is missing.

Depends on: Phase 3.

## References

- [RFD 020]: Parallel Conversations, which defines the session identity and
  history model extended by this RFD.
- [RFD 031]: Durable Conversation Storage with Workspace Projection, which
  defines the shared user-local store and directory layout required by this RFD.
- [RFD 065]: Typed Resource Model for Attachments, whose snapshot-at-attach
  model addresses the attachment-portability limitation noted in
  [Non-Goals](#non-goals).
- [RFD 049]: Non-Interactive Mode and Detached Prompt Policy, which defines the
  canonical promptability check used by this RFD.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
[RFD 112]: 112-hierarchy-targets-and-explicit-selection-clearing.md
[RFD 113]: 113-context-decomposition-for-invocation-and-workspace-scope.md
[RFD 114]: 114-plugin-workspace-scope-and-addressing.md
[\#793]: https://github.com/dcdpr/jp/issues/793
