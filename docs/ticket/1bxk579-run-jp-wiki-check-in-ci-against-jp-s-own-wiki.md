# Run `jp wiki check` in CI against JP's own wiki

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-01
- **Label**: domain=tooling
- **Label**: relevant-rfd=127
- **Label**: type=task

Nothing in CI runs `jp wiki check` against JP's wiki.
RFD 127 asks for `jp wiki check` to run on every pull request that could break a
page, including ones that only change source, since pages link to line ranges in
source files.

Running it needs `jp`: the plugin gets the workspace root and the resolved,
layered configuration (the wiki, RFD, and ticket directories, the repository
address) from the host, and building `jp` in CI only to run one check is
expensive.
A standalone mode that reads `.jp/config.toml` itself was ruled out: it would
load configuration differently from `jp`.

Two ways forward:

- Once `jp` has binary releases (RFD 123), CI downloads a released `jp`, builds
  only `jp-wiki`, and runs `jp wiki check --strict`.
- Config loading moves into a small helper that a plugin, or a CI job, can run
  without the whole of `jp`.

The job needs full git history, since permalinks are checked against the default
branch, and has to run on source changes, not only on `docs/`.

Until then, `jp_wiki_passes_check` in `crates/plugins/command/wiki` runs the
check over JP's wiki as part of `cargo test`, reading `.jp/config.toml` as plain
TOML.
Remove it once CI runs the real command.
