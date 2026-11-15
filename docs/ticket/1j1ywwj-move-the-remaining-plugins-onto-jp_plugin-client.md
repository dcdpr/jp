# Move the remaining plugins onto jp_plugin::client

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=plugins
- **Label**: package=jp_plugin
- **Label**: type=task

`jp_plugin::client` is the plugin's side of the protocol: `start` (the
`describe`/`init` handshake), `send`, `read`, `print`, `exit`, `result`,
`compose`, `answer_tool`, `describe` with `package!()`, and `main` for a
binary's `main`.
`jp-release` and `jp-release-github` use it.

The other command plugins still carry their own copies of the same functions,
which rustqual reports as duplicates (`DRY-001`, `DRY-003`) whenever a plugin
changes.
Copies of at least `send`, `read_message`, `send_exit`, and `send_describe` are
in `jp-rfd`, `jp-ticket`, `jp-wiki`, `jp-vite`, `jp-serve-web`, `jp-gui`, and
`jp-path`; `jp-ticket` also has `compose`, and `jp-rfd`, `jp-ticket`, and
`jp-wiki` have the `answer_tool_call` skeleton.

- [ ] Move each plugin's `run` onto `client::start`, and its `main` onto
      `client::main`
- [ ] Replace the local message helpers with `client`'s
- [ ] Read `describe`'s author and repository from `jp_plugin::package!()`
      instead of hardcoding them
- [ ] Check that `just qual` reports none of the plugin plumbing as duplicated
