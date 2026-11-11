# ACP shutdown breaks the adapter's pipe instead of closing it

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=llm
- **Label**: llm-provider=anthropic
- **Label**: package=jp_llm
- **Label**: type=bug

When an ACP request sequence finishes, `rpc::drive` aborts its reader and writer
tasks, which drops the adapter's stdin and stdout.
`claude-agent-acp` writes to stdout again after that, gets `EPIPE`, and dies
with an unhandled `'error'` event and exit status 1 (stack at
`claude-agent-acp/dist/utils.js:46`).

`process::run` ignores the adapter's exit status once the foreground sequence
has returned `Ok`, so the crash no longer turns a finished response into an
error.
That was the cause of the missing conversation titles.
But every ACP request still ends with the adapter crashing, and a real failure
during shutdown is indistinguishable from this one.

Shut the connection down in order instead:

- [ ] Close the adapter's stdin, then keep reading stdout until EOF or a short
      timeout
- [ ] Kill the process group only if it has not exited by then
- [ ] Test with a fixture adapter that writes once more after stdin closes: it
      exits on its own, not from `SIGPIPE`
