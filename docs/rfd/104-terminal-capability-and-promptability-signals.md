# RFD 104: Terminal Capability and Promptability Signals

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-04
- **Extends**: [RFD 048]
- **Required by**: [RFD 049]
- **Summary**: Separates terminal promptability, per-channel cursor capability,
  and format selection into distinct signals on `Printer` and `--interactive`.

## Summary

JP uses `stdout.is_terminal()` to answer four separate questions: whether stdout
accepts ANSI escapes, whether stderr accepts ANSI escapes, whether stderr can
repaint in place, and whether a user is present to answer prompts.
This RFD gives each question its own signal, moves per-channel capabilities onto
`Printer`, and pairs `--interactive` with `--no-interactive` so callers can
either assert or suppress prompting.

## Motivation

`jp conversation ls | rg fix` paused to ask which workspace to use ([\#1068]).
The prompt came from the workspace precedence ladder ([RFD 087]), which runs
whenever stdin is a terminal, and piping stdout leaves stdin attached to the
terminal.

The issue suggested that piped commands should never prompt.
This RFD does not adopt that rule: a piped command such as `jp query | less`
still has a user at the terminal who can answer prompts.
Instead, the report exposes two problems: JP had no flag to suppress prompts,
and the signal it consulted did not test whether a user was present.

Across the codebase, JP has used three differently-derived signals to answer
four distinct questions:

| Signal                   | Derivation             | Consulted by                                      |
| ------------------------ | ---------------------- | ------------------------------------------------- |
| `Term::is_tty`           | `stdout.is_terminal()` | Tool permissions, editor confirmations, lock      |
|                          |                        | timeout, plugin approval, output format, spinners |
| `TargetEnv::interactive` | `stdin.is_terminal()`  | The workspace ladder and pickers                  |
| `Sink` construction      | `--format` is pretty   | ANSI stripping on both channels                   |

When streams are redirected independently, these signals disagree:

- **Piped commands still prompt on stdin.** When stdout is piped, `Term::is_tty`
  consumers suppress prompts, but stdin remains a terminal, so the workspace
  ladder still prompts ([\#1068]).
- **In-place repaints print as separate lines.** `jp query | less` resolves
  `--format` to `text` from stdout, so `Sink` strips `\r\x1b[K` on both channels
  (`StripSink::execute` keeps line feeds and drops every other C0 control
  character, including carriage returns).
  Because stderr is still the user's terminal, each retry prints a new line
  instead of updating in place.
- **Redirected stderr writes escape codes to files.** `jp query 2> log.txt` has
  stdout on a terminal, so `--format` resolves to `text-pretty` and writes
  cursor-control sequences into `log.txt`.

[RFD 048] already separated escape-code support from promptability:

> - `stdout.is_terminal()` → can the consumer handle ANSI escape codes?
> - `/dev/tty` available → can a user answer prompts?

That decision was merged, but the code never implemented it.
`jp_printer::open_tty` opens `/dev/tty` only to give prompts a writer, and
`prompt_writer` falls back to `out` when opening `/dev/tty` fails, so the
`/dev/tty` availability check that [RFD 048] designates as the promptability
signal is computed and then discarded.

As long as these questions share a single boolean, new prompts and progress
indicators will keep consulting the wrong signal.

## Design

### Four properties

JP tracks three channel properties and one session property:

| Property                         | Question                             | Source                    |
| -------------------------------- | ------------------------------------ | ------------------------- |
| **stdout escape capability**     | Can the data consumer render ANSI?   | `--format`, defaulting to |
|                                  |                                      | `stdout.is_terminal()`    |
| **stderr escape capability**     | Can the chrome consumer render ANSI? | `--format`, defaulting to |
|                                  |                                      | `stderr.is_terminal()`    |
| **stderr cursor addressability** | Can chrome be repainted in place?    | `stderr.is_terminal()`    |
| **Promptability**                | Is a user present to answer?         | `/dev/tty`, overridable   |

#### `--format auto` resolves per channel

[RFD 048] resolves `--format auto` from `stdout.is_terminal()` and applies that
result to both channels.
In `jp query | less`, piping stdout therefore disables ANSI escapes on stderr
even though stderr is still connected to the user's terminal.

Under this RFD, an explicit `--format` value still forces both channels,
preserving [RFD 048]'s rule whenever the user specifies a format.
`--format auto` resolves per channel instead: stdout from `stdout.is_terminal()`
and stderr from `stderr.is_terminal()`.
When stdout is piped from an interactive terminal, stdout gets `{escapes:
false}` and stderr gets `{escapes: true}`.

#### Overriding escape capability vs. cursor addressability

Escape capability and cursor addressability are separate because only escape
capability can be overridden.
A user may force colour on a pipe so a pager can interpret it (`--color=always |
less -R` is the standard pattern, and `--format text-pretty` is JP's
equivalent).
Cursor movement in a redirected stream has no equivalent use case: writing
`\r\x1b[K` into a file or pipe corrupts the output.
`--format` therefore overrides escape capability and has no effect on cursor
addressability.

Cursor addressability is not user-toggleable in either direction.
Chrome repaints in place when stderr is a terminal and prints plain lines when
it is not.

#### Repainting requires both capabilities

Repainting in place emits ANSI escape sequences to a channel that must interpret
cursor movement, so it requires both capabilities:

```txt
can_repaint = caps(Err).escapes && caps(Err).addressable
```

Addressability alone is not enough.
`Sink` strips escape sequences whenever `escapes` is `false`, so a repaint
emitted when `escapes` is disabled has its carriage return and erase sequence
stripped on the way out, causing lines to accumulate on stderr.
[RFD 091] arrived at the same conjunction independently for its status region.

Promptability is independent of both channel capabilities.
In `jp query | less`, a user is still at the terminal and JP can prompt them.

### Promptability

JP resolves promptability once at startup, in this order:

1. Passing `--interactive` together with `--no-interactive` (or its
   `--non-interactive` alias) is a **usage error** because the two flags
   contradict each other.
2. An explicit CLI flag takes precedence: `--no-interactive` resolves to **not
   promptable**; `--interactive` resolves to **promptable**, or exits with an
   error if `/dev/tty` cannot be opened for reading and writing.
3. If `JP_NONINTERACTIVE` is set, the session is **not promptable**.
4. Otherwise, the session is **promptable** if and only if `/dev/tty` can be
   opened for reading and writing.

CLI flags take precedence over `JP_NONINTERACTIVE` (steps 2 and 3) so a
developer whose CI image or wrapper exports `JP_NONINTERACTIVE=1` can pass
`--interactive` on a single command to force prompting.
Promptability is therefore a tri-state request (automatic, forced on, or forced
off) rather than a boolean that the environment can unconditionally disable with
`|=`.

`/dev/tty` replaces `stdout.is_terminal()` as the default probe.
Because `/dev/tty` refers to the controlling terminal, it survives redirection
of both stdout and stderr, and it is absent when the process has no controlling
terminal (cron, systemd, a daemonised process, or `ssh` without `-t`).
On Windows, the equivalent devices are `CONOUT$` and `CONIN$`.

Opening `/dev/tty` does not guarantee that reading from it will succeed: a
process in a background process group holds a controlling terminal, but the
kernel stops it with `SIGTTIN` when it reads from `/dev/tty`.
See [Background process groups](#background-process-groups).

The two CLI flags are intentionally asymmetric:

- `--no-interactive` **overrides** detection: JP treats the session as
  unattended and resolves prompts without asking.
- `--interactive` **asserts** detection: JP requires a controlling terminal and
  fails at startup if `/dev/tty` cannot be opened.

Without a controlling terminal, JP defaults to running unattended and resolving
prompts automatically.
That fallback is the right default for background jobs, but a script or user
that expects to be prompted needs a way to fail fast instead.
`--interactive` turns a missing controlling terminal into a startup error, just
as `ssh -o BatchMode` fails fast in the opposite case.

Neither flag causes JP to write prompts to a channel that cannot carry them.

### Where each value lives

`Printer` already owns both output channels, their sinks, and the ANSI-stripping
logic, so channel capabilities live on `Printer`:

```rust
/// What a single output channel can carry.
pub struct ChannelCaps {
    /// Whether ANSI escapes reach the consumer intact.
    pub escapes: bool,

    /// Whether the channel can be repainted in place.
    pub addressable: bool,
}

impl Printer {
    pub fn caps(&self, target: PrintTarget) -> ChannelCaps;
}
```

`erase_line`, the waiting indicator, and the retry notice check `can_repaint` on
`PrintTarget::Err`.
`Sink` uses `caps(target).escapes` to decide whether to strip or pass ANSI
sequences.

Promptability is resolved in the shell at startup, before any workspace exists,
because the workspace precedence ladder ([RFD 087]) is one of its first
consumers.
Pre-`Ctx` code receives the resolved boolean as an argument (`TargetEnv` already
takes it), and `Ctx::Term` carries it for the rest of the session.
Call sites read that stored value rather than re-deriving promptability.

`prompt_writer`'s fallback to `out` is removed in the same change.
Writing an interactive prompt to the data channel corrupts stdout, and once
promptability is resolved up front, callers have already decided whether to
prompt before acquiring a writer.

`Ctx::Term` retains the session-level terminal properties:

```rust
pub(crate) struct Term {
    pub(crate) args: Globals,
    pub(crate) interactive: bool,
    pub(crate) width: Option<u16>,
}
```

`Term::is_tty` is removed rather than narrowed.
Every consumer needs one of the four specific properties (`interactive`,
`escapes`, or `addressable`), and keeping a field named `is_tty` would leave
callers to guess which property it represents.

### The rename sweep

Sixteen sites across four files name promptability `is_tty` (parameters, struct
fields, and local bindings): `cmd/query/tool/coordinator.rs` (six, including
`let can_prompt = is_tty`), `cmd/plugin/dispatch.rs` (five),
`cmd/label/resolve.rs` (four), and `cmd/lock.rs` (one).
These are renamed to `interactive` in one sweep so parameter and field names
match the signal they carry.
The error guidance in `cmd/plugin/dispatch.rs` ("run `jp <name>` in a terminal")
is updated in the same pass, since promptability depends on `/dev/tty` and
`--no-interactive` rather than whether stdout is a terminal.

Because some existing `is_tty` bindings feed both prompting and rendering
consumers, each site is migrated by behaviour rather than renamed blindly:

| Consumer                                      | Source                    |
| --------------------------------------------- | ------------------------- |
| Tool permissions, tool questions, result      | resolved promptability    |
| delivery, plugin approval, label resolution,  |                           |
| lock timeout, editor confirmation, the        |                           |
| workspace ladder and pickers                  |                           |
| Tool progress, waiting indicator, retry line, | `can_repaint` on stderr   |
| status lines                                  |                           |
| Colour, OSC titles and hyperlinks, box tables | escape capability of the  |
|                                               | target channel            |
| Table fitting                                 | existing width derivation |

For example, `ToolCoordinator::execute_with_prompting` took a single `is_tty`
flag and passed it to both permission prompts and the tool-progress task.
Replacing that flag with `interactive` alone would disable progress rendering on
a `--no-interactive` terminal run, so `execute_with_prompting` takes both values
instead.
`ToolRenderer`, lock waiting, and the pre-`Ctx` workspace prompts are audited
individually in the same way.

### Testing

Tests construct in-memory printers with `Printer::memory`, which continues to
derive channel capabilities from `OutputFormat`.
A new `Printer::memory_with` constructor accepts explicit `ChannelCaps` for
stdout and stderr so tests can exercise split-channel configurations:

```rust
// Capabilities derived from the format, as a terminal run would.
let (printer, out, err) = Printer::memory(OutputFormat::Text);

// A run whose stderr is a terminal while its stdout is piped: the case whose
// repaint accumulates today, and the one no single format can express.
let (printer, out, err) = Printer::memory_with(
    OutputFormat::Text,
    ChannelCaps { escapes: false, addressable: false },
    ChannelCaps { escapes: true, addressable: true },
);
```

This RFD does not introduce a `Printer` trait.
`Printer` has a single implementation, and `Printer::memory_with` provides the
necessary test seam.

### Overlap with RFD 091

[RFD 091] independently arrived at stderr capability checks for its status
region and has already been implemented, making this a migration of existing
code rather than a division of unbuilt work.

[RFD 091] shipped `TerminalCapability`, a `Printer` constructor input carrying
whether stderr is a terminal and the terminal's geometry, with
`TerminalCapability::interactive` as the test override and
`permits_regions(format)` as the enabling predicate.
`permits_regions(format)` is `can_repaint` under another name and already checks
stderr rather than stdout.

Rather than maintaining two parallel capability models, `Printer` owns channel
capabilities through `ChannelCaps`, and the status region becomes a consumer of
`can_repaint`.
[RFD 091] retains its rule that a tracing layer writing to stderr disables the
status region, since that guards against a competing writer rather than channel
capability.

Migrating `TerminalCapability` must preserve two requirements from the [RFD 091]
implementation:

- **Live terminal geometry.** The status region sizes its window from the
  terminal's row count and bounds each row to the column count, re-reading both
  on every draw so a block sized against a stale height does not erase rows
  outside the current viewport.
  `ChannelCaps` as sketched above carries only booleans, so either `ChannelCaps`
  must include geometry or the status region must keep a separate geometry input
  alongside it.
- **Geometry in `Printer::memory_with`.** The status region's screen-level tests
  declare a terminal of a fixed size, which a boolean-only `ChannelCaps`
  override cannot express.

This RFD intentionally departs from [RFD 091] on how `--format auto` is
resolved.
[RFD 091] resolves `--format auto` from `stdout.is_terminal()` per [RFD 048], so
`permits_regions(format)` disables the status region whenever stdout is
redirected.
Resolving `--format auto` per channel keeps ANSI escapes enabled on a terminal
stderr when stdout is piped or redirected, so `jp query > answer.txt` will
render the status region on stderr where it previously suppressed it.

## Observable behaviour changes

The pipeline from [\#1068] keeps its current behaviour by default when run from
a terminal, and can be silenced explicitly with `--no-interactive`:

```console
jp conversation ls | rg fix
# May prompt: a controlling terminal exists, so a user can answer.

jp --no-interactive conversation ls | rg fix
# Never prompts.
```

Suppressing prompts because stdout is a pipe is the heuristic this RFD removes;
`--no-interactive` provides the explicit override for scripted pipelines.

Other user-visible changes:

- `jp query | less`: stderr in-place repaints work instead of having their
  escape sequences stripped and printing each retry on a new line.
- `jp query 2> log.txt`: cursor-control sequences are no longer written to
  `log.txt`.
- `jp query` from cron or `ssh` without `-t`: runs unattended because no
  controlling terminal exists, rather than because stdout happens not to be a
  terminal.
- `jp query > out.txt` in a terminal: prompts still appear via `/dev/tty`
  instead of being skipped because stdout was redirected.

The last case is the most significant change: redirecting stdout in an
interactive terminal no longer implies unattended execution.
That matches [RFD 048] as well as tools such as `git`, `sudo`, and `fzf`.

## Drawbacks

**`/dev/tty` is not portable in spelling.** The Windows equivalents (`CONIN$`
and `CONOUT$`) behave similarly but are opened and fail differently, so the
promptability probe needs platform-specific code and tests.

**Opening `/dev/tty` for reading adds a startup side effect.** The probe is
inexpensive and required before prompting anyway, but it is one more operation
that can fail in a restrictive sandbox.

**Four properties replace a single boolean.** Callers can no longer ask a single
"am I on a terminal?" question; each call site must choose between
promptability, escape capability, and cursor addressability.

**Per-channel `auto` deviates from [RFD 048].** [RFD 048] resolves one format
from stdout and applies it to both channels, whereas this RFD preserves that
rule only for explicit `--format` values.
[RFD 048] will need a note pointing to this RFD so readers are not surprised
when `--format auto` resolves differently on stdout and stderr.

**The rename sweep touches sixteen sites across four files.** Because each site
must be classified before renaming and at least one feeds both a prompt decision
and a rendering decision, the change produces a wide diff across otherwise
stable code.

## Alternatives

### Keep `stdout.is_terminal()` and add flags for the exceptions

Keep `stdout.is_terminal()` as the single signal and let `--no-interactive` and
a hypothetical `--assume-tty` flag override the cases it gets wrong.
This was rejected because every user who pipes stdout would have to pass a flag
to keep stderr progress and prompting working, and new call sites would continue
to conflate channel formatting with promptability.

### Derive promptability from stdin

Derive promptability from `stdin.is_terminal()`, as the workspace precedence
ladder already does.
This was rejected because it breaks `jp query < prompt.txt`, where stdin is a
file while the user is still at the keyboard.
`/dev/tty` is the only source that survives redirection of any standard stream.

### One `Terminal` type owning all four properties

Fold channel capability and promptability into a single struct passed
everywhere.
This was rejected because it re-couples unrelated concerns: channel capability
belongs on `Printer`, which owns the output channels, whereas promptability
belongs to the session.

### Introduce a `Printer` trait for testing

Introduce a trait over `Printer` for test doubles.
This was rejected because `Printer` has a single implementation with no second
caller, and `Printer::memory_with` provides the required test seam once it
accepts `ChannelCaps`.

## Non-Goals

- **The detached prompt policy.** What JP does instead of prompting when no user
  is present (auto-approve, use defaults, or fail) is defined by [RFD 049].
  This RFD supplies the promptability signal that policy branches on.
- **The `Prompt` enum.** Typed prompt routing is covered by [RFD 018].
- **Whether the workspace conflict prompt should exist.** That belongs to [RFD
  087]'s design; this RFD only fixes when the prompt fires.
- **A toggle for cursor addressability.** In-place repaints happen when the
  channel supports them, with no CLI flag to force them on or off.
- **The shape of chrome records under `--format json`.** Whether a consumer can
  distinguish chrome from data in a merged stream is a machine-output concern,
  not a terminal capability concern.
- **A typed prompt transport.** Several prompts still write directly to a bare
  `io::stderr()` rather than the printer's prompt channel, so a promptable
  session can still write a prompt to a redirected stderr.
  Removing `prompt_writer`'s fallback to `out` fixes the case where `jp_printer`
  ignores its own `/dev/tty` check; routing every prompt through a single
  transport is [RFD 018].

## Risks and Open Questions

### `/dev/tty` availability in containers and multiplexers

Docker without `-t`, some CI runners, and remote agent harnesses vary in how
they expose `/dev/tty`, and a false negative there turns an interactive session
unattended.
`--interactive` lets the caller fail immediately rather than discover the
fallback after the fact, which bounds the damage but does not remove the risk.
Implementation should test these environments directly.

### The cost of probing at startup

Every invocation opens `/dev/tty`, including commands that never prompt.
[RFD 048] made the prompt writer lazy for this reason.
If opening `/dev/tty` at startup has measurable cost, the probe can be deferred
behind a `OnceCell`, at the cost of evaluating promptability on first use rather
than resolving it once in the shell.

### Background process groups

A process in a background process group holds a controlling terminal, so the
`/dev/tty` probe succeeds, but reading from `/dev/tty` raises `SIGTTIN` and
stops the process by default.
`jp query "…" &` therefore passes `--interactive`'s startup assertion and then
suspends at the first prompt.
Although the shell reports the stopped job, `--interactive` promises more at
startup than it can verify.

Two options remain open: compare `tcgetpgrp(tty_fd)` with the process group ID
when deriving promptability (which is Unix-only and untested across the
containers and multiplexers discussed above), or narrow `--interactive`'s
contract to "a controlling-terminal endpoint exists" and let the first prompt
fail at runtime with a typed error.

### Capability and writer destination must change together

[RFD 021] proposes swapping a printer's `out` and `err` writers at runtime.
Capabilities captured at construction become stale as soon as a writer is
replaced by one with different properties.
Whichever RFD lands second must swap the writer destination and its
`ChannelCaps` atomically.

### `--interactive` on a partially available terminal

`/dev/tty` can in principle open for writing but not reading.
This proposal requires both read and write access and errors under
`--interactive` otherwise; whether any real environment produces a write-only
`/dev/tty` is unknown.

## Implementation Plan

### Phase 1: Channel capability on `Printer`

Introduce `ChannelCaps` and `Printer::caps`.
Resolve `--format auto` per channel, keep explicit `--format` values forcing
both channels, and derive `addressable` from `stderr.is_terminal()`.
Move `Sink`'s ANSI-stripping decision onto `caps(target).escapes`.
Point `erase_line`, `spawn_line_timer`, and the retry notice at `can_repaint`
instead of `--format` and `is_tty`.
Add `Printer::memory_with`.

This phase fixes the accumulating retry lines and redirected log-file escape
leaks on its own, and can be merged independently.

### Phase 2: Promptability from `/dev/tty`

Add read-access checking to `open_tty`, expose `/dev/tty` availability as a
boolean value rather than falling back to `out` in `prompt_writer`, and resolve
promptability from it in the shell.
Add `--interactive` with assertion semantics and remove `Term::is_tty`.

This phase depends on Phase 1 for the `Printer`-side capabilities that replace
the rendering uses of `Term::is_tty`.

### Phase 3: The rename sweep

Classify each of the sixteen `is_tty` sites against the migration table above,
split any site that feeds both a prompt decision and a rendering decision,
rename the promptability sites to `interactive`, update their doc comments, and
correct `dispatch.rs`'s terminal-specific guidance.

This phase depends on Phase 2 and is kept in a separate commit so the
behavioural changes and the mechanical renames bisect cleanly.

## References

- [RFD 048: Four-Channel Output Model][RFD 048]: defines the four output
  channels and specifies the stdout-versus-`/dev/tty` split this RFD implements.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049]: defines
  the detached prompt policy that consumes this RFD's promptability signal.
- [RFD 087: Session-Scoped Active Workspace][RFD 087]: its precedence ladder
  defers to "the same promptability signal JP already uses elsewhere"; this RFD
  defines that signal.
- [RFD 091: Printer-Owned Status Region][RFD 091]: its enabling predicate is
  this RFD's `can_repaint`, reached independently and already implemented (see
  [Overlap with RFD 091](#overlap-with-rfd-091)).
- [RFD 021: Printer Live Redirection][RFD 021]: swapping writers at runtime
  invalidates capabilities captured at construction.
- [\#1068]: the issue report this RFD starts from.
- `ssh -o BatchMode=yes`: prior art for failing fast rather than prompting.
- `git`, `sudo`, `fzf`: prior art for `/dev/tty` as the prompt channel.

[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 091]: 091-printer-owned-status-region.md
[\#1068]: https://github.com/dcdpr/jp/issues/1068
