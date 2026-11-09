# Start a temporary conversation when none is active

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: client=cli
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=pre-rfd

When the session has no active conversation, `jp query` falls back to
`conversation.default_id`, then to the picker, and fails when nobody can answer
the picker (`resolve_from_session_or_picker`,
`crates/jp_cli/src/cmd/target.rs:735`).
`jp decide`, proposed in RFD 135, finds its conversation in the same order, but
runs without one where `jp query` would open the picker.

Proposed for both commands: where the picker would open, start a temporary
conversation, as `jp query --new --tmp` creates one, and make it the session's
active conversation.
A bare `--tmp` keeps a conversation only while it stays active
(`crates/jp_cli/src/cmd/query/args.rs:610-631`), so it is gone once the session
moves on, and `jp c edit --no-tmp` keeps it.
A decision made in a new terminal then reaches the `jp query` that follows it.

The two commands take this together, so they keep finding their conversation the
same way.

Open questions:

- Without a session JP can detect, nothing holds a temporary conversation
  active.
  What each command does there: `jp decide` can keep running without a
  conversation; `jp query` fails today.
- How a user keeps the picker.
  `jp decide` has `decision.conversation = "required"` for it; `jp query` has no
  such setting, and `conversation.default_id = "ask"` is its default value, so
  it can't mean both "open the picker" and "start a temporary conversation".
- Whether a non-interactive `jp query` gets a temporary conversation too, where
  today it fails.
- `--tmp` requires `--new` today; whether that changes once a temporary
  conversation can be created implicitly.
- Titles.
  A conversation `jp decide` starts holds a pending decision and no turns, and
  `jp query` titles a conversation only when it creates it, forks it, or finds
  its stream empty of events (`crates/jp_cli/src/cmd/query.rs:604`).
  It would have to title a conversation with no turns too.
