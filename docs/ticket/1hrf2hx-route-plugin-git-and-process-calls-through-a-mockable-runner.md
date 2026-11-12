# Route plugin git and process calls through a mockable runner

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-12
- **Label**: domain=plugins
- **Label**: domain=tooling
- **Label**: type=task

The command plugins and some internal crates spawn `git`, and a few other
programs, straight through `std::process::Command`.
Their tests therefore need a real `git`, build real repositories in temp
directories, and pick up the developer's global and system git configuration.
Commit signing through 1Password made the `jp-serve-chat` tests hang and fail;
hooks or diff settings in a user's config can change what a test sees.

The interim fix sets `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_SYSTEM` to empty in the
test helpers that build repositories.
That covers the helpers only: production code under test, such as ticket
`refresh`, `jp vite sync`, and wiki pinning, still runs the real `git` with the
user's configuration.

## Where processes are spawned directly

Production code:

- `plugins/command/ticket/src/lib.rs`: `git` (`refresh`: `log`, `show`, `diff`,
  `rev-parse`) and `git config` for the author
- `plugins/command/rfd/src/author.rs`, `plugins/command/wiki/src/author.rs`:
  `git config` for the author
- `plugins/command/vite/src/git.rs`: remote, repository prefix, unrecorded files
- `plugins/command/vite/src/serve.rs`: the dev server, and `jp`
- `plugins/command/wiki/src/git.rs`: two `git` calls
- `plugins/command/gui/src/launch.rs`: `open`
- `internal/overwrite/src/lib.rs`: `git`
- `plugins/command/serve-chat/src/checkout.rs` (chat bridge): `rev-parse`,
  `status`, `pull`; and `take.sh`, a shell script whose tests stub `flock` and
  `git` on `PATH`

Test helpers that build real repositories: `overwrite`, `tool-definitions`,
`rfd` (`cmd/init_tests.rs`), `ticket`, `vite` (`git_tests.rs`, `lib_tests.rs`),
`wiki` (`edit_tests.rs`, `pin_tests.rs`), and the chat bridge's checkout and
`take.sh` tests.

## Direction

Simulate up to the edge, with one trait in between, the way JP's core already
does:

- Every run-once external command in these crates goes through
  `jp_process::ProcessRunner`, passed in rather than created where it is used.
  `SystemProcessRunner` is the only implementation that spawns anything, and is
  chosen in each binary's `main`.
- A small shared git layer on top of it, shared across crates, exposes typed
  operations (status, log, show, diff, rev-parse, config, pull) and parses git's
  output in one place, tested once against fixed output.
- Tests use a fake: `MockProcessRunner` with scripted output for narrow cases,
  or a fake git implementation that models a repository's state (commits,
  working tree, remote) for the plugins' behaviour tests.
  No test of plugin logic spawns `git`.
- A few integration tests exercise the real git layer, with the user's and the
  system's configuration isolated.

## Open questions

- Where the shared git layer lives: in `jp_process`, the `doc` crate, or a new
  crate.

- Scripted output or a modelled fake repository, or both for different tests.

- Long-lived processes, such as `jp vite serve`'s dev server, are outside
  `ProcessRunner`'s scope and need their own seam.

- `take.sh` is shell: move its logic into Rust behind the same seam, or keep it
  as a script with stub-based tests.

- [ ] Production git and process calls in the listed crates go through an
      injected runner

- [ ] A shared, typed git layer, with its output parsing tested against fixed
      output

- [ ] Plugin tests use a fake, and no longer create real repositories

- [ ] A small set of real-git integration tests, isolated from user and system
      configuration
