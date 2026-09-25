# Tool-execution tests that spawn a process only run on Unix

- **Status**: Done
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

## Comments

-----

- **From**: jp
- **Date**: 2026-09-25T12:21:00Z

Resolved in PR #1175, by dependency injection rather than the helper binary
suggested above.

Running a program to completion is now the `ProcessRunner` trait in a new
`jp_process` crate.
`SystemProcessRunner` spawns the real process, and `MockProcessRunner` (behind
the `mock` feature) answers from a script and records every run.
The JP MCP Server takes the runner as a constructor argument (`Service::new`,
`TerminalExecutorSource::start`), so local tools and argument formatters run
through it.
Label resolution and the maintenance tools crate use the same trait, and
`.config/jp/tools/src/util/runner.rs` is gone.

All seven tests listed here run on the mock and are no longer gated
`#[cfg(unix)]`.
They assert what they previously inferred from marker files, directly from the
runner's call log: whether the formatter ran, how many attempts ran, which
arguments and answers each attempt got, and which directory it ran in.

The real runner is covered once, in `crates/jp_process/tests/system.rs`, through
a `process_probe` fixture binary that behaves the same on every platform.
It covers capture, exit codes, stdin, environment and clean environment, working
directory, spawn failure, lossy decoding, stderr line streaming, stop-on-match,
cancellation, and a stopped process whose own child keeps its pipes open.
The interrupt-then-kill grace period and process-group isolation are Unix-only,
and their tests are gated accordingly.

Not covered here: the two `jp_mcp::client_tests` that start `sh` as an MCP stdio
server.
Those are long-lived protocol processes, outside what `ProcessRunner` models.
