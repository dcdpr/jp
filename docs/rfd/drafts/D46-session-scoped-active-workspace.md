# RFD D46: Session-Scoped Active Workspace

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-06-01
- **Extends**: [RFD 020]
- **Requires**: [RFD 031]
- **Required by**: [RFD D49](D49-conversation-export-and-import.md)

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
`jp w show` reports the current selection and `jp w use --clear` drops it.

The rest of this section describes how that resolution works.

### Two-layer session model

- Session identity is reused unchanged from [RFD 020] (workspace-independent:
  `$JP_SESSION`, `getsid(0)` / console HWND, per-pane terminal vars).
- New **global layer**: session to active workspace (a concrete checkout root),
  stored in a user-global session store at
  `~/.local/share/jp/sessions/<session-key>.json`, above any `<id>`.
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

### Execution context: the workspace root is the working directory

When JP resolves a workspace via the session-active pick (the command was
launched from outside any workspace), it operates as if launched from the
workspace root: the selected root becomes the process working directory before
config loading, MCP servers, plugins, and local tools run.
Today these use inconsistent bases (config and MCP/plugin spawns inherit the
process cwd; local tools and attachments use `workspace.root()`), so without
this invariant a from-anywhere run would mix contexts.

When JP is launched from *inside* a workspace, the working directory is left
unchanged, so a subdirectory's `.jp.toml` chain still loads as it does today.

Accepted trade-off: under a from-anywhere run, relative paths such as `jp q
--attach ./foo.txt` and `jp config set --cwd` resolve against the workspace
root, not the launch directory.
Revisiting that is future work.

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
- Liveness is **derived**, not stored: a root is live when its path still
  resolves to a workspace whose `.jp/.id` equals `<id>`.
  A path that was deleted, or recreated as a different workspace, is not live,
  and its file is pruned during the existing cleanup pass.
- These are plain files, so the Windows symlink-privilege requirement that the
  old `storage` symlink imposed does not arise.

<!-- end list -->

```json
// ~/.local/share/jp/workspace/<id>/roots/<root-key>.json
{ "path": "/Users/jean/Projects/jp.git/my-feature", "last_used": "2026-06-01T18:25:00Z" }
```

The roots registry is workspace-scoped (under `<id>/`).
The session store is user-global (under `sessions/`, mapping a session to its
active workspace).
These are deliberately separate, not one store doing two jobs.

### The `jp w` command surface

- `jp w use ?`: list known workspaces (`<id>` dirs), expand each through the
  roots registry to its live checkouts, pick one, record it as the session's
  active workspace.
- `jp w use --clear`: drop this session's active workspace, falling back to cwd
  resolution.
- `jp w ls`: list known workspaces and their checkouts, mirroring `jp c ls`.
- `jp w show`: show the session's active workspace, how it was resolved, and
  whether cwd is overriding it, mirroring `jp c show`.
- `jp -w <id>`: one live root, use it; many, picker (interactive) or an error
  listing the roots (non-interactive).
  Pure addressing; mirrors `jp q` with no active conversation.
  `-w` targets a single command and does not change the session's active
  workspace.

### Precedence and the cwd-vs-active conflict

Interactive ladder: explicit `-w` wins; else if a session-active workspace is
set and cwd resolves a *different* workspace, prompt; else cwd wins when
present; else use session-active; else picker.

The conflict prompt fires on any difference (different workspace ID *or* a
different checkout of the same ID):

```text
How to proceed? [c/C/a/A/q]
c - use current workspace
C - use current workspace and make it session-active
a - use active workspace
A - use active workspace and don't ask again in this session
q - quit without running command
```

`A` persists on the session record and pins the session to the active workspace.
It is interactive-only state, cleared with `jp w use --clear` (see [Session
store and cleanup](#session-store-and-cleanup)).

**Non-interactive mode ignores the session-active workspace entirely.** A
non-interactive command runs from inside a workspace or with an explicit
`--workspace`, and errors otherwise.
`jp w use` is itself an error in non-interactive mode.
This keeps scripts deterministic: they never depend on hidden per-session state.

### Reprompt on a missing active workspace

- If the recorded root no longer exists (worktree removed), re-prompt among the
  remaining roots, mirroring how `session_active_conversation` returns `None`
  and falls back to the picker when the active conversation is gone.

### Session store and cleanup

The global session store maps a session to an active workspace root, which is
longer-lived than [RFD 020]'s per-workspace mapping (a conversation history), so
cleanup splits by session source:

- **`getsid` / `Hwnd`**: reuse RFD 020's process-liveness check.
  The mapping is removed when the originating process is confirmed dead.
- **`Env` (including `$JP_SESSION`)**: process liveness is unknown, and RFD
  020's "are the referenced conversations gone" fallback does not apply because
  the mapping points at a workspace, not conversations.
  The mapping is removed when its active root no longer resolves to a live
  workspace, the same liveness check the roots registry uses.
- A pinned `A` choice has no process bound for `Env` sources, so it persists
  until the root dies or the user runs `jp w use --clear`.

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
  [RFD 065]'s snapshot model captures attachment content at attach time and does
  not re-resolve it on later runs, which removes most of this risk; what remains
  (resolving a *new* `--attach` against the current checkout) is bounded by the
  precedence ladder and conflict prompt above, which are a guardrail, not a
  guarantee.
- **Re-embedding the workspace ID in conversation IDs.** That regression fix is
  a separate, smaller change.
  It is complementary (visible IDs make `jp w` and `jp -w` discoverable) but not
  required for this design to function.
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

Depends on: [RFD 031] Phase 1.
Pure addressing; can be merged independently of the session layer.

### Phase 2: Startup boundary and execution context

Move session resolution ahead of workspace construction and add the `jp_cli`
bootstrap step that selects the root.
Establish the root-as-working-directory invariant for from-anywhere runs.

Depends on: Phase 1.

### Phase 3: User-global session store and `jp w` surface

Add the user-global session store and the `jp w` commands (`use ?`, `use
--clear`, `ls`, `show`), plus `jp q`-from-anywhere resolution and the
source-split cleanup rules.

Depends on: Phase 2.

### Phase 4: Precedence ladder, conflict prompt, and reprompt

Implement the interactive ladder, the cwd-vs-active prompt, the persisted `A`
silence, the non-interactive rule, and reprompt-on-missing-root.

Depends on: Phase 3.

## References

- [RFD 020]: Parallel Conversations, the session identity and history model this
  RFD extends.
- [RFD 031]: Durable Conversation Storage with Workspace Projection, the shared
  user-local store and migration this RFD requires.
- [RFD 065]: Typed Resource Model for Attachments, whose snapshot-at-attach
  model bounds the attachment-portability concern noted in Non-Goals.

[RFD 020]: ../020-parallel-conversations.md
[RFD 031]: ../031-durable-conversation-storage-with-workspace-projection.md
[RFD 065]: ../065-typed-resource-model-for-attachments.md
