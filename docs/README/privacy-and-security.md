# Secure, Local, Private

JP conversations can be scoped to a workspace (e.g. a VCS-backed directory), or
kept out of it entirely (and out of version control), and you can switch between
them at any time:

```sh
# Conversations are stored in the current workspace by default
jp query --new "Where does this error come from?"

# You can start a local conversation, stored outside the workspace directory
jp query --new --local "What is the purpose of this module?"

# Or switch between them at any time
jp conversation edit --local
```

- **Local-first models**: First-class support for Ollama and llama.cpp.
  Run open-weight models (Llama, Mistral, Qwen, etc.) entirely offline.
- **Zero telemetry**: JP sends nothing to us.
  Your queries, conversations, and configuration never leave your control.
- **Sandboxed extensibility**: WASM-based plugins run in a true sandbox with
  capability-based security — no filesystem, network, or environment access
  unless explicitly granted.
  No security theater.
- **Terminal-safe output**: Before JP shows the assistant's messages and
  reasoning, tool results, or conversation history, it removes escape sequences
  that could move your cursor, clear your screen, retitle your window, or write
  to your clipboard.
  Colors in your own messages and in tool output are kept.
  See [`style.sanitize`] to change this.
- **Local conversations**: Store conversations outside your workspace (not
  tracked in git) for private or temporary work.
- **Memory safety**: Written in Rust.
  No buffer overflows, no use-after-free, no data races.

[back to README]

[`style.sanitize`]: ../configuration.md#terminal-escape-sequences
[back to README]: ../../README.md
