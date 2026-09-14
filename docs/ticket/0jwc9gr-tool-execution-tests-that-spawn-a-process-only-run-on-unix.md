# Tool-execution tests that spawn a process only run on Unix

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-14
- **Implements**: 109
- **Label**: domain=mcp
- **Label**: package=jp_cli
- **Label**: package=jp_mcp
- **Label**: type=task

Seven tests covering the JP MCP Server's process-spawning paths are gated
`#[cfg(unix)]`, so on Windows the local-command path, the inquiry re-execution
proof, and every custom-formatter behaviour are untested.

## The tests

- `jp_mcp::server::service_tests`
  - `local_inquiry_exits_and_runs_a_new_process_with_the_answer`
  - `formatter_asks_for_visibility_and_waits_for_approval`
  - `unattended_formatter_is_available_before_approval`
  - `a_formatter_is_told_the_name_the_tool_runs_under`
  - `hidden_presentation_never_executes_formatter`
- `jp_mcp::server::conformance_tests`
  - `external_inquiry_reexecutes_with_host_answers_and_records_edited_output`
- `jp_cli::cmd::query::tool::coordinator_tests`
  - `remembered_denial_does_not_run_http_argument_formatter`

## Why they are gated

Each configures a tool whose command is `{"program": "sh", "args": ["-c",
"<script>"], "shell": false}`.
The script writes a marker file to prove it ran, and prints a chosen string on
stdout.
The inquiry fixtures add a conditional branch and an embedded JSON payload.

Note this is *not* the `shell = true` gap in T-005zd01: these spawn `sh` as a
program directly.
Fixing shell-mode selection will not make these run, and making these portable
will not fix shell mode.
The two are independent.

## Why a `#[cfg(windows)]` arm is not enough

A `cmd /C` spelling of the simple formatter scripts is plausible.
The inquiry scripts branch on an argument and emit an escaped JSON envelope,
which `cmd` expresses badly enough that the fixture would stop being readable as
the thing under test.

More importantly, a second platform arm nobody can run locally is untested code
claiming coverage.

## Suggested resolution

A small test-only helper binary the fixtures invoke by path, with its behaviour
driven by arguments rather than by a shell script: write this marker, print this
text, and on a second invocation print the answer it was given.
One binary replaces every script, runs on both platforms, and makes the fixtures
state their intent directly instead of encoding it in shell.

Land it with CI actually running the suite on `windows-latest`, or the gap just
moves.
