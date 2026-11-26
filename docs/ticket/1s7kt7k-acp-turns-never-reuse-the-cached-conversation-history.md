# ACP turns never reuse the cached conversation history

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-26
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: relevant-rfd=110
- **Label**: type=bug

On the ACP subscription route, prompt caching works within a turn but not across
turns.
The first request of every turn reads only the system prompt and tools from
cache and writes the whole conversation history again, at the 1-hour cache-write
rate.

Measured on six consecutive turns of one conversation (`claude-agent-acp`
0.102.0, Claude Code 2.1.321, Opus 5.5):

| Turn | cache read | cache write |
| ---- | ---------- | ----------- |
| 1    | 71,368     | 282,286     |
| 2    | 71,368     | 300,410     |
| 3    | 71,368     | 301,818     |
| 4    | 71,368     | 331,390     |
| 5    | 71,368     | 331,833     |
| 6    | 71,368     | 333,090     |

The second request of each turn reads 350k+ tokens, so caching itself works.
Turn 2 cost about $2.76, mostly cache writes.

## Cause

Each ACP request is a fresh Claude Code session resumed from a transcript JP
writes from its own events.
The request Claude Code built for an earlier turn differs from the one it builds
from JP's transcript, from the first message on:

- Claude Code prepends a `<system-reminder>` holding the account email (`#
  userEmail`) to the prompt of each session.
  JP never sees it, so no earlier prompt in JP's transcript carries it.
  See T-1njzkjd.
- When a turn started after a user message that does not end in text
  (attachments, tool results), Claude Code sent a synthetic "No response
  requested." assistant message between the two user messages.
  JP's transcript merges them into one user message.

As Claude Code sent the first turn:

```text
user:      [documents]
assistant: No response requested.
user:      <system-reminder> + request text
```

As every later turn sends it:

```text
user:      [documents] + request text
```

Either difference alone breaks the prefix at message 1.

## Why the tests miss it

`recorded_tests::cache_reconstruction` sends the same request twice and asserts
`cache_read_input_tokens > 0` on the second.
Its cacheable reference text sits in the system prompt, so the assertion holds
when the history is never reused.
It also never sends a second turn, attachments, or a tool-result tail.

## Done when

- [ ] A recorded multi-turn scenario (attachments on turn 1, a tool round, a
      follow-up turn) asserts that the follow-up's first request reads the
      previous turn's history from cache, not only the system prompt and tools.
- [ ] That scenario fails against the current transcript encoding.
- [ ] The cross-turn read covers the history, whichever fix makes it pass.

Candidate directions are open.
Reusing Claude Code's own session files across turns is a last resort, and
accepting the missing cache is preferable to it.

Related: T-1njzkjd (Claude Code's added context), T-0n0pgw6 (per-response usage
events would have surfaced this).

## Comments

-----

- **From**: jp
- **Date**: 2026-11-26T13:50:27Z

Tried `--reply-on-resume` through `_meta.claudeCode.options.extraArgs`
(`{"reply-on-resume": null}`) on adapter 0.102.0 / Claude Code 2.1.321.
The attachment case still got the `<synthetic>` "No response requested." record
between the documents and the request, so the flag does not suppress it on the
ACP path, matching anthropics/claude-code#100726.
Reverted.

Next candidate, only if nothing on the JP side works: a local proxy with
`CLAUDE_CODE_ATTRIBUTION_HEADER=0`, subject to the open questions about whether
subscription traffic without the attribution block is still accepted and whether
Claude Code sends the OAuth token to a non-Anthropic base URL.

-----

- **From**: jp
- **Date**: 2026-11-26T18:44:20Z

Fixed for multi-turn conversations by having the ACP transcript reproduce what
Claude Code sent, regenerated on every request with nothing stored:

- Where a run of user messages ending in text also holds other blocks
  (attachments on the first turn, tool results before a new turn), the
  transcript writes those blocks, Claude Code's synthetic "No response
  requested." reply, then the prompt as its own message.
- After the first prompt it writes the five context records Claude Code adds to
  a session's first prompt (`environment`, `model`, `session_context`, `date`,
  `credential_org`).
  The shell is chosen by Claude Code's documented rule and pinned through
  `CLAUDE_CODE_SHELL`; the date is that of the conversation's first request;
  model identities come from a table verified against Claude Code 2.1.321 for
  Fable 5.2/5.1/5, Opus 5.5, Sonnet 5.5 and Haiku 5.5.
- JP blanks `oauthAccount.emailAddress` in the logins it manages before each
  launch, so `session_context` is `{}` and the address never reaches the
  request.
  Claude Code writes the address back on a profile refresh, so the blanking has
  to repeat on every launch.

Measured live: three-turn conversations read the previous turn's whole request
from cache at every boundary (e.g. turn 2 read 8,008 + 1,293 = 9,301, turn 3
read 9,301 + 214 = 9,515), and the regenerated records hash identically to the
ones Claude Code wrote.

## Remaining miss: single-request first turns

When turn 1 makes no tool call, turn 2 reads only the system prompt and tools,
on every model tried (Sonnet 5.5: read 7,131 of 7,702; Fable 5.2: read 7,132 of
7,699).
Turn 2's messages hash identically to turn 1's, so the transcript is not the
cause.

| Last cache breakpoint of the earlier request                                    | Reused on the next turn? |
| ------------------------------------------------------------------------------- | ------------------------ |
| A user prompt                                                                   | yes                      |
| A message after a tool round                                                    | yes                      |
| The `# Environment` mid-conversation system message (single-request first turn) | no                       |

A single-request turn 1 puts its last breakpoint on the `# Environment` system
message Claude Code adds with a custom system prompt.
Turn 1 sends that message as a text block with `cache_control`, turn 2 as a
plain string; whether that, or the API not reusing an entry ending on a
mid-conversation system message, causes the miss is unconfirmed.
Native `claude -p` with the default prompt reuses a single-request turn 1 fully;
with `--system-prompt` its resumed turn reused nothing at all, which does not
isolate this case.

Cost: one miss per conversation, of turn 1's messages, when turn 1 makes no tool
call.
JP cannot move the breakpoint or that message.
Report upstream once the recorded multi-turn test from T-1ngnv8p reproduces it
deterministically.
