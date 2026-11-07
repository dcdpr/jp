# RFD 010: PTY Infrastructure and Interactive Tool SDK

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23
- **Requires**: [RFD 009]
- **Summary**: Introduces the `jp_pty` crate and `jp_tool::interactive` SDK for
  building stateful tools that drive interactive CLI programs.

## Summary

This RFD introduces a `jp_pty` crate for managing pseudo-terminal sessions and a
`jp_tool::interactive` SDK module for building stateful tools that drive
interactive CLI programs.
Together they let tool authors wrap programs like `git add --patch`, `vim`, or a
persistent shell session and expose them through the stateful tool protocol from
[RFD 009].

## Motivation

[RFD 009] defines the stateful tool protocol: the `ToolState` state machine,
`ToolCommand` dispatch, the handle registry, and per-tool action sets.
It leaves open how a tool manages an interactive subprocess.
To build a `git` tool with interactive staging, an author must:

1. Spawn `git add --patch` in a pseudo-terminal (PTY) so it behaves as if a
   human is at the keyboard.
2. Read the terminal screen as plain text rather than raw escape sequences, and
   return it as `ToolState::Running { content }`.
3. Write keystrokes to the PTY when the assistant sends `Apply`.
4. Detect when the process exits and return `ToolState::Stopped`.
5. Implement `AnyToolHandle` so JP's handle registry can manage the session.

These steps require PTY allocation, process forking, terminal emulation, and
non-blocking I/O.
Without shared infrastructure, every interactive tool would reimplement them.

This RFD provides two layers:

- **`jp_pty`**: A standalone crate for PTY lifecycle management and terminal
  emulation.
  It does not depend on JP's tool system and can be used independently, such as
  for [PTY-based testing][issue-392].
- **`jp_tool::interactive`**: An SDK module that bridges `jp_pty` to the
  stateful tool protocol, providing `AnyToolHandle` implementations,
  screen-to-content conversion, and command schema generation.

## Design

### Tool author experience

A tool author implements `InteractiveTool` to define subcommands and how to
spawn them:

```rust
use jp_tool::interactive::{InteractiveTool, Command, PtyHandle};

pub struct GitTool;

impl InteractiveTool for GitTool {
    fn commands(&self) -> Vec<Command> {
        vec![
            Command::new("stage")
                .description("Interactive staging with --patch")
                .accepts_input(true),
            Command::new("rebase")
                .description("Interactive rebase")
                .accepts_input(true),
            Command::new("log")
                .description("View git log")
                .accepts_input(false),
        ]
    }

    fn spawn(&self, command: &str, args: &[String], root: &Path) -> Result<PtyHandle> {
        let mut cmd = vec!["git"];
        match command {
            "stage" => cmd.extend(["add", "--patch"]),
            "rebase" => cmd.extend(["rebase", "--interactive"]),
            "log" => cmd.extend(["log", "--oneline", "-20"]),
            _ => return Err(/* unknown command */),
        };
        cmd.extend(args.iter().map(|s| s.as_str()));

        PtyHandle::spawn(cmd, root, PtyConfig::default())
    }
}
```

The SDK handles:

- Generating the `oneOf` JSON schema with `action` variants from `commands()`.
- Mapping `AnyToolHandle` operations to the PTY (`Fetch` reads the screen,
  `Apply` writes input, and `Abort` sends `SIGTERM` followed by `SIGKILL`).
- Extracting plain text from the terminal emulator.
- Waiting for output to settle before capturing the screen.

The tool author defines the supported subcommands and translates them into shell
invocations; the SDK and `jp_pty` handle the PTY lifecycle.

### `jp_pty` crate

`jp_pty` manages PTY sessions, emulates a terminal screen, and extracts screen
content as plain text, with no dependency on JP's tool system.

#### Session

A `Session` represents a running program in a pseudo-terminal:

```rust
pub struct Session {
    master_fd: OwnedFd,
    child_pid: Pid,
    emulator: Emulator,
    config: PtyConfig,
}

pub struct PtyConfig {
    pub rows: u16,       // default: 24
    pub cols: u16,       // default: 80
    pub term: String,    // default: "xterm-256color"
}

impl Session {
    /// Spawn a command in a new PTY session.
    pub fn spawn(cmd: &[&str], cwd: &Path, config: PtyConfig) -> Result<Self>;

    /// Write bytes to the PTY (keystrokes, input data).
    pub fn write(&self, data: &[u8]) -> Result<()>;

    /// Read the current screen as plain text.
    ///
    /// Processes any pending PTY output through the terminal emulator
    /// before capturing the screen.
    pub fn screen_text(&mut self) -> String;

    /// Check if the child process is still running.
    /// Returns the exit code if it has exited.
    pub fn try_wait(&mut self) -> Option<i32>;

    /// Wait for new output activity, up to a timeout.
    ///
    /// Returns `true` if new output arrived, `false` on timeout.
    pub fn wait_for_activity(&mut self, timeout: Duration) -> bool;

    /// Send SIGTERM, wait for grace period, then SIGKILL if needed.
    pub fn kill(&mut self, grace: Duration);

    /// Resize the terminal.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()>;
}
```

#### PTY creation and process spawning

`Session::spawn` initializes the PTY and child process in four steps:

1. Call `openpty()` with the configured `Winsize` to allocate a master/slave
   descriptor pair.
2. Fork the process (see [Fork safety](#fork-safety)).
3. In the child process, call `setsid()`, redirect `stdin`, `stdout`, and
   `stderr` to the slave descriptor, set the slave as the controlling terminal
   via `TIOCSCTTY`, set `$TERM`, and `exec()` the command.
4. In the parent process, close the slave descriptor, set the master descriptor
   to non-blocking mode, and spawn a background reader thread.

The reader thread continuously reads bytes from the master descriptor and feeds
them to the terminal emulator.
It runs on a dedicated OS thread rather than a Tokio task so blocking PTY reads
do not stall the async runtime.

```rust
// Background reader (runs on a dedicated thread)
fn reader_loop(master_fd: RawFd, emulator: Arc<Mutex<Emulator>>) {
    let mut buf = [0u8; 4096];
    loop {
        match nix::unistd::read(master_fd, &mut buf) {
            Ok(0) => break,        // PTY closed
            Ok(n) => {
                emulator.lock().process(&buf[..n]);
            }
            Err(Errno::EAGAIN) => {
                // Non-blocking read, no data — poll/sleep
                poll_for_input(master_fd, Duration::from_millis(50));
            }
            Err(_) => break,
        }
    }
}
```

#### Terminal emulation

The emulator processes raw PTY output (ANSI escape sequences, cursor movement,
scrolling, and the alternate screen buffer) and maintains a character grid.

**Recommended: `alacritty_terminal`.** Parses the full range of VT100 and
`xterm-256color` escape sequences that modern CLI programs emit and maintains a
complete screen buffer.

**Fallback: `vt100`.** Has a smaller dependency footprint and handles basic
VT100 and common xterm extensions.
It lacks support for some newer terminal features (such as 24-bit color and
bracketed paste), though those rarely affect plain-text screen extraction.

**Rejected: custom emulator.** Terminal emulation has decades of edge cases that
are out of scope for JP.

Implementation will evaluate `alacritty_terminal` against `vt100` on binary size
and compatibility with target programs.
Because the emulator is internal to `jp_pty`, the `Session` API is the same
either way.

#### Screen text extraction

`screen_text()` returns one line per terminal row, with trailing whitespace on
each line and trailing blank lines stripped:

```
pick abc1234 first commit
pick def5678 fixup! first commit
pick 789abcd second commit

# Rebase abc1234..789abcd onto 000fedc (3 commands)
#
# Commands:
# p, pick <commit> = use commit
# s, squash <commit> = use commit, but meld into previous commit
```

The output contains plain text without ANSI escape sequences or cursor markers.
If a tool needs the cursor location, it can query `cursor_position()`
separately.

#### Fork safety

Calling `fork()` in a multi-threaded process such as the Tokio runtime is
hazardous unless the child calls only async-signal-safe functions before
`exec()`.
Our post-fork sequence (`setsid()`, `dup2()`, `ioctl()`, `exec()`) is safe in
practice, though several alternatives exist:

- **`posix_spawn`**: Avoids `fork()`, though setting up a controlling terminal
  is less portable (requires platform-specific flags such as
  `POSIX_SPAWN_SETSID`).
- **`command-fds` crate**: Extends `std::process::Command` with descriptor
  mappings, delegating the `fork()`/`exec()` sequence to the standard library.
- **Pre-fork helper**: Spawns a single-threaded helper process before starting
  the Tokio runtime, which is impractical for on-demand tool sessions.

The initial implementation uses `fork()` with the minimal async-signal-safe
sequence and documents the constraint.

#### Platform support

Because `openpty`, `fork`, `setsid`, and `TIOCSCTTY` are Unix APIs, `jp_pty`
targets Linux and macOS.
Windows support via ConPTY is out of scope.

### `jp_tool::interactive` SDK module

This module bridges `jp_pty::Session` to the stateful tool protocol in [RFD
009].
It lives in `jp_tool` (or in a separate `jp_tool_interactive` crate if the
`jp_pty` dependency is kept out of `jp_tool`).

#### `InteractiveTool` trait

Tool authors implement `InteractiveTool`:

```rust
pub trait InteractiveTool: Send + Sync {
    /// The commands this tool supports.
    fn commands(&self) -> Vec<Command>;

    /// Spawn a session for the given command.
    fn spawn(
        &self,
        command: &str,
        args: &[String],
        root: &Path,
    ) -> Result<PtyHandle, ToolError>;
}
```

A `Command` describes one subcommand the tool exposes:

```rust
pub struct Command {
    pub name: String,
    pub description: Option<String>,
    pub accepts_input: bool,
    pub args_schema: Option<Value>,  // JSON schema for command-specific args
}
```

`accepts_input` controls whether the `apply` action appears in the generated
schema for this tool.

#### `PtyHandle`

`PtyHandle` implements `AnyToolHandle` for PTY-backed sessions:

```rust
pub struct PtyHandle {
    session: jp_pty::Session,
    settle_timeout: Duration,
}

impl PtyHandle {
    /// Spawn a command in a PTY.
    pub fn spawn(
        cmd: &[&str],
        cwd: &Path,
        config: PtyConfig,
    ) -> Result<Self, ToolError>;
}

impl AnyToolHandle for PtyHandle {
    fn fetch(&mut self) -> ToolState {
        self.session.wait_for_activity(self.settle_timeout);
        let content = self.session.screen_text();

        match self.session.try_wait() {
            Some(0) => ToolState::Stopped {
                result: Ok(content),
            },
            Some(code) => ToolState::Stopped {
                result: Err(ToolError {
                    message: format!("Process exited with code {code}"),
                    trace: vec![],
                    transient: false,
                }),
            },
            None => ToolState::Running {
                content: Some(content),
            },
        }
    }

    fn apply(&mut self, input: Value) -> ToolState {
        if let Some(text) = input.as_str() {
            let _ = self.session.write(text.as_bytes());
        }
        // Wait for the program to process the input
        self.session.wait_for_activity(self.settle_timeout);
        self.fetch()
    }

    fn abort(&mut self) -> ToolState {
        self.session.kill(Duration::from_secs(2));
        let content = self.session.screen_text();
        ToolState::Stopped {
            result: Ok(content),
        }
    }
}
```

`settle_timeout` (100ms by default) sets how long `fetch` and `apply` wait for
PTY output before reading the screen.
Shorter timeouts return faster; longer timeouts give slow programs time to
finish rendering.
Tool authors can override this duration per command.

#### Schema generation

The SDK generates the `oneOf` JSON schema from the tool's `commands()` list:

```rust
impl InteractiveTool for T {
    fn to_schema(&self) -> Value {
        let commands: Vec<&str> = self.commands()
            .iter()
            .map(|c| c.name.as_str())
            .collect();

        let accepts_input = self.commands()
            .iter()
            .any(|c| c.accepts_input);

        let mut variants = vec![
            // spawn variant
            json!({
                "properties": {
                    "action": { "const": "spawn" },
                    "command": { "enum": commands },
                    "args": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["action", "command"]
            }),
            // fetch variant
            json!({
                "properties": {
                    "action": { "const": "fetch" },
                    "id": { "type": "string" }
                },
                "required": ["action", "id"]
            }),
        ];

        if accepts_input {
            variants.push(json!({
                "properties": {
                    "action": { "const": "apply" },
                    "id": { "type": "string" },
                    "input": { "type": "string" }
                },
                "required": ["action", "id", "input"]
            }));
        }

        // abort variant
        variants.push(json!({
            "properties": {
                "action": { "const": "abort" },
                "id": { "type": "string" }
            },
            "required": ["action", "id"]
        }));

        json!({ "type": "object", "oneOf": variants })
    }
}
```

#### Non-PTY stateful tools

Not every stateful tool needs terminal emulation.
A background `cargo check` tool only needs a child process with captured
`stdout` and `stderr`.
The SDK provides `ProcessHandle` alongside `PtyHandle` for these tools:

```rust
pub struct ProcessHandle {
    child: tokio::process::Child,
    stdout_buf: String,
    stderr_buf: String,
}

impl AnyToolHandle for ProcessHandle {
    fn fetch(&mut self) -> ToolState {
        // Read any new stdout/stderr
        self.drain_output();

        match self.child.try_wait() {
            Ok(Some(status)) => ToolState::Stopped {
                result: if status.success() {
                    Ok(self.stdout_buf.clone())
                } else {
                    Err(ToolError {
                        message: format!(
                            "Process exited with code {}",
                            status.code().unwrap_or(-1)
                        ),
                        trace: vec![self.stderr_buf.clone()],
                        transient: false,
                    })
                },
            },
            Ok(None) => ToolState::Running {
                content: if self.stdout_buf.is_empty() {
                    None
                } else {
                    Some(self.stdout_buf.clone())
                },
            },
            Err(e) => ToolState::Stopped {
                result: Err(ToolError {
                    message: e.to_string(),
                    trace: vec![],
                    transient: false,
                }),
            },
        }
    }

    fn apply(&mut self, _input: Value) -> ToolState {
        // ProcessHandle doesn't accept input — this should
        // never be called because the tool's schema omits `apply`.
        self.fetch()
    }

    fn abort(&mut self) -> ToolState {
        let _ = self.child.start_kill();
        self.fetch()
    }
}
```

With `ProcessHandle`, tools like `cargo_check` can run asynchronously and be
polled through the stateful tool protocol without pulling in `jp_pty`.

### First consumer: the `git` tool

The `git` tool is the first consumer of the SDK and exposes three subcommands:

| Command  | Shell command                     | Needs PTY | Accepts input |
| -------- | --------------------------------- | --------- | ------------- |
| `stage`  | `git add --patch [args]`          | Yes       | Yes           |
| `rebase` | `git rebase --interactive [args]` | Yes       | Yes           |
| `log`    | `git log --oneline -20 [args]`    | No        | No            |

`stage` and `rebase` run in a PTY because they prompt interactively or open an
editor.
`log` writes to `stdout` and exits, so it uses `ProcessHandle` instead of
`PtyHandle`.

As shown in [Tool author experience](#tool-author-experience), the `git` tool
implementation is roughly 50 lines of Rust.

### Crate structure

```
crates/
├── jp_pty/                     # Standalone PTY management (new)
│   ├── src/
│   │   ├── lib.rs              # Public API: Session, PtyConfig
│   │   ├── emulator.rs         # Terminal emulator wrapper
│   │   ├── process.rs          # Fork/exec, child management
│   │   └── screen.rs           # Screen text extraction
│   └── Cargo.toml              # deps: nix, alacritty_terminal or vt100
│
├── jp_tool/                    # Existing tool types
│   ├── src/
│   │   ├── lib.rs              # ToolState, ToolCommand, etc. (from RFD 009)
│   │   └── interactive.rs      # InteractiveTool, PtyHandle, ProcessHandle (new)
│   └── Cargo.toml              # optional dep on jp_pty
│
└── jp_tool_git/                # The git tool (new)
    ├── src/
    │   └── lib.rs              # InteractiveTool impl for git
    └── Cargo.toml              # deps: jp_tool
```

`jp_pty` does not depend on `jp_tool`.
`jp_tool::interactive` gates its `jp_pty` dependency behind an optional
`interactive` feature flag so non-PTY tools avoid the extra dependency.

## Drawbacks

**Binary size.** `alacritty_terminal` pulls in a VT parser, screen buffer, and
color handling.
Gating `jp_pty` behind the `interactive` feature on `jp_tool` keeps this
dependency out of builds that do not need PTY support.

**Unix-only PTY support.** `jp_pty` and `PtyHandle` rely on Unix PTY APIs and do
not compile on Windows.
Tools that use `ProcessHandle` remain cross-platform.

**Plain-text screen representation.** Capturing a static character grid loses
dynamic rendering details and can produce noisy text for split-pane layouts or
progress bars that overwrite lines.

**Multi-threaded `fork()`.** Calling `fork()` in a multi-threaded process is
only safe if the child restricts itself to async-signal-safe calls before
`exec()`.
Our post-fork sequence meets this constraint in practice, though POSIX does not
guarantee behavior across all libc implementations.

**Screen settling heuristic.** Waiting a fixed `settle_timeout` for output to
pause can capture an intermediate screen if a program pauses between bursts of
output.

## Alternatives

### `portable-pty` instead of raw PTY APIs

The [`portable-pty`] crate provides a cross-platform PTY abstraction, including
Windows ConPTY.

**Deferred.** We target Unix initially, and calling `openpty` and `fork`
directly gives us explicit control over child setup.
We can revisit `portable-pty` if we add Windows support.

### `vt100` instead of `alacritty_terminal`

The [`vt100`] crate is smaller and simpler than [`alacritty_terminal`].

**Open for evaluation.** Implementation will compare the two crates on binary
size and escape-sequence coverage for our target programs.
Because both expose similar screen-buffer APIs behind our internal `Emulator`
wrapper, switching later is straightforward.

### Expose raw PTY tools to the assistant

Instead of building domain-specific tools such as `git` on top of a PTY, expose
generic `terminal_start`, `terminal_input`, and `terminal_output` tools directly
to the assistant.

**Rejected in [RFD 009].** The assistant should invoke domain tools rather than
manage raw terminal sessions.
This RFD provides the infrastructure that makes domain tools small to implement.

### Run interactive tools via an external daemon

Use the [interminai] daemon to manage PTY sessions over Unix domain sockets.

**Rejected in [RFD 009].** Managing an external binary, daemon lifecycle, and
sockets conflicts with JP's single-binary architecture.
This RFD adopts interminai's core model (PTY, terminal emulator, and
screen-as-text capture) in-process; interminai's
`start`/`input`/`output`/`wait`/`stop` protocol also informed the action set in
RFD 009.

## Non-Goals

- **Windows support.** ConPTY has different semantics and can be addressed in a
  future RFD.
- **Real-time screen streaming.** The user sees tool call results rather than
  live terminal output.
- **GUI applications.** Only terminal and CLI programs are supported.
- **Custom terminal emulation.** `jp_pty` wraps an existing terminal emulator
  crate.

## Risks and Open Questions

### `alacritty_terminal` API stability

`alacritty_terminal` exposes the internals of the Alacritty terminal emulator
rather than a stable library API, so minor upgrades can introduce breaking
changes.
Pinning the crate version and isolating it behind an internal `Emulator` wrapper
contains upgrades to a single module.

### Activity detection vs. fixed timeout

`settle_timeout` assumes output finishes within a fixed window.
That assumption fails when a program:

- Emits output in bursts separated by pauses.
- Renders a continuously updating spinner or progress bar.
- Waits on network I/O before drawing its prompt.

Combining the timeout with heuristics, such as detecting when the cursor rests
at a prompt or when the screen grid stops changing, can handle these cases
without blocking the initial implementation.

### Screen text for alternate-screen programs

Full-screen programs like `vim` and `htop` render to the terminal's alternate
screen buffer and restore the primary buffer on exit, clearing the alternate
screen.
Tools that wrap alternate-screen programs must capture the screen before the
process exits.
`PtyHandle::fetch` reads the active buffer while the process is running, but
once the process exits, only the restored primary buffer remains.

### Large screen output and token cost

A 24x80 terminal screen holds up to 1,920 characters per capture, so a 50-step
session sends roughly 96 KB of text to the assistant (about $0.10 at $3 per
million input tokens).
Sending line diffs or only changed rows could reduce token usage in a follow-up.

### Shared `jp_pty` with testing infrastructure

[Issue \#392][issue-392] proposes PTY-based end-to-end CLI tests.
Because interactive tool sessions and CLI tests need the same operations (spawn,
write, read screen, wait), both can use `jp_pty` without coupling their designs.

## Implementation Plan

### Phase 1: `jp_pty` session management

Create the `crates/jp_pty/` crate:

1. PTY creation (`openpty`, `Winsize` configuration).
2. Child process spawning (`fork` and `exec`, `setsid`, descriptor setup).
3. Non-blocking master descriptor and background reader thread.
4. Terminal emulator integration (start with `alacritty_terminal`, evaluate
   `vt100` as an alternative).
5. `screen_text()`: extract the character grid as trimmed plain text.
6. `try_wait()`: non-blocking child status check.
7. `wait_for_activity()`: block until output arrives or the timeout expires.
8. `write()`: send bytes to the PTY master.
9. `kill()`: graceful shutdown (`SIGTERM`, wait, then `SIGKILL`).

This crate has no dependency on JP's tool system and can be reviewed and merged
independently.

**Tests:** Spawn `echo hello` and verify the screen contains `"hello"`; spawn
`cat`, write input, and read it back; spawn a process that exits and verify its
exit code; test non-blocking reads and `kill()` cleanup.

### Phase 2: `jp_tool::interactive` SDK module

Add `interactive.rs` to `jp_tool` (or create a `jp_tool_interactive` crate):

1. `InteractiveTool` trait definition.
2. `Command` struct.
3. `PtyHandle`: `AnyToolHandle` implementation wrapping `jp_pty::Session`.
4. `ProcessHandle`: `AnyToolHandle` implementation for non-PTY stateful tools.
5. Schema generation from `commands()`.
6. `settle_timeout` configuration.

Depends on Phase 1 and RFD 009 Phase 2 (`AnyToolHandle` trait).

**Tests:** Unit tests for schema generation, plus integration tests spawning
real programs with `PtyHandle` and `ProcessHandle`.

### Phase 3: `jp_tool_git` interactive tool

Create the `crates/jp_tool_git/` crate:

1. `InteractiveTool` implementation for `git`.
2. `stage` command (PTY, accepts input).
3. `rebase` command (PTY, accepts input).
4. `log` command (`ProcessHandle`, no input).
5. Registration as a built-in tool.

Depends on Phase 2 and RFD 009 Phase 4 (stateful tool dispatch).

**Tests:** Integration test that spawns `git add --patch` in a test repository,
sends input, and verifies the staged result; end-to-end test with a mock
assistant driving the `spawn`/`fetch`/`apply`/`fetch` cycle.

### Phase 4: Polish and documentation

1. Gate the `jp_pty` dependency behind the `interactive` feature flag.
2. Evaluate the binary size impact of `alacritty_terminal` versus `vt100`.
3. Document how to build a custom interactive tool.
4. Add examples to the `jp_tool::interactive` module documentation.

## References

- [RFD 009: Stateful Tool Protocol][RFD 009]: Defines `ToolState`,
  `ToolCommand`, `AnyToolHandle`, the handle registry, and per-tool action sets.
- [interminai]: Prior art for PTY-based AI tool interaction that informs the
  screen-as-text design.
- [`alacritty_terminal`]: Recommended terminal emulator crate.
- [`vt100`]: Lightweight terminal emulator alternative.
- [`portable-pty`]: Cross-platform PTY abstraction for future Windows support.
- [\#392][issue-392]: PTY-based end-to-end CLI testing infrastructure.
- [\#92]: Streaming output to a text editor.

[RFD 009]: 009-stateful-tool-protocol.md
[\#92]: https://github.com/dcdpr/jp/issues/92
[`alacritty_terminal`]: https://crates.io/crates/alacritty_terminal
[`portable-pty`]: https://crates.io/crates/portable-pty
[`vt100`]: https://crates.io/crates/vt100
[interminai]: https://github.com/mstsirkin/interminai
[issue-392]: https://github.com/dcdpr/jp/issues/392
