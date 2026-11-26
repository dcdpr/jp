# Claude Code adds its own context to every ACP request

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: relevant-rfd=110
- **Label**: type=bug

On the ACP subscription flow, Claude Code adds content to every model request
that JP did not write, even though JP passes a custom system prompt (`snapshot:
false`), `setting_sources: []`, and no built-in tools, and disables auto-memory
and hooks (`acp/options.rs`).

A retained transcript (`JP_DEBUG=1`) of a one-line query on `claude-agent-acp`
with Claude Code shows two additions in the `api-request-blob` records:

- An `# Environment` block: primary working directory, git worktree and `git
  stash` advice, platform, shell, OS version, advice about untrusted downloads,
  the model name and ID, the knowledge cutoff, and today's date.
- A `<system-reminder>` prepended to the user's prompt, carrying the account's
  email address under `# userEmail`, with instructions to treat it as private.

Both reach the model on every request, consuming input tokens and shaping model
behavior: the model receives worktree and stash instructions that JP did not
supply, along with an email address the user never included in the conversation.
Exposing the email address is also a privacy issue, because JP's tools can send
conversation content to external services if the model repeats it.

Neither addition appears in JP's conversation stream, so `jp conversation print`
and other providers never show them; only requests on the ACP subscription flow
include them.

- [ ] Find which SDK options or environment variables remove each addition.
      `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS` covers git instructions and status;
      no documented variable covers `userEmail` or the environment block.
- [ ] Disable whatever can be disabled in `options::metadata` or
      `options::environment`, with a test pinning the option.
- [ ] Document what cannot be disabled in W-00dxybe, and file an upstream issue
      if the addition comes from `claude-agent-acp` or Claude Code.
- [ ] Re-record the ACP cassette and verify that the request no longer carries
      the removed content.
