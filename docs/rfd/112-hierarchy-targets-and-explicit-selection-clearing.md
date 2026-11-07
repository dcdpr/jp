# RFD 112: Hierarchy Targets and Explicit Selection Clearing

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-09
- **Extends**: [RFD 087]
- **Summary**: Adds `.`, `..`, and `/` hierarchy targets for workspaces and
  conversations and replaces `jp w use cwd` with `--clear`.

## Summary

This RFD introduces `.`, `..`, `../..`, and `/` as hierarchy targets that walk
the workspace nesting chain, and replaces `jp w use cwd` with `--clear` for
dropping a session's selection.
Moving clearing out of the target grammar gives `cwd` a single meaning in every
position and gives workspaces and conversations a shared navigation vocabulary.

## Motivation

The current workspace and conversation targeting grammars have four related
problems.

**`cwd` means "select" or "clear" depending on the subcommand.** As a
`--workspace` target, `cwd` resolves the workspace containing the current
directory; as a `jp w use` target, it deletes the session record.
[RFD 087] justified this overload on the grounds that "clearing is just
selecting the cwd-derived workspace", but the two diverge as soon as you `cd`: a
recorded `cwd` selection follows the session and triggers the conflict prompt,
whereas a cleared record does not.

**`jp c use` cannot clear the active conversation.** Unlike `jp w use`, `jp c
use` has neither a flag nor a keyword to drop the session's active conversation.
Starting with a clean slate requires opening a new terminal.

**`.` has an unresolved collision in [RFD 087].** [RFD 087]'s grammar table
lists `cwd` and `.` as workspace targets, while the `[!WARNING]` block
immediately below it argues that `.` should be dropped because
`ConversationTarget` uses `.` for the session's active conversation.
The code implements the table while the RFD's warning argues against it, leaving
the specification in contradiction with itself.

**Neither grammar can navigate up a hierarchy.** Workspaces can nest, and [RFD
039] organizes conversations into a parent-child tree, yet targeting the
workspace above the current one or the conversation a thread was forked from
requires looking up an ID by hand.
Treating `.` as a workspace keyword that clashes with a conversation keyword
misidentifies it: `.` is the first segment of a hierarchy navigation vocabulary
(`.`, `..`, `../..`, and `/`) that applies to both workspaces and conversations.

## Design

### Hierarchy targets

Four targets are added to both grammars.
A target composed only of `.`, `..`, and `/` is a hierarchy target; anything
containing a named segment (`./foo`, `../foo`, `/foo/bar`) remains a filesystem
path.

| Target  | Workspace axis                    | Conversation axis                 |
| ------- | --------------------------------- | --------------------------------- |
| `.`     | the current workspace             | the active conversation           |
| `..`    | the parent workspace              | the conversation's parent         |
| `../..` | its grandparent, and so on        | its grandparent, and so on        |
| `/`     | the outermost enclosing workspace | the root of the conversation tree |

```sh
$ jp -w .. c ls              # the workspace containing this one
$ jp c show ..               # the conversation this one was forked from
$ jp -w / config show        # the outermost workspace in the nesting chain
```

These segments walk the workspace or conversation hierarchy, not the filesystem
directory tree.
`jp -w .` resolves the enclosing workspace from any of its subdirectories
because `Workspace::find_root` walks up the directory tree until it finds `.jp`.
Having `..` step up one level in the workspace hierarchy rather than one
directory up matches that behavior.

Workspace hierarchy targets resolve from the launch `cwd` rather than the
session's active workspace, matching every other explicit `--workspace` target
that bypasses the session layer.

### Resolving a parent workspace

Workspace parenthood is derived from directory containment and workspace ID
inequality using `Workspace::find_root` and the workspace ID file, with no new
stored state.
Given the current workspace root:

1. Read the current workspace ID.
2. Move up one directory and resolve a workspace root from there.
3. If that root has the *same* workspace ID, repeat from step 2.
4. The first root with a *different* workspace ID is the parent workspace.
5. Reaching the filesystem root without finding a different workspace ID means
   there is no parent workspace.

Comparing workspace IDs handles nested checkouts of the same workspace, such as
a git worktree inside its own repository (which `roots.rs` already models).
Stopping at the first enclosing root without checking its ID would mistake a
sibling checkout of the same workspace for a parent.

`../..` applies the parent walk twice, and `/` repeats the walk to the
filesystem root and takes the last workspace root found.
When no workspace encloses the current one, `/` resolves to the current
workspace and `..` returns an error.
The CLI help text notes this common case explicitly.

Parent resolution distinguishes two failure modes so each can provide specific
guidance:

- **Not inside any workspace**: points to `jp init` or `--workspace <id>`.
- **No enclosing workspace**: reports that the current workspace is already the
  outermost workspace.

Because hierarchy targets are deterministic, read no session state, and never
prompt, they join `<id>`, a path, and `-` in the set of targets [RFD 087]
permits in non-interactive runs and scripts.

### `cwd` becomes an ordinary selection

`jp w use cwd` selects and records the workspace containing the current
directory, matching `jp w use <path>`, rather than clearing the session record.
`jp -w cwd` is unchanged.

With this change, `cwd` and `.` agree in every command position instead of
splitting into one spelling that selects and another that clears.

### `--clear`

Both `jp w use --clear` and `jp c use --clear` delete the session record for
their axis.
Because clearing removes a selection rather than naming a target, it is a flag
instead of a keyword in the target grammar.
Passing `--clear` alongside any target argument is rejected because the command
would ask to both select and clear.

Clearing drops the **whole record**, not just the active entry:

```sh
$ jp w use --clear
Cleared the session's active workspace: ~/Projects/jp

$ jp c use --clear
Cleared the session's active conversation: jp-c17866928997
```

Marking a session as having "nothing active" while retaining history would
require a new state in both persisted formats (`WorkspaceSessionMapping` and
[RFD 020]'s `SessionMapping`).
Every reader of either format would have to handle that state to save a single
picker invocation, which `jp w use ?` already provides.

Deleting the entire record has two intended consequences:

- **`s` and `?s` have no session history to read.** Dropping the record deletes
  its history, so after `--clear` there is no previous workspace or conversation
  for `s` to restore and no session history for `?s` to list.
- **The two axes fall back differently.** A cleared workspace selection falls
  back to `cwd` resolution, an ambient default that always exists.
  A cleared conversation selection has no ambient default, so the next command
  prompts to pick an existing conversation or create a new one.

### `.` stays in both grammars

[RFD 087]'s warning is resolved by keeping `.` on both axes as the current-item
segment of the shared hierarchy vocabulary.
Dropping `.` from either grammar would leave `..` and `/` with no way to name
the current position they resolve relative to.

### Bare `use` is unaffected

Bare `jp w use` and bare `jp c use` still switch to the session's previously
active selection, falling back to a picker when none exists.
`--clear` is a separate flag, and `cwd` and `.` refer to the current position
rather than the previous one.

## Drawbacks

**`jp w use cwd` changes meaning.** Switching `jp w use cwd` directly from
clearing the session to recording a selection would silently break scripts that
used it to clear.
To avoid a silent behavior change, `jp w use cwd` errors for one release before
selecting the current workspace.

**`..` and `/` are unused in flat workspace layouts.** Nested workspaces are
uncommon, so most users do not have a parent workspace; for them, two thirds of
the new grammar either errors with "no parent workspace" (`..`, `../..`) or
resolves to the workspace they are already in (`/`).

**Directory containment implies parenthood even for unrelated projects.**
Deriving parenthood from directory nesting and workspace ID inequality requires
no stored state, but a user who places two unrelated workspaces in the same
directory tree gets an unintended parent-child relationship.

## Alternatives

**Drop `.` from the workspace grammar, as [RFD 087] suggests.** Rejected.
Removing `.` leaves `..` and `/` without a target for the current workspace they
resolve relative to.

**Drop `.` from both grammars and treat it only as a path.** Rejected for the
same reason.
It also breaks existing conversation targeting (`.` for the active conversation)
when separate argument types already distinguish workspace targets from
conversation targets.

**Keep `cwd` as the clearing target and omit `.`.** Rejected.
It keeps the `cwd` overload between `--workspace` and `jp w use`, and leaves `jp
c use` with no way to clear the active conversation.

**Use a `none` or `NONE` keyword instead of `--clear`.** Rejected.
[RFD 038] established uppercase `NONE` as the reset keyword for `--cfg`, where
values are usually paths and a keyword needs visual distinction, whereas target
grammars already let lowercase keywords shadow paths.
More importantly, a keyword in the target slot repeats the mistake of encoding
the absence of a selection as a target.

**Path-based `..`.** Rejected.
Because `Workspace::find_root` walks up the directory tree, resolving `..` as a
parent directory before finding a workspace returns the current workspace
whenever you run the command from a subdirectory.

## Non-Goals

- **Conversation-axis `..`, `../..`, and `/`.** This RFD defines the hierarchy
  grammar for both axes, but implements only the workspace half plus the
  existing conversation `.`.
  Conversation-tree navigation depends on the parent-child tree specified in
  [RFD 039].

- **A workspace picker that creates a workspace.** After `--clear`, running a
  command outside every workspace opens the picker; offering to initialize a new
  workspace from that picker belongs to the `init` workflow.

- **Declared workspace parenthood.** Parenthood is derived from directory
  containment.
  Allowing a config field or manifest to declare an unrelated workspace as a
  parent is out of scope.

- **Multi-target hierarchy segments.** Prefixed forms such as `+..` are not
  supported; every hierarchy target names a single item.

## Risks and Open Questions

**Interaction between `--clear` and end-of-run cleanup.** The end-of-run cleanup
pass prunes session records whose processes have exited.
Explicitly dropping a session record with `--clear` and implicitly pruning dead
records must not race within a single run.

## Implementation Plan

### Phase 1: `--clear` on both axes

Add `--clear` to `jp w use` and `jp c use` to delete the entire session record
for the respective axis, and reject `--clear` alongside a target.
Add a clear operation to the conversation session mapping.

This phase is independently mergeable and leaves `jp w use cwd` clearing the
workspace record so existing usage does not break yet.

### Phase 2: `cwd` deprecation window

Split the `.` alias off `cwd` into its own target.
On `jp w use`, `.` selects and records the current workspace (the behavior Phase
4 later gives `cwd`), while `jp w use cwd` returns an error pointing to
`--clear` to clear and `jp w use .` to select.
`jp -w cwd` and `jp -w .` are unchanged.

This phase depends on Phase 1 so `--clear` exists before the error message
points to it.

### Phase 3: Workspace hierarchy targets

Implement the parent workspace walk (using workspace ID inequality) and add `.`,
`..`, `../..`, and `/` to `WorkspaceTarget`, with the dot-and-slash-only parsing
rule and the two distinct error messages.
Add hierarchy targets to the non-interactive target set.

This phase depends on Phase 2, which gives `.` its own target on the `use` axis.
The `jp -w` hierarchy targets do not depend on Phase 2 and can land earlier if
the phases are split further.

### Phase 4: `cwd` as a selection

Update `jp w use cwd` to select and record the current workspace, matching `jp w
use .`.

This phase depends on Phase 2 having shipped in a release.

## References

- [RFD 020]: Session-to-conversation mappings, the record `jp c use --clear`
  drops.
- [RFD 038]: The `NONE` reset keyword, the nearest precedent for spelling
  "reset" in a value position.
- [RFD 039]: The conversation tree and the conversation-axis half of the
  hierarchy grammar.
- [RFD 087]: The workspace targeting grammar, the `cwd` overload, and the `.`
  warning this RFD resolves.

[RFD 020]: 020-parallel-conversations.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 039]: 039-conversation-trees.md
[RFD 087]: 087-session-scoped-active-workspace.md
