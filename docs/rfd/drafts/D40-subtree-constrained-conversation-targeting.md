# RFD D40: Subtree-Constrained Conversation Targeting

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-10
- **Requires**: [RFD 039]
- **Summary**: Adds a `--root-id` flag to `jp query` that restricts target
  conversations to strict descendants of a given ancestor.

## Summary

This RFD adds `--root-id` to `jp query`, constraining the target conversation to
be a strict descendant of a given ancestor.
The constraint is enforced between handle resolution and per-conversation config
loading, so a conversation outside the allowed subtree never contributes config.

## Motivation

In agentic workflows ([RFD 040]), an orchestrator spawns sub-conversations under
a parent.
A sub-agent should only target conversations within its assigned subtree, not
the orchestrator's own conversation or conversations belonging to other
sub-agents.

A sub-agent given `--id` can target any conversation in the workspace, and the
orchestrator has no way to restrict that scope.
Without a built-in constraint, every sub-agent workflow must either trust its
sub-agents to stay within their subtree or wrap `jp` in a shell that validates
IDs itself, duplicating ancestry checks outside JP against a tree that only JP
can read.

This feature was split out from Phase 4 of [RFD 050] because it is the only part
of that proposal that depends on the conversation tree, so the rest of [RFD
050]'s scripting ergonomics do not have to wait on [RFD 039].

## Design

```sh
jp query --id=jp-c17528842001 --root-id=jp-c17528832001 "Continue"
```

`--root-id` constrains the target conversation to be a **strict descendant** of
the specified conversation: it must be a child, grandchild, or deeper
descendant, and **cannot** be the `--root-id` conversation itself.

### Enforcement Timing

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
already contain config keys (such as model, system prompts, and tools) from a
conversation outside the allowed subtree, even if the run aborted before
persisting anything.
While `--root-id` is not a security boundary (see [Non-Goals](#non-goals)),
loading config from an out-of-scope conversation would still violate the scoping
constraint.

### Errors and Precedence

Conditions are checked in this order so the most specific error is reported:

| Order | Condition                                | Error                                                                  |
| ----- | ---------------------------------------- | ---------------------------------------------------------------------- |
| 1     | Root-id conversation does not exist      | `Root conversation <id> not found.`                                    |
| 2     | Target conversation does not exist       | (existing target-not-found error)                                      |
| 3     | Target equals root-id                    | `Conversation <id> cannot be both the target and the root constraint.` |
| 4     | Target is not a descendant of root-id    | `Conversation <target> is not a descendant of <root>.`                 |
| 5     | Target is a strict descendant of root-id | OK, query proceeds                                                     |

Existence checks must precede the equality check; otherwise, passing the same
nonexistent ID for both flags would report a target-equals-root error instead of
the missing conversation.

### Other Constraints

- `--root-id` requires `--id`.
  A script using `--root-id` targets a known conversation; implicit resolution
  via session mapping or `--last` would defeat the purpose of the constraint.
- `--root-id` is mutually exclusive with `--new` and `--fork`.
  It constrains targeting of existing conversations, whereas `--new` and
  `--fork` create new ones.
- `--root-id` requires the tree index from [RFD 039].
  The ancestry check walks the `parent_id` chain in the in-memory tree index in
  O(depth) time, which is negligible for realistic tree depths.

## Drawbacks

**Requires the conversation tree.** `parent_id` and the tree index are defined
in [RFD 039] Phase 1.
There is no meaningful fallback for flat conversations: without parent links,
every conversation is a root, making an ancestry constraint either always pass
or always fail.
This RFD is therefore gated on [RFD 039], which is why it was split out from
[RFD 050].

## Alternatives

### `--scope` or `--within` instead of `--root-id`

`--scope` and `--within` are alternative names for the ancestry constraint flag.
`--scope` is shorter but more abstract, and `--within` (`--within=<id>`) does
not indicate that the value is a conversation ID.
`--root-id` matches `--root` on `conversation ls` ([RFD 039]), since both refer
to tree roots, and the `-id` suffix makes clear that the flag takes a
conversation ID.

### `--root-id` applies to `--new` / `--fork`

Instead of conflicting with `--new` and `--fork`, `--root-id` could place newly
created or forked conversations *under* the specified root.
This was rejected because it gives the flag two distinct roles: validating
existing targets and selecting a parent on creation.
`--fork=0 --id=<parent>` ([RFD 039]) already creates a child under a specified
parent, making the creation role redundant.

### Enforce in the sub-agent orchestrator instead of in `jp`

Because the orchestrator assigns the subtree, it could validate the sub-agent's
target ID before invoking `jp`.
This was rejected because every orchestrator would have to read the conversation
tree and reimplement ancestry traversal that belongs alongside the tree index in
`jp`.

## Non-Goals

- **Conversation access control.** `--root-id` constrains targeting based on
  tree ancestry, not permissions.
  It is a scoping mechanism, not a security boundary: a sub-agent that can
  invoke `jp` directly can omit the flag.

- **Constraining commands other than `jp query`.** Management commands
  (`conversation rm`, `conversation archive`) do not accept `--root-id`.
  Extending the constraint to them requires deciding how a subtree constraint
  applies when a command accepts multiple targets.

## Implementation Plan

Implementation is a single phase that depends on [RFD 039] Phase 1 (`parent_id`
and the tree index).
Add the `--root-id` flag to `Query` (requiring `--id` and conflicting with
`--new` and `--fork`), model the constraint on `ConversationLoadRequest`, and
enforce it between `resolve_request` and `apply_conversation_config` so the
per-conversation config layer never loads from a conversation outside the
allowed subtree (see [Enforcement Timing](#enforcement-timing)).

## References

- [RFD 039: Conversation Trees][RFD 039]: Defines `parent_id` and the tree index
  used by the ancestry check.
- [RFD 040: Hidden Conversations and Tool Context][RFD 040]: Organizes sub-agent
  conversations as child conversations, motivating subtree-scoped targeting.
- [RFD 050: Scripting Ergonomics for Conversation Management][RFD 050]: Defines
  `--id`, `--no-activate`, and the management-command conventions that a
  constrained sub-agent uses alongside `--root-id`.

[RFD 039]: ../039-conversation-trees.md
[RFD 040]: ../040-hidden-conversations-and-tool-context.md
[RFD 050]: ../050-scripting-ergonomics-for-conversation-management.md
