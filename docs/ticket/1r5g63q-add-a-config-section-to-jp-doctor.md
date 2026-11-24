# Add a config section to jp doctor

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-24
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: type=enhancement
- **Parent**: T-1n2tnwv

A `## Config` section in `jp doctor`'s report, starting with config files that
fail to parse: user-global, workspace, the `.jp.toml` chain, and user-workspace,
plus the files their `extends` name.

The check lives in `jp_config` and loads each file on its own, so one broken
file is reported with its path and the parse error's location rather than ending
the run.
`jp doctor` never resolves the configuration, so a broken file cannot stop it
from running.

Related: T-0vnd04g (a TOML syntax error does not name the file).

- [ ] Reports each config file that fails to parse, with its path and the
      error's line and column
- [ ] Covers files reached through `extends`
- [ ] Rows use the report's table: target, problem, command
