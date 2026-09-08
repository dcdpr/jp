# RFD D65: End-to-End CLI Test Harness

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-08

## Summary

This RFD adds a thin end-to-end testing tier that spawns the `jp` binary as a
process, in a pty, against a scripted provider.
It covers the four things the 5900 in-process tests cannot reach: `main`'s
wiring, config loaded from disk, process lifecycle, and terminal rendering
composed with real configuration.

The tier is deliberately small — ten or so tests, not a mirror of the
in-process suite.

## Motivation

JP has no binary-level tests.
There is no `crates/jp_cli/tests/` directory; every test calls a function like
`run_turn_loop` in-process, with a `Printer::memory` and a hand-built
`AppConfig`.

That leaves four gaps.

**`main`'s wiring is unverified.** `build_printer` composes the output format,
the chrome policy, the output width, and whether a tracing layer writes to
stderr.
Nothing tests it.
[RFD 091]'s enabling predicate depends on that last input, so "`-v` disables
status regions" is a documented user-facing contract whose only evidence is that
someone read the code and agreed.

**Config loading from disk is unverified.** `--cfg @path`, the `.jp.toml` chain
walked up from CWD, `extends`, the user-global and user-workspace layers.
`jp_config` tests the merge machinery on partials it builds in memory; nothing
runs the loader against a real directory through the real CLI.

**Process lifecycle is unverified.** Exit codes, signal handling, `Ctx::drop`
calling `Printer::shutdown`.
Whether Ctrl-C during a drawn status region leaves the terminal clean is a
guarantee [RFD 091] states and nothing checks.

**Terminal rendering and configuration are never composed.** [T-0994dfa] gives a
pty and a screen model, and `jp_printer`'s tests drive a probe binary through it
— but the probe constructs its region by hand.
No test asks what `jp query` renders given a config file.

The gap has a visible cost already: two manual verification fixtures live in
`crates/jp_printer/examples/` (`mcp_window_fixture.toml`,
`tool_window_fixture.toml`) whose entire purpose is to let a human watch
something no test can assert.
They exist because the paths they exercise — config from disk, a real terminal,
a real process — have no automated equivalent.

Doing nothing means each of those claims stays a reading of the code, and the
next person to touch `main`'s wiring finds out from a user.

## Design

### What already exists

Most of the machinery is in the tree.
This RFD is composition, not construction.

| Piece                                    | Where                                | What it gives                                                                                                                     |
| ---------------------------------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `jp_test::Vcr`                           | `crates/jp_test/src/mock.rs`         | HTTP cassettes over `httpmock`, recorded from real providers with `RECORD=1`                                                      |
| `MockProvider`, `SequentialMockProvider` | `crates/jp_llm/src/provider/mock.rs` | Scripted event streams, in-process                                                                                                |
| `jp_pty`                                 | [T-0994dfa]                        | A real pty where the platform has one, a `vt100` screen model where it does not, and a `Screen` to assert rows and cursor against |
| `region_probe`                           | `crates/jp_printer/src/bin/`         | The pattern: a `[[bin]]` spawned into a pty by a test that asserts on the rendered screen                                         |

What is missing is the outermost layer: nothing spawns `jp`.

### Shape

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

Three inputs have to be controlled for that to be deterministic.

**The provider.** A scripted mock, not a cassette.
The test above is about terminal rendering; tying it to a recorded provider
fixture would make it churn whenever those fixtures are re-recorded, and would
couple two axes that vary independently.
Cassettes stay where they are, testing provider behaviour.

The mechanism is the existing `ProviderId::Test`: the binary already resolves
it, and a config file naming it with a scripted response is enough.
Whether the script travels in the config, in an environment variable, or in a
file the test writes is an implementation detail.

**The shell.** `run_tool_command` shells out, so any test involving tool
execution inherits the host's shell, its timing, and its `sleep` granularity.
The fix is the one [aico] uses: a stub executable named `sh` placed first on
`PATH` for the test's lifetime, reading a script that says what to emit on
stderr and when.
This makes tool progress and the output window testable without `sleep 0.5`
loops.

**The terminal.** `jp_pty`'s `Terminal`, as `jp_printer`'s region tests already
use it.
Where a case needs no terminal — exit codes, `--format json` output shape —
`assert_cmd` is simpler and faster than a pty.

### Which tests belong here

The tier earns its cost only for behaviour that the outermost layer alone can
show.
Five to start, each of which is currently a claim in [RFD 091]'s pull request
with no test behind it:

1. `jp -v query` renders no status region.
2. `jp query 2>file` writes no cursor-control bytes into the file.
3. `jp --format text-pretty query > out.txt` still renders one, because stderr
   is a terminal.
4. `jp --quiet query` renders none.
5. Ctrl-C during a drawn region leaves the terminal clean.

Beyond those, the tier grows only when a bug escapes the in-process suite
because it lived in the wiring.

### New dependencies

`assert_cmd` for spawning and asserting on non-terminal runs.
Everything else is already in the tree.

## Drawbacks

- **Each test costs a process spawn**, and the pty cases cost a terminal
  round-trip on top.
  A suite of these would be the slowest thing in CI; the discipline that keeps
  it cheap is refusing to grow it.
- **The inverted pyramid is a real hazard.** End-to-end tests are the easiest to
  write badly: they touch everything, so they fail for reasons unrelated to what
  they name, and a flaky one trains people to re-run rather than read.
- **A stub `sh` on `PATH` is a sharp tool.** It shadows a binary every test in
  the process might reach, and a leaked `PATH` entry would be a confusing
  failure somewhere else entirely.
- **Three new seams to keep working**: the scripted provider, the stub shell,
  and the pty.
  Each is a thing that can rot silently, and a rotted seam turns a test green
  without testing anything.

## Alternatives

- **Keep the manual fixtures.** They work, and a human watching a real terminal
  catches things no assertion would.
  Rejected as the whole answer: nothing runs them, so a regression is found when
  someone happens to look.
  The fixtures stay useful for exploratory checks — this tier does not replace
  them, it removes the need to run them before every merge.
- **More in-process tests.** Cheaper and faster, and where most coverage
  belongs.
  They cannot reach `main`, argument parsing, or config on disk, which is
  exactly the gap.
- **`mockito`-style hand-written HTTP stubs**, as [aico] uses.
  Rejected: `jp_test::Vcr` already records real provider traffic, which is
  strictly better evidence than a stub someone wrote from memory.
- **Extend `region_probe` rather than spawn `jp`.** A probe is easier to
  control, but it constructs its own region — so it tests `jp_printer`, which
  is already covered, and not the wiring, which is not.

## Non-Goals

- **Mirroring the in-process suite.** The pyramid stays the right shape: many
  unit tests, fewer integration tests, minimal end-to-end.
- **Testing provider behaviour.** Cassettes do that, and keep doing it.
- **Replacing the manual fixtures**, which stay for exploratory verification.
- **A general CLI testing framework.** This is a handful of tests and the
  smallest support code they need.

## Risks and Open Questions

- **Flakiness is the main risk.** A pty test that waits on the clock rather than
  on a condition will fail under CI load.
  `jp_pty`'s `wait_for` exists for this and returns the screen that satisfied
  the predicate; nothing in this tier should call `sleep`.
- **Windows.** `jp_pty` reaches ConPTY there, but a stub `sh` on `PATH` does not
  translate, and neither does Ctrl-C delivery.
  The likely answer is that the pty cases run everywhere and the shell-stub
  cases are Unix-only, but that needs deciding rather than discovering.
- **Where the scripted provider's script lives** is unresolved.
  A config key is discoverable and reuses the loader the test wants to exercise;
  an environment variable keeps test-only shape out of the config tree.
- **Whether this belongs in `crates/jp_cli/tests/` or its own crate.** A
  separate crate keeps `assert_cmd` and the stub binaries out of `jp_cli`'s
  dependency graph, at the cost of another workspace member.
- **The stub shell's contract is a small language** — what to emit, on which
  stream, with what delay, and what exit code.
  Designing it badly means every tool test works around it.

## Implementation Plan

Phase 1 stands alone; the rest depend on it.
All of it depends on [T-0994dfa] having landed, since `jp_pty` is the terminal
half.

1. **Spawn and assert.** Add `crates/jp_cli/tests/` with `assert_cmd`, a
   `Fixture` that builds a temp workspace and writes config files, and the
   non-terminal cases: exit codes and `--format json` output shape.
   No pty, no provider.

2. **The scripted provider.** Settle where the script lives, wire it through
   `ProviderId::Test`, and prove it with one test that runs a full query and
   asserts on stdout.

3. **The pty cases.** Compose `jp_pty`'s `Terminal` with the fixture, and land
   the five cases named in the design.
   This is the phase that closes [RFD 091]'s unverified claims.

4. **The stub shell.** A `sh` stub on `PATH` and the tool-execution cases it
   makes deterministic: the progress window, parallel tool labelling, a tool
   result rendering over a live window.
   Retires `tool_window_fixture.toml`.

## References

- [T-0994dfa] — the pty harness this builds on; introduced `jp_pty`, its
  `Terminal`, `Screen`, and `wait_for`.
- [RFD 091] — the status region, whose enabling predicate and lifecycle
  guarantees are the first things this tier would verify.
- [RFD 048] — the four-channel output model; the stdout/stderr/tty split is
  what the pty cases assert.
- [aico] — a comparable CLI, whose `test_bins/` stub shells and `assert_cmd`
  suite are the direct inspiration for phases 1 and 4.
- `crates/jp_test/src/mock.rs` — `Vcr`, the existing cassette recorder.
- `crates/jp_printer/src/bin/region_probe.rs` — the spawn-into-a-pty pattern
  this generalises.
- `crates/jp_printer/examples/mcp_window_fixture.toml` and
  `tool_window_fixture.toml` — the manual fixtures this tier automates.

[RFD 048]: ../048-four-channel-output-model.md
[RFD 091]: ../091-printer-owned-status-region.md
[T-0994dfa]: ../../ticket/0994dfa-add-a-pty-harness-for-terminal-rendering-tests.md
[aico]: https://github.com/jurriaan/aico/tree/main/test_bins
