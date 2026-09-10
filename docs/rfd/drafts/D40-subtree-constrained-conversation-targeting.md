# RFD D40: Subtree-Constrained Conversation Targeting

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-10
- **Requires**: [RFD 039]

## Summary

This RFD adds `--root-id` to `jp query`, constraining the target conversation to
be a strict descendant of a given ancestor.
The constraint is enforced between handle resolution and per-conversation config
loading, so a conversation outside the allowed subtree never contributes config.

## Motivation

In agentic workflows ([RFD 040]), an orchestrator spawns sub-conversations under
a parent.
The sub-agent should only be able to target conversations within its assigned
subtree — not the orchestrator's own conversation, and not conversations
belonging to other sub-agents.

Today nothing expresses that scope.
A sub-agent handed `--id` can name any conversation in the workspace, and the
orchestrator has no way to fence it in.
Doing nothing means every sub-agent workflow has to trust its sub-agents to stay
in their lane, or wrap `jp` in a shell that validates IDs itself —
reimplementing ancestry checks outside JP against a tree only JP can see.

This was originally Phase 4 of [RFD 050].
It is split out because it is the only part of that RFD that needs the
conversation tree, and holding the rest of 050's scripting ergonomics behind
[RFD 039] costs more than it buys.

## Design

```sh
jp query --id=jp-c17528842001 --root-id=jp-c17528832001 "Continue"
```

`--root-id` constrains the target conversation to be a **strict descendant** of
the specified conversation.
The check is strict: the target must be a child, grandchild, or deeper
descendant.
The target **cannot** be the root-id itself.

### Enforcement timing

The constraint lives on `ConversationLoadRequest`, not on `Query`, so it is
checked between handle resolution and per-conversation config loading:

```
load_workspace  → load_base_partial
  → conversation_load_request           (Query produces it, with root_id)
  → resolve_request                     ← handles materialized
  → enforce_root_constraint             ← NEW; runs before config load
  → apply_conversation_config           (loads target's per-conversation config)
  → command.run
```

If the check ran inside `Query::run` instead, the merged `AppConfig` would
already contain config keys (model, system prompts, tools, ...) from a
conversation outside the allowed subtree, even if the run aborted before
persisting anything.
That undermines the scoping intent — see [Non-Goals](#non-goals) for why this
is *not* a security boundary, but it is still a correctness issue.

### Errors and precedence

Errors are checked in this order, so the most informative message wins:

| Order | Condition                                | Error                                                                  |
| ----- | ---------------------------------------- | ---------------------------------------------------------------------- |
| 1     | Root-id conversation does not exist      | `Root conversation <id> not found.`                                    |
| 2     | Target conversation does not exist       | (existing target-not-found error)                                      |
| 3     | Target equals root-id                    | `Conversation <id> cannot be both the target and the root constraint.` |
| 4     | Target is not a descendant of root-id    | `Conversation <target> is not a descendant of <root>.`                 |
| 5     | Target is a strict descendant of root-id | OK, query proceeds                                                     |

Existence checks must precede the equality check; otherwise the "target == root"
message is misleading when neither conversation actually exists.

### Other constraints

- `--root-id` requires `--id`.
  A script using `--root-id` knows which conversation it wants — implicit
  resolution via session mapping or `--last` would defeat the purpose of the
  constraint.
- `--root-id` is mutually exclusive with `--new` and `--fork`.
  It constrains targeting of existing conversations; `--new` and `--fork` create
  new ones.
- `--root-id` requires the tree index from [RFD 039].
  The ancestry check walks the `parent_id` chain using the in-memory tree index,
  which is O(depth) — trivial for realistic tree depths.

## Drawbacks

**It buys nothing until the tree exists.** `parent_id` and the tree index are
[RFD 039] Phase 1, and there is no flat-conversation fallback that means
anything — without ancestry, every conversation is a root and the constraint is
either vacuous or universal.
This RFD is therefore entirely gated on 039, which is the reason it is a
separate document rather than a phase of [RFD 050].

## Alternatives

### `--scope` or `--within` instead of `--root-id`

Alternative names for the ancestry constraint flag.
`--scope` is shorter but more abstract.
`--within` reads well (`--within=<id>`) but does not convey that the value is a
conversation ID.
`--root-id` is consistent with `--root` on `conversation ls` ([RFD 039]) — both
refer to tree roots — and the `-id` suffix makes clear it takes a conversation
ID.

### `--root-id` applies to `--new` / `--fork`

`--root-id` could constrain `--new` and `--fork` to create conversations *under*
the specified root, rather than being mutually exclusive.
Rejected because it would give the flag two purposes: constraining existing
targets and influencing creation.
`--fork=0 --id=<parent>` ([RFD 039]) already creates a child under a specified
parent, making the creation case redundant.

### Enforce in the sub-agent orchestrator instead of in `jp`

The orchestrator knows which subtree it handed out, so it could validate the
sub-agent's chosen ID before invoking `jp`.
Rejected because the orchestrator would have to read the tree itself to walk
ancestry, duplicating logic that belongs next to the tree index, and because
every orchestrator would reimplement it.

## Non-Goals

- **Conversation access control.** `--root-id` constrains targeting based on
  tree ancestry, not permissions.
  It is a scoping mechanism, not a security boundary.
  A sub-agent that can invoke `jp` at all can omit the flag.

- **Constraining anything other than `jp query`.** Management commands
  (`conversation rm`, `conversation archive`) take no `--root-id`.
  Extending the constraint to them is separate work, and needs a decision about
  what a subtree constraint means for a command that takes many targets.

## Implementation Plan

Single phase.

Add the `--root-id` flag to `Query`.
Model the constraint on `ConversationLoadRequest` and enforce it between
`resolve_request` and `apply_conversation_config` so the per-conversation config
layer never loads from a conversation outside the allowed subtree (see
[Enforcement timing](#enforcement-timing)).
Requires `--id`.
Mutually exclusive with `--new` and `--fork`.

Depends on [RFD 039] Phase 1 (`parent_id` and tree index).

## References

- [RFD 039: Conversation Trees][RFD 039] — defines `parent_id` and the tree
  index the ancestry check walks.
- [RFD 040: Hidden Conversations and Tool Context][RFD 040] — sub-agent
  conversations organized as children, motivating the constraint.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050] — the
  RFD this was split out of; defines `--id`, `--no-activate`, and the
  management-command conventions a constrained sub-agent uses alongside
  `--root-id`.

[RFD 039]: ../039-conversation-trees.md
[RFD 040]: ../040-hidden-conversations-and-tool-context.md
[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
