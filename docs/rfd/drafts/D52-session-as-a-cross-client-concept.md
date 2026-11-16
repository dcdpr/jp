# RFD D52: Session as a Cross-Client Concept

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-08-17
- **Summary**: Defines a session as a cross-client durable cursor to an active
  workspace and conversation, decoupling it from terminal PIDs.

## Summary

In JP, "session" currently conflates two concepts: how a client derives its
identity, and the durable cursor mapping that identity to an active conversation
and workspace.
Identity derivation is client-specific; the cursor applies to every client.
This RFD defines a session as that cross-client cursor and treats the CLI's
PID-derived identity as its weakest instance rather than its definition.

## Motivation

Alongside the CLI, JP is adding a web UI, a macOS client, and an iOS client.
Every client has the same requirement as the CLI: restoring the conversation and
workspace a view was working on.
Each client names its views differently.

**The term is undefined.** `docs/wiki/00dxybd-ubiquitous-language.md` defines
Attachment, Conversation, Event, Thread, and Turn, but has no entry for Session
despite sessions existing since [RFD 020].
Three new clients now need the concept at the same time.

**The CLI's model is the weakest of the four clients and does not generalize.**
The CLI derives a session identity from `getsid(0)` on Unix or
`GetConsoleWindow()` on Windows, both of which die with the process.
On iOS and macOS, `UISceneSession.persistentIdentifier` and window restoration
identifiers survive app relaunch by construction, and a browser tab can mint and
persist its own identifier.
Defining a session around the CLI's constraints forces the other clients into a
PID-shaped model, letting the one client that cannot restore its own identity
set the ceiling for the clients that can.

## Design

### Separating Liveness from Durability

`SessionSource` records how a session identity was derived, and cleanup
pattern-matches on it to answer two independent questions:

| Source                     | Liveness checkable | Survives client restart |
| -------------------------- | ------------------ | ----------------------- |
| `Getsid`                   | yes                | no                      |
| `Hwnd`                     | yes                | no                      |
| `Env`                      | no                 | depends on the exporter |
| client-supplied (proposed) | no                 | yes                     |

Liveness and durability are orthogonal, and cleanup policy depends on both:
liveness determines whether a session record can be reclaimed immediately, and
durability determines whether an inactive record is worth keeping.
Every non-CLI client produces identities that are durable but not
liveness-checkable, a combination `SessionSource` cannot currently express.

### Session Definition

A session is an independent client context that tracks its own active
conversation and workspace.
The client supplies the session identity; JP stores the cursor and does not
derive durable identities itself.

Under this definition, the terminal-specific derivation in `jp_cli::session` is
the CLI's implementation of that interface rather than the meaning of the term.
The CLI keeps its three-layer resolution, while a web or Apple client supplies
its native scene identifier to restore state across launches.

### Addressing Another Session

A `-s` flag lets a CLI command target another session explicitly (`jp -s <key> c
use <id>`), mirroring `jp -w` for workspaces.

This flag is in tension with [RFD 087], which restricts `jp w use` to
interactive terminals so scripts never depend on hidden per-session state.
The distinction is that [RFD 087] guards against *implicit resolution*, where a
script's behavior changes because an ambient terminal session switched
workspaces.
Passing `-s <key>` in `argv` makes the target session explicit rather than
ambient.
Accepting `-s` requires making and accepting that distinction explicitly.

<!-- Open: does `-s` belong in this RFD at all, or its own? It is the only part
here that changes CLI behavior rather than defining a concept. -->

## Drawbacks

Broadening the definition commits every client to a shared model before two of
the three new clients have shipped.
If the model fits the web UI's navigation poorly, the term will already be in
the glossary.

Client-supplied identities cannot be checked for liveness.
Because JP cannot distinguish an active scene from an abandoned one, reclaiming
those session records relies entirely on a retention policy.

## Alternatives

**Keep `session` CLI-only and let each client define its own concept.** This is
the cheapest option upfront, but it produces three implementations of the same
cursor and three terms in the glossary, creating the terminology drift the
glossary exists to prevent.

**Define the concept around the CLI's model and have other clients emulate it.**
This forces the web and Apple clients to synthesize PID-shaped ephemeral
identifiers and discard the durable identifiers they already have.

## Non-Goals

- Implementing session handling in any client.
- Defining the retention and garbage-collection policy for session records,
  which stands on its own and is not blocked on this definition.
- Deciding how the web client mints or scopes identities.

## Risks and Open Questions

**The name.** "Session" collides with HTTP and auth sessions as soon as the web
UI adds a login, violating the glossary's one-word-one-concept rule.
`context` is available (it was the earlier user-facing name for a workspace),
but readers of older material read "context" as "workspace", and in an LLM tool
the word competes with "context window".
A fresh term avoids both problems.

<!-- Candidates worth weighing rather than settling here: keep `session` and
forbid it for auth (cheapest, since `$JP_SESSION`, the storage keys, RFD 020 and
087 all already say session); `view`; `seat`. -->

**Does `sticky` generalize?** [RFD 087] adds a per-session `sticky` flag for
workspace selection.
Whether that concept is meaningful in a client whose views are already
workspace-scoped is unknown.

**Do non-CLI clients need history, or only the active pointer?** The CLI's
most-recent-first history supports `jp c use -` and `?session`.
A client with visible navigation may have no use for it.

**Who owns identity minting for the web client?** A tab-scoped identifier in
browser storage is durable but trivially forgeable, which matters if a hosted
web UI ever serves more than one user.

## Implementation Plan

1. **Phase 1: Write the glossary entry.** Settle the name and definition in
   `docs/wiki/00dxybd-ubiquitous-language.md` with no code changes.
   This phase unblocks the other clients and is the most likely to reshape the
   remaining phases.
2. **Phase 2: Split the source axes.** Represent liveness-checkability and
   durability separately, and derive cleanup policy from both while preserving
   behavior for the three existing sources.
3. **Phase 3: Add a client-supplied identity source.** Accept a durable opaque
   identifier with unknown liveness from a client.
   Depends on Phase 2.
4. **Phase 4: Add `-s` scoping.** Implement `-s <key>` only if the argument
   reconciling it with [RFD 087] is accepted.

## References

- [RFD 020]: Session identity and per-workspace conversation mapping.
- [RFD 087]: The user-global session store, the `sticky` flag, and the
  determinism argument this RFD must reconcile with.

[RFD 020]: ../020-parallel-conversations.md
[RFD 087]: ../087-session-scoped-active-workspace.md
