# A rejected `JP_CFG_*` value is reported without its reason

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-10-13
- **Label**: package=jp_cli
- **Label**: type=bug

A `JP_CFG_*` variable whose value fails to parse stops the run with an error
that says neither what is wrong nor which variable is at fault:

```text
$ JP_CFG_STYLE_REASONING_WINDOW_ROWS=loads jp config show >/dev/null
CLI Config error
| error | style_reasoning_window_rows: parse error |
```

The same value given with `--cfg` names the values that are accepted:

```text
$ jp config show -c style.reasoning.window.rows=loads
CLI Config error

 error  style.reasoning.window.rows: parse error: invalid row count `loads`: expected `off`, `auto`, `false`, `true`, or a number of rows
```

It is not specific to this key.
A value rejected by its key's own parser becomes a
`KvAssignmentErrorKind::Parse` error (`assignment_error` in
`crates/jp_config/src/assignment.rs`), which displays as `parse error` and
carries the reason as its source.

## Cause

`load_base_partial` (`crates/jp_cli/src/lib.rs:1145`) and
`load_user_global_partial` (`crates/jp_cli/src/lib.rs:1154`) turn the
`load_envs` error into `Error::CliConfig(error.to_string())`, which keeps the
outer message and drops the source.

The `--cfg` path (`crates/jp_cli/src/config_pipeline.rs:539`) formats the same
error with `crate::error::error_chain`, which appends each source.

The key in the message is the variable's name lowercased, with `JP_CFG_`
stripped (`style_reasoning_window_rows`), so it matches neither the variable
that was set nor the config key.

## Fix

Format both call sites with `error_chain`, as the `--cfg` path does, and name
the variable as it was set:

```text
JP_CFG_STYLE_REASONING_WINDOW_ROWS: parse error: invalid row count `loads`: expected `off`, `auto`, `false`, `true`, or a number of rows
```

Pin the message in a test that sets the variable.

## Context

Found while checking the config surface of RFD 120 (T-0z7cwrr).
