# RFD 109: Hierarchy Targets and Explicit Selection Clearing

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-09
- **Extends**: [RFD 087]

## Summary

`.`, `..`, `../..`, and `/` become hierarchy targets that walk the workspace
nesting chain, and `--clear` replaces `jp w use cwd` as the way to drop a
session's selection.
Together these free `cwd` to mean one thing in every position, and give
workspaces and conversations one navigation vocabulary over two hierarchies.

## Motivation

Two defects in the current targeting grammar share a cause.

**`cwd` means two different things depending on the subcommand.** As a
`--workspace` target it resolves the workspace you are standing in; as a `jp w
use` target it drops the session record entirely.
[RFD 087] justified the overload on the grounds that "clearing is just selecting
the cwd-derived workspace", but the two diverge the moment you `cd`: a recorded
cwd-selection follows the session and fires the conflict prompt, a cleared
record does not.

**`.` has an unresolved collision.** [RFD 087]'s grammar table lists `cwd`, `.`
for workspaces, and the `[!WARNING]` immediately below it argues that `.` should
be dropped because `ConversationTarget` spells the session's active conversation
`.`.
Neither the table nor the warning has won: the code implements the table, and
the RFD contradicts itself in print.

The warning frames the collision as two opposite meanings for one character.
That reading leads to dropping `.` from one grammar, which is the wrong
conclusion, because it misidentifies what `.` is.
`.` is not a workspace keyword that happens to clash with a conversation
keyword.
It is the first segment of a *hierarchy navigation vocabulary* that applies to
both axes — the vocabulary whose remaining segments are `..`, `../..`, and `/`.
Workspaces nest, and [RFD 039] gives conversations a parent-child tree.
Both hierarchies want the same words.

Do nothing and three things persist: a keyword that means "select" or "clear"
depending on where it appears, a published RFD arguing with its own
specification, and no way to say "the workspace above this one" or "the
conversation this one was forked from" without looking up an ID by hand.

There is also a plain capability gap.
`jp c use` has no way to clear the session's active conversation — no flag, no
keyword.
The only way to a clean slate is a new terminal.

## Design

### Hierarchy targets

Four targets are added to both grammars.
A target composed only of `.`, `..`, and `/` is a hierarchy target; anything
containing a named segment (`./foo`, `../foo`, `/foo/bar`) stays a filesystem
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

The segments walk the *hierarchy*, never the directory tree.
`jp -w .` is the workspace you are in whichever subdirectory you stand in, which
is already how `.` behaves today — `Workspace::find_root` walks up until it
finds `.jp`.
Making `..` mean "one level up the workspace hierarchy" rather than "one
directory up" removes an inconsistency rather than introducing one.

Hierarchy targets resolve from the launch cwd, not from the session's active
workspace, consistent with every other explicit `--workspace` target bypassing
the session layer.

### Resolving a parent workspace

Workspace parenthood is derived, not stored.
Given the current root:

1. Read the current workspace ID.
2. Move up one directory and resolve a root from there.
3. If that root carries the *same* ID, repeat from step 2.
4. The first root carrying a *different* ID is the parent workspace.
5. Reaching the filesystem root without finding one means there is no parent.

The ID comparison is what makes this correct.
Two checkouts of the same workspace can nest — a git worktree inside its own
repository is the common case, and `roots.rs` already models it.
Stopping at the first root found above the current one would return a sibling
checkout of the same workspace and call it a parent.

`/` runs the same walk to exhaustion and takes the last root found.
`../..` applies the walk twice.
No new state is stored anywhere: parenthood is a function of directory
containment and ID inequality, computed from `Workspace::find_root` and the ID
file, both of which exist.

Two failures are distinguished, because they need different guidance:

- Not in a workspace at all — points at `jp init` or `--workspace <id>`.
- In a workspace that has no enclosing one — says the current workspace is
  outermost.

When nothing nests, `/` resolves to the current workspace and `..` errors.
That is the common case, and the help text says so rather than implying the
segments always have somewhere to go.

Hierarchy targets are deterministic, read no session state, and never prompt, so
they join `<id>`, a path, and `-` in the set of targets [RFD 087] permits
non-interactively.
Scripts gain hierarchy navigation.

### `cwd` becomes an ordinary selection

`jp w use cwd` selects the workspace you are standing in and records it, exactly
as `jp w use <path>` does, it no longer clears anything.
`jp -w cwd` is unchanged.

`cwd` and `.` therefore agree in every position, which is the property that made
`.` inconsistent under the old meaning: two near-identical spellings, one
selecting and one clearing.

### `--clear`

Both `jp w use --clear` and `jp c use --clear` drop the session record for their
axis.
Clearing is not a target, so it is a flag rather than a word in the grammar.
Putting the absence of a selection into the slot that names selections is what
produced the `cwd` overload.

Clearing drops the **whole record**, not just the active entry:

```sh
$ jp w use --clear
Cleared the session's active workspace: ~/Projects/jp

$ jp c use --clear
Cleared the session's active conversation: jp-c17866928997
```

The alternative — marking "nothing active" while retaining history — needs a
new state in two persisted formats (`WorkspaceSessionMapping` and [RFD 020]'s
`SessionMapping`) that every reader of either must then handle, to save one
picker invocation.
The cost lands permanently on every consumer; the benefit is already available
as `jp w use ?`.

Two consequences follow and are intended:

- **`s` and `?s` go dark together.** Dropping the record takes the history with
  it, so after `--clear` there is no previous workspace to return to and no
  session history to pick from.
  That is what a clean slate means.
- **The two axes fall back to different things.** A cleared workspace selection
  returns to cwd resolution, an ambient default that always exists.
  A cleared conversation selection has no ambient equivalent, so the next
  command asks to pick a conversation or create a new one.

`--clear` with any target is rejected: the invocation would be asking to both
select and not select.

### `.` stays in both grammars

[RFD 087]'s warning is resolved by keeping `.` on both axes.
It is the `.` of a navigation vocabulary shared by two hierarchies.
Dropping it from either would leave that grammar with `..` and `/` and no way to
name the position they are relative to.

### Bare `use` is unaffected

Bare `jp w use` and bare `jp c use` return to the session's previously active
selection, falling back to a picker.
This RFD does not change that: `--clear` is a distinct request, and `cwd` / `.`
name the current position rather than the previous one.

## Drawbacks

**Two user-facing grammar changes at once.** `jp w use cwd` changes meaning
rather than erroring, which is the worst kind of breaking change — a script
that used it to clear will silently record a selection instead.
This is the strongest argument against the proposal and the reason `jp w use
cwd` should error for one release rather than switching meaning quietly.

**`..` and `/` are inert for most users.** Nested workspaces are uncommon.
Most people will never have a parent workspace, so two thirds of the new grammar
resolves to "no parent workspace" or to the workspace they are already in.

**Parenthood-by-containment is a definition, not a discovery.** Deriving it from
directory nesting plus ID inequality is cheap and needs no stored state, but it
is a choice.
A user who deliberately nests two unrelated projects gets a parent relationship
they did not ask for.

## Alternatives

**Drop `.` from the workspace grammar, as [RFD 087] suggests.** Rejected: it
resolves the collision by conceding the character, which only works while `.` is
the whole vocabulary.
Once `..` and `/` exist, the grammar that lost `.` has no way to name the
position the other segments are relative to.

**Drop `.` from both grammars and let it mean only a path.** Rejected for the
same reason, and it costs a breaking change to conversation targeting to buy
nothing the position-type separation does not already provide.

**Keep `cwd` as the clearing target and leave `.` out.** Rejected: it preserves
the overload that caused the problem, and leaves `jp c use` with no way to clear
at all.

**A `none` or `NONE` keyword instead of `--clear`.** [RFD 038] established
uppercase `NONE` as this project's reset spelling for `--cfg`.
Rejected here because that grammar takes values that are usually paths, where a
keyword needs visual separation, while target grammars already let lowercase
keywords shadow paths.
More importantly, it repeats the original mistake: a word in the target slot
that does not name a target.

**Path-based `..`.** Rejected: resolving `..` as a directory and then finding a
workspace from there returns the workspace you are already in whenever you stand
in a subdirectory, since `find_root` walks up.
It is a no-op spelling wearing the appearance of navigation.

## Non-Goals

- **Conversation-axis `..`, `../..`, and `/`.** The grammar is defined here for
  both axes, but the conversation half needs the parent-child tree.
  [RFD 039] specifies it and this RFD implements only the workspace half plus
  the existing conversation `.`.

- **A workspace picker that creates a workspace.** After `--clear`, a run from
  outside every workspace reaches the picker, and offering "create one here"
  there is an `init`-flow change and belongs with that work.

- **Declared workspace parenthood.** Parenthood is derived from containment.
  A config field or manifest declaring an unrelated workspace as a parent is a
  larger design question about what a project is.

- **Multi-target hierarchy segments.** `+..` and similar have no meaning; every
  hierarchy target names exactly one thing.

## Risks and Open Questions

**Clearing and the end-of-run cleanup.** The cleanup pass prunes session records
whose sources are dead.
Dropping a record explicitly and having it pruned implicitly must not race
within one run.

## Implementation Plan

### Phase 1: `--clear` on both axes

Add `--clear` to `jp w use` and `jp c use`, dropping the whole session record
for their axis.
Reject `--clear` alongside a target.
Add clearing to the conversation session mapping, which has no such operation
today.

Independently mergeable.
Leaves `jp w use cwd` clearing as well, so nothing breaks yet.

### Phase 2: `cwd` deprecation window

`jp w use cwd` errors, naming `--clear` to clear and `jp w use .` to select.
`jp -w cwd` is untouched.

Depends on Phase 1, so the replacement exists before the error points at it.

### Phase 3: Workspace hierarchy targets

Implement the parent walk (ID inequality) and add `.`, `..`, `../..`, and `/` to
`WorkspaceTarget`, with the only-dot-segments parse rule and the two distinct
error messages.
Add them to the non-interactive target set.

Depends on Phase 2 for `.` on the `use` axis to be unambiguous.
`jp -w` hierarchy targets do not depend on it and could land earlier if the
phases are split further.

### Phase 4: `cwd` as a selection

`jp w use cwd` selects and records the current workspace, matching `jp w use .`.

Depends on Phase 2 having shipped in a release.

## References

- [RFD 020] — session-to-conversation mappings, the record `jp c use --clear`
  drops.
- [RFD 038] — the `NONE` reset keyword, the nearest precedent for spelling
  "reset" in a value position.
- [RFD 039] — the conversation tree, and the conversation-axis half of the
  hierarchy grammar.
- [RFD 087] — the workspace targeting grammar, the `cwd` overload, and the `.`
  warning this RFD resolves.

[RFD 020]: 020-parallel-conversations.md
[RFD 038]: 038-config-reset-keywords.md
[RFD 039]: 039-conversation-trees.md
[RFD 087]: 087-session-scoped-active-workspace.md
