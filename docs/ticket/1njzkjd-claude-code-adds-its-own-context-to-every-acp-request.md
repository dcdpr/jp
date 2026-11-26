# Claude Code adds its own context to every ACP request

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-19
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: relevant-rfd=110
- **Label**: type=bug

On the ACP subscription route, Claude Code adds content to every model request
that JP did not write, even though JP passes a custom system prompt (`snapshot:
false`), `setting_sources: []`, no built-in tools, and disables auto memory and
hooks (`acp/options.rs`).

A kept transcript (`JP_DEBUG=1`) of a one-line query on `claude-agent-acp` with
Claude Code shows two additions in the `api-request-blob` records:

- An `# Environment` block: primary working directory, git worktree and `git
  stash` advice, platform, shell, OS version, advice about untrusted downloads,
  the model name and ID, the knowledge cutoff, and today's date.
- A `<system-reminder>` prepended to the user's prompt, carrying the account's
  email address under `# userEmail`, with instructions about treating it as
  private.

Both reach the model on every request, so they cost input tokens and shape its
behaviour: the model reads advice about worktrees and stashes that JP never
gave, and an email address the user never put in the conversation.
The email is a privacy concern in its own right: JP's tools can send
conversation content to external services, and the model now holds an address it
may repeat.

Neither appears in JP's conversation stream, so `jp conversation print` and the
other providers never show it; only the ACP route's requests differ.

- [ ] Find which SDK options or environment variables remove each addition.
      `CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS` covers git instructions and status;
      no documented variable covers `userEmail` or the environment block.
- [ ] Disable whatever can be disabled in `options::metadata` or
      `options::environment`, with a test pinning the option.
- [ ] Record what can't be disabled in W-00dxybe, and file it upstream if it is
      the adapter's or Claude Code's doing.
- [ ] Re-record the ACP cassette and check the request no longer carries the
      removed content.
