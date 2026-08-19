# Re-read the workspace when an app window becomes active

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-08-19
- **Implements**: 099

`jp_workspace_conversations` and `jp_workspace_events` re-read the conversation
index before answering, so a handle kept open across a concurrent `jp query`
reports that query's turns.
The app does not take advantage of it: it reads once when a window opens a
workspace and never asks again, so the transcript on screen is whatever was on
disk at that moment.

The gap this closes is small and the mechanism is already there.
A window that re-requests its conversation list and its open transcript when it
becomes the active window would show the result of a query the user just ran in
a terminal, which is the normal way this app is used alongside `jp`.

Scope is the app side only:

- Re-request on `NSApplication` activation, or on the window becoming key,
  whichever proves less noisy.
- Keep the current selection and scroll position across the refresh.
  A list that jumps because a conversation moved to the top is worse than a
  stale one.
- Do nothing when the payload is unchanged, so an activation that found no new
  work costs no redraw.

Not this ticket: watching the store.
Nothing notifies the app that something changed, so a window left in the
foreground still shows what it last read.
That is the Live Workspace View draft's problem, and it wants `refresh_index`
(reconcile by diff) rather than the reset-style reload the boundary uses today.
