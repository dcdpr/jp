# Split jp_cli's lib.rs below 800 lines

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=task

`crates/jp_cli/src/lib.rs` has 946 production lines, exceeding the 800-line
`[srp] file_length` limit in `.config/rustqual/config.toml`.
It carries a `qual:allow(srp, file_length=1000)` pin so the `qual` gate fails if
the file grows past 1,000 lines.

The file was over the limit before, but went uncounted: `mod env_testing;` near
the top was marked `#[cfg(test)]`, and rustqual stops counting at the first
`#[cfg(test)]` (T-1haqsmg).
Moving `EnvVarGuard` to `jp_test::env_var` (T-1dhs4hp) removed that line.

Split by responsibility into child modules:

- the run entry point and its outcome handling (`run`, `run_inner`,
  `RunOutcome`, trace-log persistence, `parse_error`);
- config resolution and loading (`resolve_config`, `Resolved`,
  `effective_cfg_overrides`, `load_base_partial`, `load_user_global_partial`,
  `load_partial_configs_from_files`, `KeyValueOrPath`, `CfgKeyword`);
- workspace loading and checkout registration (`load_workspace`, `LoadIntent`,
  `register_checkout`, `register_workspace_checkout`,
  `cleanup_workspace_session_records`);
- logging (`TracingGuard`, `TraceSink`, `configure_logging`, `JP_CRATES`);
- runtime and threads (`build_runtime`, `worker_threads`, `num_threads`,
  `drain_background_tasks`).

Remove the `file_length` pin once the file is under 800 production lines.
Move each function whole: `qual diff` keeps a moved item's findings at their
baseline only when the old file no longer defines it.
