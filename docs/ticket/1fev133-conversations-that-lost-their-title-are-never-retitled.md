# Conversations that lost their title are never retitled

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=conversation
- **Label**: package=jp_cli
- **Label**: type=enhancement

A title is generated only when a conversation is created: `NewTitle::Generate`
in `jp_cli::cmd::query`, and in plugin dispatch only while the stream is empty
and the conversation has no title.
If that one request fails, nothing tries again.

Many conversations have no title because of a bug fixed in
`jp_llm::provider::anthropic::acp::process`: the Claude ACP adapter crashed with
`EPIPE` after JP closed its pipes, and `process::run` reported that exit as an
error, which discarded the title the model had already produced.
It was timing-dependent, so it hit some conversations and not others.
It likely dates from #1170, which introduced `process.rs`.

`jp conversation title` already retitles one conversation by hand.

Options:

1. Generate a title on a later query when the conversation still has none.
   It needs a guard so a title model that keeps failing is not retried on every
   turn.
2. Retitle every untitled conversation in one pass, for example `jp conversation
   title` accepting several targets.

Option 1 also covers future failures of any kind; option 2 only repairs the ones
that already happened.
