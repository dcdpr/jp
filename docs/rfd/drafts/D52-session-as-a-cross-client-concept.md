# RFD D62: Session as a Cross-Client Concept

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-17

## Summary

The word "session" currently fuses two concepts: how a client derives an
identity for itself, and a durable cursor from that identity to an active
conversation and workspace.
The first is terminal-specific; the second appears in every client JP is
growing.
This RFD proposes defining the cursor as the cross-client concept, treating the
CLI's PID-derived identity as its weakest instance rather than its definition.

## Motivation

JP is acquiring clients: a web UI in active development, and a macOS client with
an iOS counterpart following.
Each one has the same requirement the CLI has — *this view was working on
conversation X in workspace Y, put it back* — and each one has a different way
of naming a view.

Two things make this worth settling now rather than later.

**The term is undefined.** `docs/architecture/ubiquitous-language.md` has
entries for Attachment, Conversation, Event, Thread, and Turn.
It has no entry for Session, despite sessions being implemented since [RFD 020].
Three clients are about to need the word at once, with nothing written down.

**The CLI's model does not generalize, and it is the weakest of the four.** A
session identity today is `getsid(0)` on Unix or `GetConsoleWindow()` on
Windows.
Both die with the process.
Meanwhile `UISceneSession.persistentIdentifier` on iOS and window restoration
identifiers on macOS survive app relaunch by construction, and a browser tab can
mint an identifier and persist it itself.
If the concept is defined by the CLI's constraints, the other clients inherit a
PID-shaped model that fits none of them — and the one client that *cannot*
restore its own identity sets the ceiling for the ones that can.

## Design

### Two axes, currently one enum

`SessionSource` answers "how was this identity derived", and cleanup
pattern-matches it to answer two independent questions:

| Source                     | Liveness checkable | Survives client restart |
| -------------------------- | ------------------ | ----------------------- |
| `Getsid`                   | yes                | no                      |
| `Hwnd`                     | yes                | no                      |
| `Env`                      | no                 | depends on the exporter |
| client-supplied (proposed) | no                 | yes                     |

Liveness and durability are orthogonal, and policy needs both: whether a record
can be reclaimed depends on liveness, whether it is worth keeping depends on
durability.
A durable-but-unverifiable identity is the exact combination the current enum
cannot express, and it is the combination every non-CLI client has.

### The concept

A session is an independent client context that tracks its own active
conversation and workspace.
The client supplies the identity; JP stores the cursor and never tries to derive
a durable identity of its own.

Under that definition the terminal-specific derivation in `jp_cli::session` is
one client's implementation of an interface, not the meaning of the term.
The CLI keeps its three-layer resolution; a web or Apple client supplies its
native scene identifier and gets restoration for free.

### Addressing another session

Scoping a command to a session it is not running in mirrors the existing
workspace flag: `jp -s <key> c use <id>`, alongside `jp -w`.

This has a deliberate tension with [RFD 087], which makes `jp w use`
interactive-only on the grounds that scripts should never depend on hidden
per-session state.
The distinction to argue: 087 objects to *implicit resolution*, where a script's
behavior changes because some terminal picked a different workspace.
A key named in argv is the opposite — nothing is ambient.
That argument needs to be made explicitly and accepted, not assumed.

<!-- Open: does `-s` belong in this RFD at all, or its own? It is the only part
here that changes CLI behavior rather than defining a concept. -->

## Drawbacks

Widening the term commits every client to a shared model before two of the three
clients have shipped anything.
The model may turn out to fit the web UI's navigation poorly, and by then it is
in the glossary.

A client-supplied identity source is unverifiable by construction.
JP cannot distinguish a live scene from an abandoned one, so those records rely
entirely on retention policy rather than liveness.

## Alternatives

**Leave session CLI-only, let each client invent its own concept.** Cheapest
now.
The cost is three implementations of the same cursor and three names for it in
the glossary, which is the drift the glossary exists to prevent.

**Define the concept around the CLI's model and have other clients emulate it.**
Requires the web and Apple clients to synthesize something PID-shaped,
discarding durable identifiers they already have.

## Non-Goals

Implementing any client's session handling.

Session record retention and collection.
That policy stands on its own and is not blocked on this definition.

Deciding how the web client mints or scopes identities.

## Risks and Open Questions

**The name.** "Session" collides with HTTP and auth sessions the moment the web
UI grows a login, which is precisely the one-word-one-concept hazard the
glossary guards against.
`context` is free again — it was the earlier user-facing name for what is now a
workspace — but it carries two costs: a reader of older material reads
"context" as "workspace", and in an LLM tool the word competes with "context
window".
Neither problem exists for a fresh word.

<!-- Candidates worth weighing rather than settling here: keep `session` and
forbid it for auth (cheapest, since `$JP_SESSION`, the storage keys, RFD 020 and
087 all already say session); `view`; `seat`. -->

**Does `sticky` generalize?** [RFD 087] adds a per-session sticky flag for
workspace selection.
Whether that concept means anything in a client whose views are already
workspace-scoped is unknown.

**Do non-CLI clients want history, or only the active pointer?** The CLI's
most-recent-first history exists to support `jp c use -` and `?session`.
A client with visible navigation may have no use for it.

**Who owns identity minting for the web client?** A tab-scoped identifier in
browser storage is durable but trivially forgeable, which matters if a hosted
web UI ever serves more than one user.

## Implementation Plan

**Phase 1 — write the glossary entry.** Settle the name and the definition in
`docs/architecture/ubiquitous-language.md`.
No code.
This is the phase that unblocks the other clients, and it is the phase most
likely to change the rest.

**Phase 2 — split the source axes.** Represent liveness-checkability and
durability separately, and derive cleanup policy from both.
Behavior-preserving for the three existing sources.

**Phase 3 — a client-supplied identity source.** Accept a durable opaque
identifier from a client, with unknown liveness.
Depends on phase 2.

**Phase 4 — `-s` scoping.** Only if the argument against [RFD 087]'s stance is
accepted.

## References

- [RFD 020] — session identity and the per-workspace mapping.
- [RFD 087] — the user-global session store, the sticky flag, and the
  determinism argument this RFD has to reconcile with.

[RFD 020]: ../020-parallel-conversations.md
[RFD 087]: ../087-session-scoped-active-workspace.md
