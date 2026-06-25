# RFD 087: Session-Scoped Active Workspace

- **Status**: Accepted
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-01
- **Extends**: [RFD 020]
- **Requires**: [RFD 031]
- **Tracking Issue**: [#793](https://github.com/dcdpr/jp/issues/793)

## Summary

Each terminal session can select an active workspace with `jp w use`, after
which `jp q` runs against it from anywhere without `--workspace`, the same way
[RFD 020] gives each session an active conversation.
This also decides which checkout a command acts on when [RFD 031] maps one
workspace ID to several git worktrees.

## Motivation

[RFD 031] makes every git worktree of a repository share one user-local store
keyed by workspace ID, which fixes durability and the directory-collision crash.
It leaves one question unanswered: when a workspace ID resolves to several
checkouts on disk, which one does a command act on?

Today there is no good answer.
`--workspace=<id>` is ambiguous across checkouts, and nothing targets a
workspace from outside its directory at all, so running `jp` always means first
`cd`-ing into the right tree.
Do nothing and that stays true: worktree users keep navigating by hand, and the
multi-checkout case has no defined behavior.

[RFD 020] already solved the same shape of problem for conversations: each tab
tracks its own active conversation, and the feature is widely relied on.
Applying that model one level up gives each tab an active workspace, so `jp q`
works from anywhere and the multi-checkout ambiguity becomes an explicit,
per-session choice.

## Design

### Workflow

A fresh terminal has no active workspace, so `jp` behaves exactly as it does
today: it operates on the workspace you are standing in.

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

The choice is scoped to this tab, exactly like the active conversation in [RFD
020]: another tab can select a different workspace, and the two never interfere.
`jp w show` reports the current selection and `jp w use cwd` drops it, falling
back to cwd resolution.

The rest of this section describes how that resolution works.

### Two-layer session model

- Session identity is reused unchanged from [RFD 020] (workspace-independent:
  `$JP_SESSION`, `getsid(0)` / console HWND, per-pane terminal vars).

- New **global layer**: session to active workspace, stored in a user-global
  session store at `~/.local/share/jp/sessions/<source-key>.json`, above any
  `<id>`.
  This store mirrors [RFD 020]'s session mapping shape: a most-recent-first
  `history` of selected workspaces (each entry records the workspace `<id>`, its
  resolved checkout root, and a timestamp), plus a session-level `sticky` flag.
  The active workspace is `history[0]`; the previous one is `history[1]` (the
  `session` / `s` target — see [The `jp workspace` command
  surface](#the-jp-workspace-command-surface)).
  Recording the `<id>` (not just the root) is what makes recovery work after the
  root is deleted (see [Reprompt on a missing active
  workspace](#reprompt-on-a-missing-active-workspace)).
  The filename encodes the full session *source*, not just its value:

  ```text
  getsid-<pid>.json
  hwnd-<handle>.json
  env-<KEY>-<hash(value)>.json   # e.g. env-JP_SESSION-9f86d0….json
  ```

  An automatic `getsid` / `Hwnd` session can never alias an `Env` session that
  shares the same numeric value, and two different env vars (`$JP_SESSION`,
  `$TMUX_PANE`) holding the same value get distinct files.
  Hashing the opaque env value also keeps unsafe characters out of the filename.
  This is the same source-encoding scheme [RFD 020]'s per-workspace session
  store now uses (`Session::storage_key`); this RFD reuses that one encoder
  rather than defining a second so the two stores stay consistent.
  The blast radius of a collision at this layer is which workspace a command
  runs against, so the keys are kept disjoint by construction.
  Two tabs that deliberately set the same `$JP_SESSION` still share a record
  (same source, same key, same value), which stays the supported way to link
  sessions.

- Existing **per-workspace layer** is unchanged: session to active conversation
  at `<id>/sessions/<session-key>.json`.

- Composition for `jp q` from anywhere: resolve session to active workspace
  (global), enter it (see Execution context below), resolve session to active
  conversation (per-workspace), run.

### Startup ordering

Selecting a workspace by ID happens *before* a `Workspace` exists, which inverts
today's startup: `run_inner` loads the workspace first, then resolves session
identity.
A dedicated bootstrap step in `jp_cli` owns the pre-workspace resolution:

1. Resolve session identity (`session::resolve`).
2. Inspect cwd for a workspace, if any.
3. Read the user-global session store for this session's active workspace.
4. Read the per-workspace roots registries under the user data directory.
5. Choose a concrete checkout root (see
   [Precedence](#precedence-and-the-cwd-vs-active-conflict)).
6. Only then construct `Workspace`, load config, load the conversation index,
   and run the command.

User-data scanning and root selection stay in this `jp_cli` step.
`jp_workspace::Workspace` keeps managing an already-selected workspace and gains
no awareness of how the root was chosen.

The bootstrap step resolves the selection once — launch cwd, selected workspace
root, child cwd, resolution source, and the session (if any) — and passes those
values explicitly to its consumers (workspace construction, config loading, MCP
and plugin spawns, local tools, and user-typed path parsing).
They are not re-derived from the process cwd at each call site, which is what
keeps the launch-cwd / root / child-cwd distinction from collapsing again.

### Workspace bootstrap requirement

Not every command needs a workspace selected, so each command declares its
requirement — the workspace-level analog of today's per-command
`conversation_load_request` (`jp_cli::cmd`).
The bootstrap step reads this declaration and only runs the resolution ladder
(steps 5–6 above) when the command asks for it:

- **none** — no workspace is bootstrapped.
  `jp w ls` reads the user-global registries only; `jp w use cwd` just clears
  the session record; `jp init` is unchanged.
- **resolve** — resolve and validate a target root to record a selection,
  without loading the conversation index.
  `jp w use ?` and `jp w use <id>` need the root, not the conversation data.
- **load** — resolve, construct `Workspace`, and load the conversation index.
  `jp q`, `jp w show`, and most existing commands.

The bootstrap handoff therefore has a *no workspace selected* form.
For `none` commands — and for `resolve` / `load` commands that legitimately
resolve to no workspace, such as `jp w show` from outside any workspace with
nothing active — the downstream consumers that assume a root (config loading,
MCP / plugin child cwd, path parsing) simply do not run.
This makes "absence of a selected workspace" a first-class bootstrap outcome
rather than something each command has to fake.

### Bootstrap storage ownership

The two new stores are owned by this `jp_cli` bootstrap step, not by
`Workspace`.
Today `Workspace::cleanup_stale_files` requires an `FsStorageBackend` bound to
the selected workspace, only touches that workspace's lock files and
conversation session mappings, and runs at the *end* of a command once a
workspace exists.
That owner cannot reach a user-global record before any workspace is selected,
and it never sees roots for workspace IDs that were not selected this run, so it
cannot own the new stores.

Ownership therefore splits:

- **Workspace cleanup (unchanged):** conversation locks and per-workspace
  conversation session mappings, for the selected workspace.
- **Bootstrap cleanup (new):** the user-global active-workspace session records
  and roots-registry pruning.
  It runs before workspace selection (read the active workspace, validate the
  candidate roots, prune dead ones) and after the command (upsert the current
  root, drop the global session record when its source is dead).

Pruning dead roots is opportunistic: the bootstrap prunes the roots it inspects
while resolving a selection, rather than scanning every known workspace on every
run.

### Execution context: launch cwd, workspace root, and child cwd

A from-anywhere run has to keep three directories distinct, which coincide today
only because JP normally runs from inside the workspace it operates on:

- **launch cwd** — where the user invoked `jp`; the shell completed any
  relative path argument against this.
- **workspace root** — the selected checkout root.
- **child cwd** — the working directory spawned MCP servers, plugins, and local
  tools inherit.

Whenever JP operates on a workspace whose root is not the launch cwd's own
workspace — a session-active pick, an explicit `-w <path>` or `-w <id>`, or the
fallback picker — the **child cwd** becomes the selected workspace root: config
loading, MCP servers, plugins, and local tools all run as if launched from
there.
Today these use inconsistent bases (config and MCP/plugin spawns inherit the
process cwd; local tools use `workspace.root()`), so without this invariant a
from-anywhere run would mix contexts.

**User-typed relative path arguments resolve against the launch cwd, not the
workspace root.** A user standing in `~/scratch` who types `jp q --attach
./foo.txt` had the shell complete `./foo.txt` against `~/scratch`; silently
re-rooting it at the workspace would surprise the user and, once shipped, become
a contract nobody chose.

When JP is launched from *inside* a workspace, the child cwd is left unchanged,
so a subdirectory's `.jp.toml` chain still loads as it does today.

**Minimum path behavior for this RFD.** Resolving against the launch cwd does
not relax the existing workspace-containment rule: a user-typed relative path
that resolves outside the selected workspace errors exactly as it does today.
This RFD adds no new handling — mounting, snapshotting, or external `file://`
resources — for outside-workspace paths; that is left to the deferred path RFD.
The from-anywhere flows this RFD targets (`jp q "…"`, attachments that live
inside the selected workspace) are unaffected; only attaching a path outside the
selected workspace is deferred, and it fails closed.

The full per-input-class resolution model — exactly how every relative path,
mount spec, and cwd-targeted config edit resolves, plus the containment policy
for paths outside the workspace — is broader than this RFD and is deferred (see
[Non-Goals](#non-goals)).

### Roots registry (one workspace ID, many checkouts)

The single `storage` symlink is replaced by a roots registry that maps one
workspace ID to its checkouts on disk.
A shared read-modify-write file would have a lost-update race that can silently
drop a checkout from the set, so the registry is instead a *directory of
per-root files*, one per checkout, mirroring how sessions and locks already
work:

```text
~/.local/share/jp/workspace/<id>/roots/<root-key>.json
```

- `<root-key>` is a stable hash of the checkout's canonical path, so each
  checkout owns exactly one file and writes never contend.
- Each run upserts only its own file, recording the canonical path and a
  `last_used` timestamp.
  No file is read-modified-written by more than one checkout.
- Liveness is **derived**, not stored: a root is live when JP workspace
  discovery from that path resolves a workspace whose loaded ID equals `<id>`
  (in today's colocated layout, that means `<root>/.jp/.id`).
  A path that was deleted, or recreated as a different workspace, is not live,
  and its file is pruned by the bootstrap cleanup (see [Bootstrap storage
  ownership](#bootstrap-storage-ownership)).
- These are plain files, so the Windows symlink-privilege requirement that the
  old `storage` symlink imposed does not arise.

<!-- end list -->

```json
// ~/.local/share/jp/workspace/<id>/roots/<root-key>.json
{ "path": "/Users/jean/Projects/jp.git/my-feature", "last_used": "2026-06-01T18:25:00Z" }
```

The roots registry is workspace-scoped, living under the workspace's user-local
silo directory (`<slug>-<id>`, located by ID suffix per [RFD 031]; the `<id>`
shorthand in the paths above stands for that silo).
The session store is user-global (under `sessions/`, mapping a session to its
active workspace).
These are deliberately separate, not one store doing two jobs.

### Migration from the `storage` symlink

Today the user-local back-pointer to a checkout is a single `storage` symlink
(`<id>/storage` → the checkout's `.jp`), and that symlink is how `-w <id>`
resolves a root today.
[RFD 031] Phase 1 renames the user-local directory to `<id>` but does not touch
this symlink, so RFD 087 owns its transition to the roots registry.

Migration is best-effort, not load-bearing.
On first bootstrap for an `<id>`, if a legacy `storage` symlink exists, JP
canonicalizes its target, verifies it still resolves to a workspace whose loaded
ID equals `<id>`, and seeds `roots/<root-key>.json` from it; the symlink is then
left in place (ignored) or removed.
A dead or mismatched target is pruned.
Beyond this one-time seed, roots are discovered organically — each checkout
upserts its own file on every run — so a user who never re-enters a given
checkout only loses `-w <id>` access to *that* checkout until they next run from
it.

This migration depends on [RFD 031] Phase 1 being *implemented*, not merely
accepted (see [Implementation Plan](#implementation-plan)).

### The `jp workspace` command surface

`jp workspace` is the canonical command; `jp w` is the visible short alias,
mirroring `jp conversation` / `jp c`.
Examples below use `jp w` for brevity.

- `jp w use <target>`: select the session's active workspace, using the
  targeting grammar below.
  `jp w use ?` opens the picker (list known workspaces, expand each through the
  roots registry to its live checkouts, pick one).
  `jp w use` is interactive-only in all forms — including `cwd` — because it
  mutates session state; scripts target with `jp -w` instead (see
  [Non-interactive mode](#precedence-and-the-cwd-vs-active-conflict)).
- `jp w use cwd` (short `.`): drop the session's active workspace and fall back
  to cwd resolution.
  This replaces a `--clear` flag — clearing is just selecting the cwd-derived
  workspace.
- `jp w ls`: list known workspaces and their checkouts, mirroring `jp c ls`.
- `jp w show [<target>]`: with no target, report the session's active workspace;
  with `<target>` (e.g.
  `jp w show <id>`), report that workspace — mirroring `jp c show`.
  The readout covers how it was resolved, whether the session is **sticky** to
  it, whether cwd is overriding it, the conversation count, and the active
  conversation (if any).
  The active conversation is per-`<id>` (the session mapping), shown once.
  The conversation count is the union by conversation ID across the user-local
  durable store and every live root's workspace `.jp/conversations/`, so it
  includes external (`ext`) conversations that live only in one checkout (see
  [RFD 031]).
  `jp w show <id>` therefore loads the conversation index (index only, not event
  contents) for each live root and deduplicates by ID — the one place `show`
  does a multi-root read, chosen so the count is accurate rather than cheap.
  When the target resolves to a single concrete root — a path, or an `<id>`
  with one live root — the readout shows that root.
  When an `<id>` has several live roots, `jp w show` lists every live root and
  marks the session-active one (if any); it does not prompt, so `show` stays
  read-only and script-friendly.
- `jp -w <target>`: a per-command workspace override using the same targeting
  grammar.
  It selects the workspace for this invocation only — it does not change the
  session's active workspace.
  A bare `<target>` is treated as a path if it resolves to an existing path,
  otherwise parsed as a workspace ID, so a local directory whose name matches a
  workspace ID shadows the ID.
  When an ID has one live root, JP uses it; when it has several, interactive
  runs prompt and non-interactive runs fail with the candidate roots listed.

#### Workspace targeting grammar

`jp w use` and `jp -w` share a `WorkspaceTarget` grammar modeled on
`ConversationTarget` (`jp_cli::cmd::target`), reusing the same keywords and
single-letter aliases for the concepts that carry over:

| Target           | Meaning                                                                                                             |
| ---------------- | ------------------------------------------------------------------------------------------------------------------- |
| `<id>`           | a literal workspace ID                                                                                              |
| free text        | fuzzy-match known workspaces by slug / path / ID → picker (slug is cosmetic)                                        |
| `?`              | pick from all known workspaces                                                                                      |
| `?s`, `?session` | pick from this session's workspace history                                                                          |
| `s`, `session`   | the session's previously active workspace (like `cd -`)                                                             |
| `l`, `latest`    | the live root with the newest `last_used` across the roots registry (global recency, distinct from `s` / `session`) |
| `cwd`, `.`       | the cwd-derived workspace; as a `use` target, clears the session selection                                          |
| `-`              | read a workspace ID from stdin (for `jp -w` in non-interactive use)                                                 |
| `help`           | print keyword help and exit                                                                                         |

Keywords with no workspace meaning are intentionally omitted: `newest` / `n`
(workspaces have no creation timeline), the `pinned` / `p` family (workspaces
have no listing-pin — see [Relationship to `jp
conversation`](#relationship-to-jp-conversation)), and the archive keywords.
The multi-target keywords (`+session`, `+pinned`, `+archived`) are omitted
because both `jp w use` and `jp -w` select exactly one workspace; they can be
added later under the same grammar if a multi-workspace command appears.

Session-history targets (`s`, `?s`) operate on concrete history entries —
workspace ID *plus* checkout root — not on the workspace ID alone.
`s` restores the exact previously active checkout (`cd -` semantics), not the
previous workspace re-resolved against its roots; multiple roots of the same ID
are distinct history entries.

The picker and fuzzy free-text match display each workspace by its **slug** —
the `<slug>` in the user-local silo directory `<slug>-<id>` (see [RFD 031]), the
workspace directory name captured when the silo was first created.
The slug is cosmetic: it may be absent (a bare `<id>` silo, in which case the
`<id>` is shown), is never renamed, and is not unique across workspaces.
Fuzzy free-text matches over slug, path, and ID, but resolution is always by ID
and concrete root — never by slug — so a shared or stale slug affects only
display and search, never which workspace a command runs against.

### Precedence and the cwd-vs-active conflict

Interactive ladder, in order:

1. Explicit `-w` wins.
2. Else, if the session is **sticky** to its active workspace (`A`, below) and
   that workspace is still live, use it — even when cwd resolves to a different
   workspace.
3. Else, if a session-active workspace is set and cwd resolves a *different*
   workspace (or a different checkout of the same ID), prompt.
4. Else cwd wins when present.
5. Else use the session-active workspace when live.
6. Else picker.

The conflict prompt fires on any difference (different workspace ID *or* a
different checkout of the same ID):

```text
How to proceed? [c/C/a/A/q]
c - use current workspace
C - use current workspace and make it session-active
a - use active workspace
A - use active workspace and keep the session sticky to it
q - quit without running command
```

`A` persists on the session record and makes the session **sticky** to the
active workspace.
It is interactive-only state, cleared with `jp w use cwd` (see [Session store
and cleanup](#session-store-and-cleanup)).

**Non-interactive mode ignores the session-active workspace entirely.** Here
"non-interactive" means JP cannot prompt the user, determined by the same
promptability signal JP already uses elsewhere (plugin install, lock-timeout
handling, tool-call prompts).
This RFD does not pin that signal to a specific mechanism: [RFD 049] is the
eventual canonical definition (controlling-terminal availability rather than
stdout being a TTY), and RFD 087 inherits whatever the shared signal resolves to
as it evolves.
Non-interactively, a workspace-consuming command (bootstrap `load` or `resolve`)
runs from inside a workspace or with an explicit `-w`, and errors otherwise.
The explicit `-w` accepts only concrete targets — a workspace `<id>`, a path,
`cwd` / `.` (resolve from the invocation directory), or `-` (read an ID from
stdin).
Session-derived targets (`s` / `session`, `?s` / `?session`) and pickers (`?`,
fuzzy free-text) are interactive-only: they resolve against hidden per-session
state or need a prompt, so they error non-interactively.
The read-only `jp w ls` and `jp w show` run non-interactively unchanged, since
they report registry and resolved/stored state without prompting.
`jp w use` is interactive-only in every form: it mutates session state, and a
script returns to cwd behavior by not setting `$JP_SESSION` rather than by
running `jp w use cwd`.
This keeps scripts deterministic: they never depend on hidden per-session state.

### No session identity

A session-active workspace is per-session state, so it needs a session identity
(`$JP_SESSION`, `getsid` / `Hwnd`, or a per-tab terminal var; see [RFD 020]).
Having no session identity is distinct from having no active workspace:

- `jp w use` without a session identity errors, mirroring `jp c use` ("No
  session identity available.
  Set `$JP_SESSION` or run in a terminal with automatic session detection.").
  There is nothing to persist the selection against.
- `jp q` launched from outside a workspace without a session identity errors
  with guidance to pass `-w`.
  It does not fall back to a non-persisted one-shot picker: a choice that cannot
  be recorded would have to be re-made on every invocation, and [RFD 020]
  already establishes that mappings are not persisted without a session
  identity.
  Scripts stay deterministic.

### Reprompt on a missing active workspace

- If the recorded root no longer exists (worktree removed), recovery uses the
  `workspace_id` stored alongside it: read the `<id>`, expand
  `workspace/<id>/roots/` to its live checkouts, prune dead ones, and re-prompt
  among the remainder (one live root may be used directly).
  This is why the session record stores the `<id>` and not only the root — once
  the root is gone, the `<id>` cannot be recovered from `<root>/.jp/.id`.
  It mirrors how `session_active_conversation` returns `None` and falls back to
  the picker when the active conversation is gone.

### Session store and cleanup

The global session store mirrors [RFD 020]'s session mapping: a
most-recent-first `history` of selected workspaces (active = `history[0]`,
previous = `history[1]`), plus a session-level `sticky` flag and the session
`source`.
It is longer-lived than the per-workspace conversation mapping, so cleanup
splits by session source:

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

The `sticky` field is the persisted `A` state from the precedence ladder.

- **`getsid` / `Hwnd`**: reuse RFD 020's process-liveness check.
  The mapping is removed when the originating process is confirmed dead.
- **`Env` (including `$JP_SESSION`)**: process liveness is unknown, so cleanup
  is existence-based across the whole history, mirroring RFD 020's `Env` rule
  (which keeps a mapping while *any* referenced conversation still exists).
  A history entry is pruned only when its `workspace_id` has no live root; the
  whole mapping is removed only when no history entry references a workspace ID
  with any live root.
  Keying cleanup off the workspace ID rather than the single active root is what
  lets the missing-root recovery flow run: when the active root is gone but its
  `<id>` still has other live checkouts, the mapping survives so recovery can
  re-prompt among them (see [Reprompt on a missing active
  workspace](#reprompt-on-a-missing-active-workspace)).
- A sticky `A` choice has no process bound for `Env` sources, so it persists
  until its `workspace_id` has no live root — not merely until the active root
  dies.
  If the active root is removed while a sibling checkout survives, recovery
  re-prompts among the remaining roots rather than dropping the sticky
  selection.

### Relationship to `jp conversation`

`jp workspace` deliberately mirrors `jp conversation` so the two share a mental
model.
The session store reuses [RFD 020]'s mapping shape (a most-recent-first history,
active = `history[0]`); session identity, the `getsid` / `Hwnd` / `Env` source
split, and the stale-cleanup rules are the same machinery; `jp w use` / `ls` /
`show` mirror `jp c use` / `ls` / `show`; and the targeting grammar reuses
`ConversationTarget`'s keywords and single-letter aliases for the concepts that
carry over (`?`, `?s`, `s` / `session`, `l` / `latest`, `-`, `help`).

Two things genuinely diverge, by necessity:

- **The cwd-vs-active conflict and the `sticky` flag have no conversation
  analog.** A conversation has no ambient, location-derived candidate competing
  with the session-active one; a workspace does (the tree you are standing in).
  The conflict prompt, the precedence ladder, and the `sticky` pin exist only
  because workspaces are filesystem-rooted.
  **Sticky** here means "this session keeps using its active workspace even when
  cwd points elsewhere" — distinct from a **Pinned Conversation**, which marks
  a conversation as important in listings.
  They are different concepts and use different words on purpose.
- **`cwd` is a workspace-only target.** `jp w use cwd` clears the session
  selection and returns to cwd resolution; conversations have no cwd fallback,
  so the keyword (and its clearing behavior) is workspace-specific.

Conversely, the multi-target keywords (`+session`, `+pinned`, `+archived`) and
the listing-pin / archive keywords from `jp c` are absent here because the
underlying concepts do not exist for workspaces; they are not a divergence to
reconcile.

## Drawbacks

- A new user-global session store and its cleanup pass.
  It reuses RFD 020's process-liveness check for `getsid` / `Hwnd` sources but
  needs a distinct rule for `Env` sources (see [Session store and
  cleanup](#session-store-and-cleanup)).
- Cold-start double prompt: a fresh session run from nowhere prompts for a
  workspace, then a conversation.
  Acceptable for v1; optimization deferred.
- The conflict prompt adds a decision point, though only for users who have set
  a session-active workspace.

## Alternatives

- **Always cwd-wins, no session-active workspace.** Rejected: never lets you run
  `jp` from outside a workspace directory, the core goal.
- **Always session-active-wins over cwd.** Rejected: silently runs against the
  wrong place when you `cd` elsewhere.
- **A single global active workspace, not per-session.** Rejected: breaks
  parallel tabs, the property [RFD 020] users rely on.
- **Multi-target symlink for the back-pointer.** Not a filesystem primitive; the
  registry file models one-to-many natively.

## Non-Goals

- **Git awareness.** Consistent with [RFD 031], JP does not inspect worktree
  topology.
- **Attachment portability across checkouts.** Continuing a conversation in a
  different checkout can break path-relative attachments.
  Once [RFD 065]'s snapshot-at-attach model lands it will capture attachment
  content at attach time and stop re-resolving it on later runs, which would
  remove most of this risk.
  Until then, continuing a conversation from a different checkout can re-resolve
  path-relative attachments against the selected checkout.
  Either way, the precedence ladder and conflict prompt above are a guardrail,
  not a guarantee; [RFD 065] is not a prerequisite for this RFD.
- **Re-embedding the workspace ID in conversation IDs.** That regression fix is
  a separate, smaller change.
  It is complementary (visible IDs make `jp w` and `jp -w` discoverable) but not
  required for this design to function.
- **A global-local workspace for use outside any project.** Issue \#144 asks for
  JP to work anywhere by falling back to a "global local" workspace when the
  user is outside every project workspace.
  This RFD does not implement that — it only lets commands launched outside a
  workspace target an *existing* workspace selected by session state, the
  picker, or `-w`.
- **A unified path-resolution model.** Defining, per input class, how every
  relative path argument, mount spec, and cwd-targeted config edit resolves
  across launch cwd / workspace root / child cwd is broader than this RFD and
  intersects the "use JP outside a workspace" work (issue \#144).
  This RFD commits only to two rules: child processes run with the workspace
  root as their cwd, and user-typed relative path arguments resolve against the
  launch cwd.
  The rest belongs in a dedicated RFD.
- **Cross-machine sync.**

## Risks and Open Questions

- A workspace ID with no live checkouts (every worktree removed) cannot be
  entered by `jp -w <id>` or `jp w use`.
  The intended behavior is an error pointing the user at a checkout; confirm
  that is sufficient.
- Root-key derivation must be stable across runs and collision-resistant for
  distinct canonical paths.
  A hash of the canonical path is the intended approach.

## Implementation Plan

### Phase 1: Roots registry and `-w <id>` resolution

Replace the `storage` symlink with the per-root registry directory and resolve
`-w <id>` against it: one live root, use it; many, picker/error; none, error.
Derive liveness via the same-ID check.
Seed the registry from any legacy `storage` symlink on first bootstrap (see
[Migration from the `storage` symlink](#migration-from-the-storage-symlink)).

Depends on: [RFD 031] Phase 1 *implemented* (not merely accepted).
This phase only adds ID-to-root resolution and changes no session state, so it
can be merged independently of the session layer.

### Phase 2: Startup boundary and execution context

Move session resolution ahead of workspace construction and add the `jp_cli`
bootstrap step that selects the root.
Establish the root-as-working-directory invariant for from-anywhere runs.
Add the per-command workspace bootstrap requirement (none / resolve / load), the
analog of `conversation_load_request`.

Depends on: Phase 1.

### Phase 3: User-global session store and `jp w` surface

Add the user-global session store (history-shaped, mirroring [RFD 020]) and the
`jp w` commands (`use ?`, `use cwd`, `ls`, `show`) with the shared workspace
targeting grammar, plus `jp q`-from-anywhere resolution and the source-split
cleanup rules.

Depends on: Phase 2.

### Phase 4: Precedence ladder, conflict prompt, and reprompt

Implement the interactive ladder, the cwd-vs-active prompt, the persisted `A`
sticky state, the non-interactive rule, and reprompt-on-missing-root.

Depends on: Phase 3.

## References

- [RFD 020]: Parallel Conversations, the session identity and history model this
  RFD extends.
- [RFD 031]: Durable Conversation Storage with Workspace Projection, the shared
  user-local store and migration this RFD requires.
- [RFD 065]: Typed Resource Model for Attachments, whose snapshot-at-attach
  model bounds the attachment-portability concern noted in Non-Goals.
- [RFD 049]: Non-Interactive Mode and Detached Prompt Policy, the eventual
  canonical definition of "interactive" this RFD's local rule defers to.

[RFD 020]: 020-parallel-conversations.md
[RFD 031]: 031-durable-conversation-storage-with-workspace-projection.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 065]: 065-typed-resource-model-for-attachments.md
