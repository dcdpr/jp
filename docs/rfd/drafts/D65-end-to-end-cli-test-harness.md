# RFD D65: End-to-End CLI Test Harness

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08
- **Summary**: Adds a PTY-backed end-to-end test harness that spawns the `jp`
  binary against a scripted LLM provider.

## Summary

This RFD adds a small end-to-end test suite that spawns the `jp` binary in a
pseudo-terminal against a scripted LLM provider.
It exercises the four surfaces that in-process tests bypass: `main` startup
wiring, on-disk configuration loading, process signal and exit lifecycle, and
terminal rendering under real configuration.

## Motivation

JP has no binary-level tests (`crates/jp_cli/tests/` does not exist).
Every CLI test calls functions like `run_turn_loop` in-process with
`Printer::memory` and a hand-built `AppConfig`, leaving four gaps:

**`main` wiring is unverified.** `build_printer` combines the output format,
chrome policy, output width, and stderr tracing status.
[RFD 091]'s enabling predicate depends on whether a tracing layer writes to
stderr, yet no test verifies the documented contract that `-v` disables status
regions.

**Config loading from disk is unverified.** `jp_config` tests partial merging in
memory, but no test runs the CLI loader against a real directory across `--cfg
@path`, `extends`, the `.jp.toml` chain walked up from CWD, or the user-global
and user-workspace layers.

**Process lifecycle is unverified.** In-process tests do not cover exit codes,
signal handling, or `Ctx::drop` calling `Printer::shutdown`.
[RFD 091] guarantees that Ctrl-C during a drawn status region leaves the
terminal clean, but no test checks it.

**Terminal rendering and configuration are never composed.** [T-0994dfa]
provides a PTY and screen model that `jp_printer`'s tests drive through a probe
binary, but the probe constructs its region by hand.
No test verifies what `jp query` renders from a config file.

Because no automated test exercises on-disk config, a real terminal, and a real
process together, `crates/jp_printer/examples/` carries two manual verification
fixtures (`mcp_window_fixture.toml` and `tool_window_fixture.toml`) for
inspecting terminal windows by hand.
Without binary-level tests, regressions in `main` wiring go undetected until a
user hits them.

## Design

### Existing Components

The underlying test components already exist in the workspace; only the
outermost layer that spawns `jp` is missing.

| Component                                | Location                             | Role                                                                                                |
| ---------------------------------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------- |
| `jp_test::Vcr`                           | `crates/jp_test/src/mock.rs`         | HTTP cassettes over `httpmock`, recorded from real providers with `RECORD=1`                        |
| `MockProvider`, `SequentialMockProvider` | `crates/jp_llm/src/provider/mock.rs` | In-process scripted event streams                                                                   |
| `jp_pty`                                 | [T-0994dfa]                        | Platform PTY with a `vt100` screen model fallback and `Screen` assertions for rows and cursor state |
| `region_probe`                           | `crates/jp_printer/src/bin/`         | Test binary (`[[bin]]`) spawned in a PTY to assert on the rendered screen                           |

### Harness Structure

Tests live in `crates/jp_cli/tests/`, spawn the built binary via
`CARGO_BIN_EXE_jp`, and assert on either a rendered screen or the process's
streams and exit code.

```rust
#[test]
fn verbose_logging_disables_the_status_region() {
    let ws = Fixture::new().with_config("style.mcp_startup.delay_secs = 0");

    let screen = ws.run_in_pty(&["-v", "query", "hi"]);

    assert!(!screen.contains("⏱"), "live logs on stderr must disable regions");
}
```

Deterministic execution requires controlling three external inputs:

**The provider.** The harness uses a scripted mock rather than a recorded
cassette.
Terminal rendering tests should not churn when provider fixtures are
re-recorded, and rendering should not be coupled to provider HTTP traffic;
cassettes remain dedicated to testing provider behavior.
The binary already resolves `ProviderId::Test`, so a config file selecting it
with a scripted response is sufficient.
Whether the script is passed in the config, in an environment variable, or in a
file written by the test is an implementation detail.

**The shell.** Because `run_tool_command` shells out, tool execution tests would
otherwise inherit the host shell, its timing, and its `sleep` granularity.
Following [aico], the harness prepends a stub `sh` executable to `PATH` for the
test's lifetime, reading a script that specifies what to emit on stderr and
when.
This makes tool progress and the output window testable without `sleep 0.5`
loops.

**The terminal.** Interactive cases run in `jp_pty`'s `Terminal`, matching
`jp_printer`'s region tests.
Cases that do not need a terminal, such as exit code and `--format json` output
checks, use `assert_cmd` directly.

### Initial Test Cases

This suite covers only behavior that requires spawning the `jp` binary.
It starts with five unverified claims from [RFD 091]:

1. `jp -v query` renders no status region.
2. `jp query 2>file` writes no cursor-control bytes into `file`.
3. `jp --format text-pretty query > out.txt` still renders a status region
   because stderr is a terminal.
4. `jp --quiet query` renders no status region.
5. Ctrl-C during a drawn region leaves the terminal clean.

Beyond these five cases, the suite grows only when a wiring bug escapes the
in-process tests.

### Dependencies

Non-terminal process assertions add `assert_cmd`.
All other dependencies are already in the workspace.

## Drawbacks

- **Process and PTY overhead.** Every test spawns a child process, and PTY cases
  add a terminal round-trip.
  Keeping the suite fast in CI requires keeping it small.
- **Broad failure surface.** Because end-to-end tests exercise the entire
  binary, they can fail for reasons unrelated to the behavior under test, and
  flaky failures train contributors to re-run CI rather than investigate.
- **`PATH` shadowing with a stub `sh`.** Prepending a stub `sh` shadows a binary
  that other tests in the process might invoke, so a leaked `PATH` entry can
  cause unrelated failures elsewhere.
- **Test-double maintenance.** The scripted provider, stub shell, and PTY
  introduce three test seams that can drift silently and allow tests to pass
  without exercising the real code path.

## Alternatives

- **Keep only the manual fixtures.** Visual inspection in a real terminal
  catches issues that assertions miss, and the manual fixtures remain useful for
  exploratory checks.
  However, nothing runs them automatically, so regressions go unnoticed until
  someone checks by hand.
- **Write more in-process tests.** In-process tests are faster and carry most of
  the test coverage, but they cannot reach `main`, CLI argument parsing, or
  on-disk config loading.
- **`mockito`-style hand-written HTTP stubs.** [aico] stubs provider HTTP
  responses by hand.
  `jp_test::Vcr` already records real provider traffic, which is more accurate
  than hand-written stubs.
- **Extend `region_probe` rather than spawn `jp`.** A probe binary is easier to
  control, but it constructs its own region and tests `jp_printer` rather than
  CLI wiring.

## Non-Goals

- **Mirroring the in-process suite.** Most coverage stays in unit and
  integration tests; the end-to-end suite stays minimal.
- **Testing provider behavior.** VCR cassettes already cover provider traffic.
- **Replacing the manual fixtures.** The example fixtures stay for exploratory
  verification.
- **A general CLI testing framework.** This suite adds only the support code
  needed for these tests.

## Risks and Open Questions

- **Timing-dependent flakiness.** A PTY test that waits on wall-clock time
  rather than a condition will fail under CI load.
  Tests must use `jp_pty`'s `wait_for` (which returns the screen that satisfied
  the predicate) and never call `sleep`.
- **Windows support.** `jp_pty` uses ConPTY on Windows, but a `sh` stub on
  `PATH` and Ctrl-C delivery do not carry over directly.
  Running the PTY cases on all platforms while restricting shell-stub cases to
  Unix is the likely split, but it needs an explicit decision.
- **Scripted provider input location.** Passing the script via a config key is
  discoverable and reuses the config loader under test, whereas an environment
  variable keeps test-only fields out of the config tree.
- **Crate placement.** Placing the suite in a separate crate keeps `assert_cmd`
  and the stub binaries out of `jp_cli`'s dependency graph, at the cost of
  another workspace member instead of `crates/jp_cli/tests/`.
- **Stub shell script design.** The stub shell needs a small command format for
  specifying output streams, delays, and exit codes; a clumsy format will force
  workarounds in every tool test.

## Implementation Plan

Phase 1 stands alone; phases 2 through 4 build on it, and the PTY cases depend
on `jp_pty` from [T-0994dfa].

1. **Spawn and assert.** Add `crates/jp_cli/tests/` with `assert_cmd`, a
   `Fixture` that builds a temporary workspace and writes config files, and the
   non-terminal cases covering exit codes and `--format json` output shape
   without a PTY or provider.

2. **Scripted provider.** Decide where the provider script lives, wire it
   through `ProviderId::Test`, and verify it with a test that runs a full query
   and asserts on stdout.

3. **PTY cases.** Compose `jp_pty`'s `Terminal` with `Fixture` and add the five
   status-region tests to verify [RFD 091]'s claims.

4. **Stub shell.** Add the `sh` stub on `PATH` and deterministic tool-execution
   tests for the progress window, parallel tool labeling, and a tool result
   rendering over a live window, then retire `tool_window_fixture.toml`.

## References

- [T-0994dfa]: Introduces `jp_pty`, including `Terminal`, `Screen`, and
  `wait_for`.
- [RFD 091]: Defines the status region whose enabling predicate and lifecycle
  guarantees this suite verifies.
- [RFD 048]: Defines the four-channel output model (the stdout/stderr/tty split)
  asserted by the PTY cases.
- [aico]: Comparable CLI whose `test_bins/` stub shells and `assert_cmd` suite
  inspired phases 1 and 4.
- `crates/jp_test/src/mock.rs`: `Vcr`, the existing cassette recorder.
- `crates/jp_printer/src/bin/region_probe.rs`: The spawn-into-a-PTY pattern this
  harness generalizes.
- `crates/jp_printer/examples/mcp_window_fixture.toml` and
  `tool_window_fixture.toml`: The manual fixtures this suite automates.

[RFD 048]: ../048-four-channel-output-model.md
[RFD 091]: ../091-printer-owned-status-region.md
[T-0994dfa]: ../../ticket/0994dfa-add-a-pty-harness-for-terminal-rendering-tests.md
[aico]: https://github.com/jurriaan/aico/tree/main/test_bins
