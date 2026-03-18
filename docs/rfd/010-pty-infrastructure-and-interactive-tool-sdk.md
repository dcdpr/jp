# RFD 010: PTY Infrastructure and Interactive Tool SDK

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-02-23

## Summary

This RFD introduces a `jp_pty` crate for managing pseudo-terminal sessions and a
`jp_tool::interactive` SDK module for building stateful tools that drive
interactive CLI programs. Together they provide the infrastructure that tool
authors need to wrap programs like `git add --patch`, `vim`, or a persistent
shell session — exposing them through the stateful tool protocol defined in [RFD
009](009-stateful-tool-protocol.md).

## Motivation

RFD 009 defines the protocol for stateful tools: the `ToolState` state machine,
`ToolCommand` dispatch, handle registry, and per-tool action sets. But it
deliberately leaves open *how* a tool manages an interactive subprocess. A tool
author who wants to build a `git` tool with interactive staging support needs
to:

1. Spawn `git add --patch` in a pseudo-terminal (PTY) so it behaves as if a
   human is at the keyboard.
2. Read the terminal screen as text (not raw escape sequences) to return as
   `ToolState::Running { content }`.
3. Write keystrokes to the PTY when the assistant sends `Apply`.
4. Detect when the process exits and return `ToolState::Stopped`.
5. Implement the `AnyToolHandle` trait so JP's handle registry can manage the
   session.

Each of these steps involves low-level systems programming: PTY creation,
process forking, terminal emulation, non-blocking I/O. Without shared
infrastructure, every tool author would reimplement this from scratch.

This RFD provides two layers:

- **`jp_pty`**: A standalone crate that handles PTY lifecycle and terminal
  emulation. No dependency on JP's tool system — it can be used independently
  (e.g., for [PTY-based testing][issue-392]).
- **`jp_tool::interactive`**: An SDK module that bridges `jp_pty` to the
  stateful tool protocol. It provides the `AnyToolHandle` implementation,
  screen-to-content conversion, and helpers for defining tool commands.

[issue-392]: https://github.com/dcdpr/jp/issues/392

## Design

### User experience (tool author)

A tool author building an interactive `git` tool writes something like:

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

- Generating the `oneOf` JSON schema with `action` variants from `commands()`
- Creating the `AnyToolHandle` implementation that maps `Fetch` → screen read,
  `Apply` → PTY write, `Abort` → SIGTERM/SIGKILL
- Screen text extraction from the terminal emulator
- Activity detection (waiting for output to settle before capturing)

The tool author focuses on what commands their tool supports and how to
translate them into shell commands. The PTY mechanics are invisible.

### `jp_pty` crate

A standalone crate with no dependency on JP's tool system. It provides three
things: PTY session management, terminal emulation, and screen text extraction.

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

The session creation flow:

1. `openpty()` with the configured `Winsize` creates a master/slave pair.
2. Fork the process (see [Fork safety](#fork-safety) below).
3. In the child: `setsid()`, redirect stdin/stdout/stderr to the slave fd,
   set the slave as controlling terminal via `TIOCSCTTY`, set `$TERM`, `exec()`
   the command.
4. In the parent: close the slave fd, set master fd to non-blocking, spawn a
   background reader task.

The background reader continuously reads from the master fd and feeds bytes to
the terminal emulator. This happens on a dedicated thread (not a tokio task)
because PTY reads are blocking I/O that shouldn't occupy the async runtime.

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
scrolling, alternate screen buffer, etc.) and maintains a character grid. The
`screen_text()` method extracts the grid as plain text with trailing whitespace
trimmed per line.

**Recommended crate: `alacritty_terminal`.** It provides battle-tested VT100/
xterm-256color parsing and a complete screen buffer. It handles the full range
of escape sequences that modern programs emit.

**Fallback: `vt100`.** Lighter weight, smaller dependency footprint. Handles
basic VT100 and common xterm extensions. May struggle with programs that use
advanced features (24-bit color, bracket paste mode, etc.), but those features
don't affect screen text extraction.

**Not recommended: custom emulator.** Terminal emulation has decades of edge
cases. Writing a new one is a deep rabbit hole that wouldn't serve JP's goals.

The choice between `alacritty_terminal` and `vt100` should be made during
implementation based on binary size impact and actual compatibility with the
programs we need to support. The `Session` API is the same regardless — the
emulator is an internal detail.

#### Screen text extraction

The screen is returned as one line per terminal row, trailing whitespace
trimmed, empty trailing lines removed:

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

No ANSI codes, no cursor markers. The assistant sees clean text. Cursor
position is available via a separate `cursor_position()` method if the tool
author wants to include it in the content.

#### Fork safety

`fork()` in a multi-threaded process (which tokio creates) is a known hazard.
Only async-signal-safe functions should be called between `fork()` and `exec()`.
The pattern used here — `setsid()`, `dup2()`, `ioctl()`, `exec()` — is safe
in practice, but we should consider alternatives:

- **`posix_spawn`**: Avoids fork entirely. More limited (can't call `setsid()`
  directly), but some platforms support `POSIX_SPAWN_SETSID`.
- **`command-fds` crate**: Wraps `std::process::Command` with fd passing,
  avoiding manual fork/exec.
- **Pre-fork**: Create the PTY and fork before the tokio runtime starts.
  Impractical for on-demand session creation.

The recommended approach for the initial implementation: use `fork()` with the
minimal safe sequence. Document the constraint. Revisit if we encounter issues.

#### Platform support

PTY APIs (`openpty`, `fork`, `setsid`, `TIOCSCTTY`) are Unix-only. This crate
targets Linux and macOS. Windows support (via ConPTY) is out of scope for this
RFD.

### `jp_tool::interactive` SDK module

This module bridges `jp_pty::Session` to the stateful tool protocol from
RFD 009. It lives in the `jp_tool` crate (or a new `jp_tool_interactive`
crate if the dependency on `jp_pty` should be optional).

#### `InteractiveTool` trait

The primary interface for tool authors:

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

The `AnyToolHandle` implementation for PTY-backed tools:

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

The `settle_timeout` controls how long to wait for output to settle after
sending input. Default: 100ms. This is a trade-off between responsiveness
(return quickly) and completeness (wait for the program to finish rendering).
The tool author can override it per-command if needed.

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

Not every stateful tool needs a PTY. A tool that runs `cargo check` in the
background only needs a child process with captured stdout/stderr — no terminal
emulation. The SDK should support this with a `ProcessHandle` alongside
`PtyHandle`:

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

This lets a tool like `cargo_check` become stateful (async with polling) using
the same SDK, without bringing in PTY dependencies.

### The `git` tool — first consumer

The `git` tool is the primary motivating example and the first consumer of the
SDK. It exposes three interactive subcommands:

| Command | Shell command | Needs PTY | Accepts input |
|---------|-------------|-----------|---------------|
| `stage` | `git add --patch [args]` | Yes | Yes |
| `rebase` | `git rebase --interactive [args]` | Yes | Yes |
| `log` | `git log --oneline -20 [args]` | No | No |

`stage` and `rebase` need a PTY because they render full-screen interactive
UIs. `log` is a simple command that produces output and exits — it uses
`ProcessHandle` rather than `PtyHandle`.

The tool implementation is approximately 50 lines of Rust (the `InteractiveTool`
impl shown in the User Experience section). The SDK and `jp_pty` handle
everything else.

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

`jp_pty` has no dependency on `jp_tool`. `jp_tool::interactive` depends on
`jp_pty` behind an optional feature flag so tools that don't need PTY support
don't pay the dependency cost.

## Drawbacks

**Binary size.** `alacritty_terminal` is a significant dependency. It pulls in
a parser, screen buffer, and color handling that adds to the binary. Feature
gating (`interactive` feature on `jp_tool`) mitigates this — users who don't
need interactive tools don't pay the cost.

**Platform restriction.** PTY APIs are Unix-only. The `jp_pty` crate does not
compile on Windows. Any tool that uses `PtyHandle` is Unix-only. Tools using
`ProcessHandle` work everywhere.

**Terminal emulation fidelity.** Even with `alacritty_terminal`, some programs
may render in ways that produce confusing plain-text output (e.g., progress
bars that overwrite lines, split-pane layouts). The assistant sees the final
screen state, not the animation. This is inherent to the approach.

**Fork safety.** `fork()` in a multi-threaded process is technically
problematic. The minimal post-fork sequence we use is safe in practice but not
guaranteed by POSIX. This is a known risk shared with many other Rust projects
that spawn subprocesses.

**Screen settling heuristic.** The `settle_timeout` approach (wait N ms for
output to stop) is imperfect. A program might produce a burst of output, pause
briefly, then produce more. The assistant might see an intermediate state. More
sophisticated approaches (tracking cursor position, detecting known prompts)
add complexity for marginal benefit.

## Alternatives

### `portable-pty` instead of raw PTY APIs

The [`portable-pty`](https://crates.io/crates/portable-pty) crate provides a
higher-level, cross-platform PTY abstraction (including Windows ConPTY).

**Rejected for now because:** We only target Unix initially, and the raw API
gives us more control over the fork/exec sequence. `portable-pty` is worth
revisiting if Windows support becomes a priority.

### `vt100` instead of `alacritty_terminal`

The [`vt100`](https://crates.io/crates/vt100) crate is smaller and simpler.

**Not yet decided.** This should be evaluated during implementation based on:
(a) binary size difference, (b) whether programs we need to support use escape
sequences that `vt100` doesn't handle. Both crates expose similar screen-buffer
APIs, so switching later is feasible.

### Expose raw PTY tools to the assistant

Instead of domain tools (`git`) that use PTY internally, expose generic
`terminal_start`/`terminal_input`/`terminal_output` tools.

**Rejected in RFD 009** because it exposes the wrong abstraction. The assistant
should call domain tools, not manage terminals. This RFD provides the
infrastructure that makes domain tools easy to build.

### Run interactive tools via external daemon (interminai)

Use the [interminai](https://github.com/mstsirkin/interminai) daemon to manage
PTY sessions, communicating over Unix sockets.

**Rejected in RFD 009** because it introduces operational complexity (external
binary, daemon lifecycle, socket management) that conflicts with JP's
single-binary philosophy. The in-process approach in this RFD absorbs the
concept (PTY + terminal emulator + screen-as-text) without the daemon
architecture. interminai's protocol design (start/input/output/wait/stop)
directly informed the action set in RFD 009.

## Non-Goals

- **Windows support.** ConPTY is a different API with different semantics. A
  future RFD can address it.
- **Real-time screen streaming to the user.** The user sees tool call results,
  not live terminal output. Real-time mirroring is a potential enhancement.
- **GUI application support.** Strictly terminal/CLI programs.
- **Custom terminal emulator.** We use an existing crate, not roll our own.

## Risks and Open Questions

### `alacritty_terminal` API stability

`alacritty_terminal` is the internals of the Alacritty terminal emulator,
published as a crate. It is not designed as a stable library API — breaking
changes between versions are expected. We should pin to a specific version and
wrap it behind our own `Emulator` abstraction so a version bump doesn't
ripple through the codebase.

### Activity detection vs. fixed timeout

The `settle_timeout` approach assumes output stops arriving within N ms. This
fails for programs that:

- Produce output in bursts with pauses between them
- Show a spinner or progress indicator that updates continuously
- Wait for network I/O before rendering the final state

A more robust approach might combine the timeout with heuristics: detect when
the cursor returns to a known prompt position, or when the screen content
hasn't changed for N ms. This is worth prototyping but shouldn't block the
initial implementation.

### Screen text for alternate-screen programs

Programs like `vim` or `htop` use the terminal's alternate screen buffer. When
they exit, the terminal restores the primary buffer — the alternate screen
content disappears. For tools that need to capture the alternate screen (e.g.,
an editor tool), the screen capture must happen while the program is running,
not after it exits. The `PtyHandle.fetch()` implementation handles this
correctly (it reads the current screen state), but tool authors should be aware
of this behavior.

### Large screen output and token cost

A 24×80 terminal screen is ~1920 characters per capture. A 50-step interaction
sends ~96KB of screen text to the assistant. At $3/M input tokens, that's
roughly $0.10 per interaction. For frequent use, this adds up. Potential
optimizations (sending diffs, only changed lines, or compressed
representations) are worth exploring but are out of scope for this RFD.

### Shared `jp_pty` with testing infrastructure

[Issue #392](https://github.com/dcdpr/jp/issues/392) proposes PTY-based
end-to-end CLI testing. The `jp_pty` crate could serve both purposes: tool
sessions and test infrastructure. The API surface is the same (spawn, write,
read screen, wait). This is a potential synergy but shouldn't constrain the
design of either use case.

## Implementation Plan

### Phase 1: `jp_pty` — PTY session management

Create the `crates/jp_pty/` crate:

1. PTY creation (`openpty`, `Winsize` configuration)
2. Child process spawning (`fork` + `exec`, `setsid`, fd setup)
3. Non-blocking master fd, background reader thread
4. Terminal emulator integration (start with `alacritty_terminal`, evaluate
   `vt100` as alternative)
5. `screen_text()` — extract character grid as trimmed plain text
6. `try_wait()` — non-blocking child status check
7. `wait_for_activity()` — block until output arrives or timeout
8. `write()` — send bytes to PTY master
9. `kill()` — graceful shutdown (SIGTERM → wait → SIGKILL)

No dependency on JP's tool system. Can be reviewed and merged independently.

**Tests:** Spawn `echo hello`, verify screen contains "hello". Spawn `cat`,
write input, read it back. Spawn a program that exits, verify exit code. Test
non-blocking reads. Test kill/cleanup.

### Phase 2: `jp_tool::interactive` — SDK module

Add `interactive.rs` to `jp_tool` (or create `jp_tool_interactive` crate):

1. `InteractiveTool` trait definition
2. `Command` struct
3. `PtyHandle` — `AnyToolHandle` impl wrapping `jp_pty::Session`
4. `ProcessHandle` — `AnyToolHandle` impl for non-PTY stateful tools
5. Schema generation from `commands()` list
6. `settle_timeout` configuration

Depends on Phase 1 and RFD 009 Phase 2 (`AnyToolHandle` trait).

**Tests:** Unit tests for schema generation. Integration tests with `PtyHandle`
spawning real programs. Integration tests with `ProcessHandle`.

### Phase 3: `jp_tool_git` — first interactive tool

Create the `crates/jp_tool_git/` crate:

1. `InteractiveTool` impl for git
2. `stage` command (PTY, accepts input)
3. `rebase` command (PTY, accepts input)
4. `log` command (process, no input)
5. Register as a built-in tool

Depends on Phase 2 and RFD 009 Phase 4 (stateful tool dispatch).

**Tests:** Integration tests: spawn `git add --patch` on a test repo, send
input, verify staging result. End-to-end test with mock assistant driving the
full spawn/fetch/apply/fetch cycle.

### Phase 4: Polish and documentation

1. Feature gate `jp_pty` dependency behind `interactive` feature
2. Evaluate `alacritty_terminal` vs `vt100` binary size impact
3. Document how to build a custom interactive tool
4. Add examples to the `jp_tool::interactive` module docs

## References

- [RFD 009: Stateful Tool Protocol](009-stateful-tool-protocol.md) — defines
  `ToolState`, `ToolCommand`, `AnyToolHandle`, handle registry, and per-tool
  action sets that this RFD builds on.
- [interminai](https://github.com/mstsirkin/interminai) — prior art for
  PTY-based AI tool interaction. Validates the approach and informs the
  screen-as-text design.
- [`alacritty_terminal`](https://crates.io/crates/alacritty_terminal) —
  recommended terminal emulator crate.
- [`vt100`](https://crates.io/crates/vt100) — lightweight terminal emulator
  alternative.
- [`portable-pty`](https://crates.io/crates/portable-pty) — cross-platform
  PTY abstraction (future consideration for Windows).
- [#392](https://github.com/dcdpr/jp/issues/392) — PTY-based end-to-end CLI
  testing infrastructure (potential shared use of `jp_pty`).
- [#92](https://github.com/dcdpr/jp/issues/92) — stream output to text editor
  (related interactive workflow).
