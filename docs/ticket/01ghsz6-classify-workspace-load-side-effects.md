# classify workspace load side effects

- **Status**: Todo
- **Kind**: Chore
- **Authors**: Jean Mertz
- **Date**: 2026-08-12

Opening a workspace mutates it.
`load_workspace` in `jp_cli` currently performs, on every command that touches a
workspace:

- user-workspace directory creation, sibling merge, and the one-time durable
  conversation import (`with_user_storage`, RFD 031),
- the legacy `storage` symlink migration,
- a roots-registry recency upsert (`upsert_root`, RFD 087),
- a write of the workspace ID (`Workspace::id().store()`), which mints a fresh
  random ID when the stored one is unreadable.

None of these go through the persist backend, so `--no-persist` does not
suppress them.

RFD 087 introduced `LoadIntent::{Run, Inspect}` to stop `jp w show` from
reordering `l` / `latest` recency merely by reporting on a workspace.
That fixed one command by declaring it writes nothing at all.
It did not answer the general question, which is what this ticket is for:

**Which of these mutations are properties of running `jp` at all, and which
belong to specific commands?**

The two ends are clear and the middle is not.
A recency upsert plainly belongs to commands that use a workspace, not to ones
that describe it.
Repairing corrupt on-disk state plausibly belongs to any invocation that touches
the directory — but "repair" needs a careful definition first: minting a
default ID for an unreadable one *changes the workspace's identity*, which
breaks `roots::is_live` matching and silently detaches every registry entry and
session selection pointing at it.
That is closer to re-initialization than to repair.

Commands to consider once the rule exists:

- `jp c show`, `jp c ls`, `jp c print`, `jp config show` — read-only in intent,
  currently taking the full `Run` path.
- `jp w show` — already `Inspect`; check the taxonomy agrees with that choice
  rather than grandfathering it.
- The `--no-persist` contract: today it swaps the persist and lock backends but
  leaves workspace-setup writes untouched.
  Whether that is the intended boundary is part of the same question.

Deliverable: an RFD fixing the taxonomy and the vocabulary for it, then the
mechanical follow-up of tagging each command.
The `LoadIntent` enum is a starting point, not necessarily the final shape — a
two-value split may be too coarse once "always repair, never re-identify" is in
scope.

Context: PR \#866 review threads on `crates/jp_cli/src/cmd/workspace/show.rs`,
where this was raised and explicitly deferred.
