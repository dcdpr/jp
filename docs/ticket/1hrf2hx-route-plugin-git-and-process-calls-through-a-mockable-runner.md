# Route plugin git and process calls through a mockable runner

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=plugins
- **Label**: domain=tooling
- **Label**: type=task

Command plugins and several internal crates spawn `git` and other external
programs directly via `std::process::Command`.
Their tests require a `git` binary, initialize real repositories in temporary
directories, and inherit the host's global and system git config.
Commit signing (for example through 1Password) caused the `jp-serve-chat` tests
to hang and fail, and global hooks or diff settings can alter test output.

Setting `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` to `""` in test helpers
isolates repository setup, but production code under test (such as `jp ticket
refresh`, `jp vite sync`, and wiki link pinning) still spawns `git` with the
host's configuration.

## Where processes are spawned directly

Production code:

- `plugins/command/ticket/src/lib.rs`: `git` (`refresh`: `log`, `show`, `diff`,
  `rev-parse`) and `git config` for the author
- `plugins/command/rfd/src/author.rs`, `plugins/command/wiki/src/author.rs`:
  `git config` for the author
- `plugins/command/vite/src/git.rs`: `origin` remote URL and default branch
- `plugins/command/vite/src/serve.rs`: the dev server and `jp`
- `plugins/command/wiki/src/git.rs`: `remote`, `rev-parse`, `cat-file`,
  `merge-base`, `hash-object`, `log`, `diff`, and `status`
- `plugins/command/gui/src/launch.rs`: `open`
- `internal/overwrite/src/lib.rs`: `rev-parse`, `status`, and `ls-files`
- `plugins/command/serve-chat/src/checkout.rs` (chat bridge): `rev-parse`,
  `status`, and `pull`; and `take.sh`, a shell script whose tests stub `flock`
  and `git` on `PATH`

Test helpers that build real repositories: `overwrite`, `tool-definitions`,
`rfd` (`cmd/init_tests.rs`), `ticket`, `vite` (`git_tests.rs`, `lib_tests.rs`),
`wiki` (`edit_tests.rs`, `pin_tests.rs`), and the chat bridge's checkout and
`take.sh` tests.

## Direction

Route external process calls through a trait boundary, matching JP's core
crates:

- Route every one-shot external command in these crates through an injected
  `jp_process::ProcessRunner`.
  Only `SystemProcessRunner`, constructed in each binary's `main`, spawns OS
  processes.
- Build a shared git layer on top of `ProcessRunner` that exposes typed
  operations (`status`, `log`, `show`, `diff`, `rev-parse`, `config`, `pull`)
  and parses git output in one place, unit-tested against fixed fixtures.
- Use fakes in tests: `MockProcessRunner` with scripted output for narrow cases,
  or an in-memory fake git implementation that models repository state (commits,
  working tree, remote) for plugin behavior tests.
  Unit tests of plugin logic do not spawn `git`.
- Cover the real git layer with a small set of integration tests isolated from
  global and system git config.

## Open questions

- Where to place the shared git layer: in `jp_process`, `doc`, or a dedicated
  crate.
- Whether to use scripted output, a modeled fake repository, or both depending
  on the test.
- What seam to use for long-lived processes such as `jp vite serve`'s dev
  server, which fall outside `ProcessRunner`.
- Whether to move `take.sh` into Rust behind the same seam or keep it as a shell
  script with stub-based tests.

## Done when

- [ ] Production git and process calls in the listed crates go through an
      injected `ProcessRunner`
- [ ] A shared, typed git layer parses git output and is unit-tested against
      fixed fixtures
- [ ] Plugin unit tests use a fake and do not initialize real git repositories
- [ ] A small set of integration tests exercises the real git layer, isolated
      from global and system git config
