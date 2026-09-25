# Describe ACP tool calls before Claude Code requests them

- **Status**: Todo
- **Kind**: Feature
- **Authors**: jp
- **Date**: 2026-09-25
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_cli
- **Label**: package=jp_llm
- **Label**: package=jp_mcp
- **Label**: type=follow-up

With the Claude Code provider, a tool call's formatter (and any question it
asks) only starts once Claude Code sends the call's permission request.
Claude Code sends parallel calls to JP's MCP server one after another, so when
the model asks for two `ticket_create` calls with long titles, the second call's
`shorter_title` inquiry only starts after the user has answered the first call's
approval prompt.
The user waits several seconds between the two prompts.

For providers JP calls directly, this already overlaps: a call's arguments are
complete once it finishes streaming, so its formatter runs while an earlier
call's approval prompt is open (PR #1175).

## Why ACP calls can't overlap today

JP builds an ACP call's `ToolCallRequest` from the permission request's
`raw_input` (`jp_llm` `acp/protocol.rs`, the permission handler).
The model's streamed `tool_use` blocks only produce a pending row with the tool
name; their input is ignored.

The streamed input does arrive early.
In a manual run of PR #1175, the second call's complete input reached JP at
12:36:48.596, and Claude Code sent the call itself at 12:37:13.782, 9 ms after
the first call's result went back.

It can't be used as-is:

- The stream and the permission request travel separately, and stream events can
  arrive after the permission request.
- Calls made inside a subagent (`parent_tool_use_id` set) are dropped from the
  stream.
- Claude Code may change the input before it asks (e.g. through a PreToolUse
  hook).
  Not verified.

## Proposal

Describe the call speculatively, and only keep the result if it matches:

1. The ACP protocol parses a finished `tool_use` block's input and emits it as a
   hint, keyed by the tool-call id.
2. For a tool with `format = "unattended"`, the coordinator starts the formatter
   from the hint, including routing its questions.
3. When the permission request arrives, compare its `raw_input` with the hint.
   Equal: reuse the description and answers.
   Different: discard them and describe the call again.
4. A call Claude Code never sends (cancelled or restarted turn) discards its
   description; any open question is recorded as `withdrawn`.
5. A remembered `N` for the tool still skips the formatter.

The comparison guarantees the approval prompt never shows a description built
from other arguments.
The worst case is one wasted assistant request.

## Not this

Marking tools read-only so Claude Code runs them in parallel: `ticket_create`
writes a file, so the mark would be false, and it isn't confirmed that Claude
Code uses it that way.

## Open

- Whether PreToolUse hooks can rewrite tool input in practice, and how often the
  hint and `raw_input` differ.
- Whether the MCP server should keep the speculative description (it owns
  formatter runs) or the Host should.
