# Split jp_cli's cmd/query.rs below 800 lines

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-26
- **Label**: client=cli
- **Label**: package=jp_cli
- **Label**: type=task

`crates/jp_cli/src/cmd/query.rs` sits at the 800-line `[srp] file_length` limit
in `.config/rustqual/config.toml`, with no headroom. #1396 went over it (804
production lines, 6 independent clusters reported by SRP-002) and was brought
back to exactly 800 by tightening two call sites.
The next line added to the file fails the `qual` gate.
The file carries no `qual:allow` pin, and should not need one after this.

Split by responsibility into child modules under `cmd/query/`, next to the
existing `args`, `draft`, `interrupt`, `mcp_startup`, `stream`, `tool`, `turn`,
and `turn_loop`:

- CLI flags applied to the config: the `IntoPartialAppConfig` impl,
  `apply_auth`, `active_provider`, `apply_model`, `apply_enable_tools`,
  `effective_enable`, `effective_tool_choice`, `apply_directive_to_tool`,
  `inject_builtin_tools`, `validate_tool_use`, `apply_attachments`,
  `apply_reasoning`, `conversation_config`, `turn_config_delta`,
  `persist_config_reset`;
- workspace mounts: `MountPlan`, `apply_mounts`, `inject_mounts`,
  `create_mount_effects`, `create_workspace_symlink`, `existing_mount_target`,
  `mount_access_seed`, `declares_rules`, `is_enabled_subprocess`,
  `workspace_default_partial_rule`, `mount_partial_rule`;
- turn startup: `TurnInputs` and its `collect` and `run`, `Query::run_turn`,
  `build_thread`, `build_sections`, `last_request_id`, `ended_before_start`;
- the turn's structured result: `StructuredDataError`, `turn_structured_data`,
  `last_assistant_message`;
- quoted replies: `blockquote`, `seed_quoted_reply`, `reformat_quoted_message`;
- reading query text and schema from arguments, which belongs in the existing
  `args`: `parse_schema`, `read_piped_stdin`, `string_or_path`, `arg_file_path`,
  `query_file_path`, `read_arg_file`.

`Query::run` and `run_locked`, conversation selection and creation
(`build_conversation`, `create_new_conversation`, `acquire_lock`,
`AcquiredConversation`, `fork_conversation`), title resolution, and pre-query
compaction stay in `query.rs`.

`cmd/plugin/dispatch.rs` imports `NewTitle`, `PendingStreamTrim`, `TurnInputs`,
`last_request_id`, `resolve_new_title`, and `turn_structured_data` from
`cmd::query`.
Re-export them from `query.rs` or update that import; either keeps the plugin
host compiling.

Move each function whole, with its tests in the matching `_tests.rs` file.
`qual diff` keeps a moved item's findings at their baseline only when the old
file no longer defines it.
This is a refactor: no behavior changes, and the `jp_cli` suite passes unchanged
apart from test files moving with their code.

Done when `query.rs` is well under 800 production lines (aim for under 600, so
ordinary changes don't put it back at the limit) and `qual` passes.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-26T09:05:47Z

The split as described leaves `query.rs` at 740 production lines, not under 600.

The "800" the ticket started from only covered the first half of the file:
rustqual stops counting at the first `#[cfg(test)]` it finds, and `TurnInputs`
had one on its `stubbed_provider` field around line 1393.
Everything after it (the config, mount, quote, and structured-result code) was
never counted, so moving it out barely changed the number.
That `#[cfg(test)]` field is now in `turn_start.rs`, where it hides that file's
tail from the count the same way.

One more deviation: the `@path`/stdin/schema readers went to a new
`query/input.rs`, not `args.rs`.
`args.rs` was already at 796 lines, and adding them took it to 844.

What's left in `query.rs` that the ticket doesn't list as staying is request
composition: `edit_message`, `QuerySource`, `should_echo_request`,
`force_edit`/`force_no_edit`, and `PendingStreamTrim`.
`build_conversation` and `BuiltConversation` belong with them, although the
ticket lists `build_conversation` as staying.
Moving that cluster to `query/compose.rs` would put `query.rs` at about 550.

-----

- **From**: jp
- **Date**: 2026-11-26T09:14:46Z

Moved request composition into `query/compose.rs`: `build_conversation`,
`edit_message`, `should_echo_request`, `force_edit`, `force_no_edit`,
`QuerySource`, `BuiltConversation`, and `PendingStreamTrim`, with their tests in
`compose_tests.rs`.
`PendingStreamTrim` stays re-exported from `query.rs`, so `dispatch.rs` and
`turn_loop.rs` keep their imports.
Its fields are now `pub(super)` so tests outside `compose` can still build one.

`query.rs` is at 564 production lines, counted the same way rustqual counts
them, and `compose.rs` is at 187.
`qual` passes, and all 2363 `jp_cli` tests pass.

`turn_start.rs` still has the `#[cfg(test)]` field on `TurnInputs` that cuts
rustqual's line count short, so its count understates the file.
It is at 360 lines in total, so it is nowhere near the limit.
