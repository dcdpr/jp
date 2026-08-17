# T0004: Shell-mode commands hardcode `sh` and fail on Windows

- **Status**: Todo
- **Kind**: Bug
- **Authors**: jp
- **Date**: 2026-08-14

Every consumer of `CommandConfig` with `shell = true` spawns `sh` (or `/bin/sh`)
by name.
Windows has no system `sh`, so on a Windows install without Git/MSYS on `PATH`
any shell-mode command fails.

Windows is a supported target: `.github/workflows/rust.yml` runs the `test` task
on `windows-latest`.

## Affected sites

- `crates/jp_llm/src/tool.rs` — `Command::new("sh")` for every `shell = true`
  tool command.
- `crates/jp_config/src/editor.rs` — `duct::cmd("/bin/sh", ...)`.
- `crates/jp_cli/src/cmd/label/resolve.rs` — `Command::new("sh")` for
  command-backed `conversation.labels` rules.

## Why it is filed rather than fixed in place

Raised in review of PR \#982 (conversation labels).
The label resolver matches the existing convention rather than introducing the
gap, and fixing it only for labels would leave `conversation.tools` shell
commands broken on the same platform — a uniform, documented limitation is
better than an inconsistent partial fix.

## Severity

Contained and visible, not silent.
Automatic label application reports and skips the rule; an explicit
`--label=:name` alias returns an error; a shell-mode tool call fails with a
spawn error naming the command.

## Suggested resolution

Fold into the shared command-runner extraction: one mechanical core behind the
six current spawn sites (`jp_llm::tool`, `jp_config::editor`,
`jp_attachment_cmd_output`, `jp_cli::cmd::plugin::dispatch`, `jp_mcp::client`,
`jp_cli::cmd::label::resolve`), with platform shell selection decided once
rather than six times.

Either pick the platform shell (`sh -c` on Unix, `cmd /C` or PowerShell on
Windows), or reject `shell = true` on Windows at config-validation time with a
message naming the fix.
Whichever is chosen, cover it with a test.
