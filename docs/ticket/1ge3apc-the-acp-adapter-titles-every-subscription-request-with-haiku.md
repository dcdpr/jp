# The ACP adapter titles every subscription request with Haiku

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-09
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: relevant-rfd=110
- **Label**: type=follow-up

Since `claude-agent-acp` 0.102.0 (Claude Code 2.1.321), every subscription
request makes an extra `claude-haiku-5-5` call (about 1,175 input and 16 output
tokens) to generate a session title.
The adapter publishes that title as `session_info_update`, and JP ignores it.
The cost shows up in `modelUsage` of every result and in
`cache_reconstruction_usage.snap`.

JP opens a resumed session for each request, and the adapter's
`src/session-titles.ts` calls `generateSessionTitle` for any session the CLI
doesn't title itself.
That includes resumed sessions and sessions with
`CLAUDE_CODE_DISABLE_TERMINAL_TITLE` set, so that variable has no effect here.
This was checked against a live recording.
The adapter has no option to turn the call off.

The cost is small per request, but it applies to every request and comes out of
the subscription allowance.

Possible fixes:

- Ask upstream for an opt-out (a `_meta` option or environment variable).
- Make the adapter treat the title as already chosen by the client.
  A rename makes `settled` true, but that costs an extra ACP request per session
  and relies on adapter internals.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-09T20:39:21Z

Fixed on the `claude-fable-5-2` branch.
JP's derived transcript now ends with a `custom-title` record.
The adapter's `getSessionInfo` reads it as `customTitle` and publishes it
instead of calling `generateSessionTitle`.
This covers every `session/load` request.
The first request of a conversation has no history and goes through
`session/new`, which doesn't use this transcript.

-----

- **From**: jp
- **Date**: 2026-11-09T20:44:13Z

The first request of a conversation is covered as well.
On `session/new` JP passes the Agent SDK's `title` option
(`_meta.claudeCode.options.title = "JP"`), so that session starts out titled
too.
