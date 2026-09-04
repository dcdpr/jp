# RFD 104: Terminal Capability and Promptability Signals

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-04
- **Extends**: [RFD 048]
- **Required by**: [RFD 049], [RFD 091]

## Summary

JP answers four different questions with one boolean: whether stdout is a
terminal.
This RFD names the four questions, gives each its own source, moves channel
capability onto `Printer` where the channels already live, and completes the
`--interactive` / `--no-interactive` pair so a caller can both override and
assert promptability.

## Motivation

`jp conversation ls | rg fix` stopped and asked which workspace to use
([\#1068]).
The prompt came from the workspace precedence ladder ([RFD 087]), which runs
whenever stdin is a terminal — and piping stdout does not change stdin.

The reporter read that as "commands in a pipe must not ask".
This RFD does not adopt that rule: a piped command still has a user behind it,
and `jp query | less` may legitimately prompt.
What the report exposes is narrower and worse — JP had no way to say "do not
ask me", and the signal it did consult had nothing to do with whether anyone was
there.

That report is one symptom of a structural problem.
JP has, at various points, used three differently-derived signals to answer
questions that are not the same question:

| Signal                   | Derivation             | Consulted by                                      |
| ------------------------ | ---------------------- | ------------------------------------------------- |
| `Term::is_tty`           | `stdout.is_terminal()` | Tool permissions, editor confirmations, lock      |
|                          |                        | timeout, plugin approval, output format, spinners |
| `TargetEnv::interactive` | `stdin.is_terminal()`  | The workspace ladder and pickers                  |
| `Sink` construction      | `--format` is pretty   | ANSI stripping on both channels                   |

The failures follow from the mismatch, not from any one of them being wrong in
isolation:

- **A piped listing prompts.** stdout is not a terminal, so the `is_tty`
  consumers go quiet; stdin still is, so the ladder asks.
  [\#1068].
- **A repaint accumulates.** `jp query | less` resolves `--format` to `text`
  from stdout, so the `Sink` strips `\r\x1b[K` entirely — `StripSink::execute`
  keeps line feeds and drops every other C0 control, carriage return included.
  stderr is still the user's terminal, so instead of one retry line updating in
  place they get a new line per attempt.
- **A log file collects escapes.** `jp query 2> log.txt` has stdout on a
  terminal, so `--format` resolves to `text-pretty` and cursor control is
  written into the file verbatim.

[RFD 048] already decided the first of these:

> - `stdout.is_terminal()` → can the consumer handle ANSI escape codes?
> - `/dev/tty` available → can a user answer prompts?

The decision was merged; the code never implemented it.
`jp_printer::open_tty` opens `/dev/tty`, but only to give prompts somewhere to
draw, and `prompt_writer` falls back to `out` when the open fails — so the
availability that [RFD 048] designates as the promptability signal is computed
and then discarded.

Doing nothing keeps a class of bug that reappears with each new prompt or
progress affordance, because the next contributor reaches for the nearest
boolean and it is the wrong one.

## Design

### Four properties

Two channels, two properties each, plus one property of the session:

| Property                         | Question                             | Source                    |
| -------------------------------- | ------------------------------------ | ------------------------- |
| **stdout escape capability**     | Can the data consumer render ANSI?   | `--format`, defaulting to |
|                                  |                                      | `stdout.is_terminal()`    |
| **stderr escape capability**     | Can the chrome consumer render ANSI? | `--format`, defaulting to |
|                                  |                                      | `stderr.is_terminal()`    |
| **stderr cursor addressability** | Can chrome be repainted in place?    | `stderr.is_terminal()`    |
| **Promptability**                | Is a user present to answer?         | `/dev/tty`, overridable   |

#### `auto` resolves per channel

[RFD 048] resolves `--format auto` from stdout and applies the result to both
channels.
That is the conflation this RFD removes, one level up: it makes `jp query |
less` treat the user's terminal as unable to render escapes because *stdout* is
a pipe.

Under this RFD an explicit `--format` still forces both channels — 048's rule
is unchanged for every value a user can type.
`auto` resolves per channel instead: stdout from `stdout.is_terminal()`, stderr
from `stderr.is_terminal()`.
A piped-stdout run on a terminal gets `{escapes: false}` on stdout and
`{escapes: true}` on stderr, which is what each consumer can actually take.

#### Forcing, and what cannot be forced

Escape capability and cursor addressability are separate because their override
stories differ.
A user may legitimately ask for colour a channel cannot render —
`--color=always | less -R` is the canonical case, and `--format text-pretty` is
JP's spelling of it.
Nobody legitimately asks to repaint a file: `\r\x1b[K` written to a redirected
stream is not a preference, it is corruption.
So `--format` forces escape capability and has no bearing on addressability.

Cursor addressability is not user-toggleable in either direction.
Chrome repaints when the channel supports it and prints plainly when it does
not.

#### Repainting needs both

A repaint is written as escapes and lands on a channel that must be able to act
on them, so it requires the conjunction:

```txt
can_repaint = caps(Err).escapes && caps(Err).addressable
```

Addressability alone is not enough.
The `Sink` strips escapes whenever `escapes` is false, so a repaint emitted on
that basis is stripped on the way out and the line accumulates instead — the
second failure in the Motivation.
[RFD 091] reached the same conjunction independently for its status line.

Promptability is deliberately independent of both.
A piped `jp query | less` has a user behind it, and JP may ask.

### Promptability

Resolved once, at startup, in this order:

1. `--interactive` together with `--no-interactive` (or its `--non-interactive`
   alias) is a **usage error**.
   The two contradict, and silently letting one win hides the contradiction.
2. An explicit CLI flag decides: `--no-interactive` → **not promptable**;
   `--interactive` → **promptable**, or an error if `/dev/tty` cannot be opened
   for reading and writing.
3. `JP_NONINTERACTIVE` → **not promptable**.
4. Otherwise, whether `/dev/tty` can be opened for reading and writing.

The CLI outranks the environment, which is why steps 2 and 3 are separate.
A developer whose CI image or wrapper exports `JP_NONINTERACTIVE=1` and who adds
`--interactive` to one command is asking to be asked; resolving that to "not
promptable" would silently do the opposite of what they typed.
This makes promptability a tri-state request — automatic, forced on, forced off
— rather than a boolean the environment can `|=` into.

`/dev/tty` replaces `stdout.is_terminal()` as the derivation.
It is the controlling terminal, so it survives redirection of stdout and stderr
both, and it is absent when the process has none — cron, systemd, a daemonised
process, `ssh` without `-t`.
On Windows the pair is `CONOUT$` and `CONIN$`.

Availability is not the same as being able to *use* it: a process in a
background process group holds a controlling terminal but is stopped by
`SIGTTIN` if it reads from one.
See [Open Questions](#background-process-groups).

The two flags are asymmetric in kind, which is intended:

- `--no-interactive` **overrides** the derivation.
  There is no user; resolve without asking.
- `--interactive` **asserts** it.
  There must be a user; fail now if there is not.

The assertion exists because the default is a silent degradation.
Without a controlling terminal, JP proceeds unattended and resolves prompts on
the user's behalf — which is the right default, and the wrong outcome for a
script that meant to be asked.
`--interactive` converts that into a startup error, the way `ssh -o BatchMode`
converts the opposite case.

Neither flag makes JP prompt into a channel that cannot carry a prompt.

### Where each value lives

`Printer` already owns both channels, their sinks, and the ANSI-stripping
decision.
Channel capability belongs there rather than beside it:

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

`erase_line`, the waiting indicator, and the retry notice consult `can_repaint`
on `PrintTarget::Err`.
The `Sink`'s strip-or-pass decision becomes `caps(target).escapes`.

Promptability is resolved in the shell, before any workspace exists, because the
workspace ladder ([RFD 087]) is one of its first consumers.
It reaches pre-`Ctx` code as an argument — `TargetEnv` already takes it — and
`Ctx::Term` carries it for everything after.
There is one resolution and no re-derivation at a call site.

`prompt_writer`'s fallback to `out` is removed in the same change.
The fallback exists to guarantee a prompt renders somewhere, but a prompt in the
data channel is worse than no prompt, and once availability is a value the
caller has already decided not to prompt.

`Ctx::Term` keeps what is not a channel property:

```rust
pub(crate) struct Term {
    pub(crate) args: Globals,
    pub(crate) interactive: bool,
    pub(crate) width: Option<u16>,
}
```

`Term::is_tty` is removed rather than narrowed.
Every consumer wants one of the four properties by name, and a field called
`is_tty` invites the fifth reader to guess which.

### The rename sweep

Sixteen sites across four files name promptability `is_tty` — parameters,
struct fields, and the bindings that read them: `cmd/query/tool/coordinator.rs`
(six, including `let can_prompt = is_tty`), `cmd/plugin/dispatch.rs` (five),
`cmd/label/resolve.rs` (four), and `cmd/lock.rs` (one).
They are renamed to `interactive` in one sweep.

Leaving them is worse than not having split the field at all: a name that merely
overloads a word costs a reader one lookup, and a name that contradicts its
value costs them a wrong assumption.
`dispatch.rs`'s "run `jp <name>` in a terminal" guidance is corrected in the
same pass — being in a terminal stopped being the deciding factor.

The sweep is not uniform, because some values feed both kinds of consumer.
The migration is by behaviour, not by identifier:

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

`ToolCoordinator::execute_with_prompting` is the case that proves the point.
It took one flag and fed it both the permission decisions and the tool-progress
task, so re-pointing it at promptability silently removed progress from a
`--no-interactive` terminal run.
It takes both values.
`ToolRenderer`, lock waiting, and the pre-`Ctx` workspace prompts are checked
individually rather than renamed in bulk.

### Testing

`Printer::memory` is JP's stand-in for a printer whose behaviour a test
controls.
It gains capabilities as a constructor argument, defaulting to today's
behaviour:

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

No `Printer` trait is introduced.
There is one implementation and one test seam; a trait would be an abstraction
with no second caller.

### Overlap with RFD 091

[RFD 091] arrives at much of this section from the other direction.
Its enabling predicate is `can_repaint` under another name, its condition 2
already moves the tty source for chrome from stdout to stderr, and it already
proposes capability as a `Printer` constructor input with an override for the
memory constructor.

The two should not both own it.
This RFD owns channel capability as a general property; RFD 091's status line
becomes one client of `can_repaint` rather than a feature with its own
predicate. 091 keeps what is genuinely its own: the third condition, that a
tracing layer writing to stderr disables the line, since that is about a
competing writer rather than about what the channel can do.

One place where 091 is deliberately not followed: it states that `--format auto`
continues to resolve by stdout tty-ness per [RFD 048], and works around the
consequence in its predicate.
This RFD changes the resolution instead, which removes the workaround.

## Observable behaviour changes

The command from [\#1068] keeps its current behaviour by default, and gains a
way to change it:

```console
jp conversation ls | rg fix
# May prompt: a controlling terminal exists, so a user can answer.

jp --no-interactive conversation ls | rg fix
# Never prompts.
```

That is a deliberate choice, not an oversight.
Suppressing prompts because stdout is a pipe is the heuristic this RFD removes;
the fix for the report is the override, not a rule about pipes.

The rest are fixes, and each is visible:

- `jp query | less` — stderr repaints work.
  Today the escapes are stripped and each retry adds a line.
- `jp query 2> log.txt` — cursor escapes stop reaching the file.
- `jp query` from cron or `ssh` without `-t` — already unattended, but now for
  the stated reason rather than because stdout happens not to be a terminal.
- `jp query > out.txt` from a terminal — prompts still appear.
  Under the old derivation they were skipped, because redirecting stdout was
  read as "no user".

The last is the largest change in kind: a redirected-stdout run in a terminal
becomes interactive where it previously ran unattended.
That is the behaviour [RFD 048] specifies, and it matches `git`, `sudo`, and
`fzf`.

## Drawbacks

**`/dev/tty` is not portable in spelling.** The Windows equivalents behave
similarly but are opened differently and fail differently, so the promptability
probe needs per-platform code and per-platform testing.

**Opening `/dev/tty` for reading is a new side effect at startup.** It is cheap
and it is what the prompt path needs anyway, but it is one more thing that can
fail in a sandbox.

**Four properties are more to hold than one boolean.** The reader who only wants
"am I on a terminal" now has to know which terminal and for what.
The cost is real, and it is the cost of the questions actually being different.

**Per-channel `auto` deviates from [RFD 048].** 048 resolves one format from
stdout and applies it to both channels, and this RFD keeps that only for
explicit values.
A reader who knows 048 will find `--format auto` behaving differently per
channel surprising until they reach this document, and 048 needs a note pointing
here.

**The rename sweep touches sixteen sites in four files,** and is not purely
mechanical: each has to be classified before it is renamed, and at least one
feeds both a prompt decision and a rendering one.
It is a wide diff over code that is otherwise stable.

## Alternatives

### Keep `stdout.is_terminal()` and add flags for the exceptions

Leave the derivation alone; let `--no-interactive` and a hypothetical
`--assume-tty` paper over the cases it gets wrong.
Rejected: it makes every user who pipes stdout learn a flag to restore the
behaviour they had, and it leaves the next affordance to pick the wrong signal
again.

### Derive promptability from stdin

The workspace ladder already does this, and it is right more often than stdout
is.
Rejected: it breaks `jp query < prompt.txt`, where stdin is a file and the user
is still at the keyboard.
`/dev/tty` is the only source that survives redirection of any standard stream.

### One `Terminal` type owning all four properties

Fold channel capability and promptability into a single struct passed
everywhere.
Rejected: it re-creates the coupling this RFD removes, one indirection further
out.
Channel capability belongs to the thing that owns the channels; promptability
belongs to the session.

### Introduce a `Printer` trait for testing

Rejected as a midlayer: one implementation, no second caller, and the test seam
`Printer::memory` already provides is sufficient once it takes capabilities.

## Non-Goals

- **The detached prompt policy.** What JP does *instead of* prompting when there
  is no user — auto-approve, use defaults, or fail — is [RFD 049].
  This RFD supplies the signal that policy branches on and takes no position on
  the branches.
- **The `Prompt` enum.** Typed prompt routing is [RFD 018].
- **Whether the workspace conflict prompt should exist.** That is [RFD 087]'s
  design; this RFD only fixes when it fires.
- **A toggle for cursor addressability.** Repaints happen when the channel
  supports them.
  No flag either way.
- **The shape of chrome records under `--format json`.** Whether a consumer can
  distinguish chrome from data in a merged stream is a machine-output concern,
  not a capability concern.
- **A typed prompt transport.** Several prompts build on a bare `io::stderr()`
  rather than the printer's prompt channel, so a promptable run can still put a
  question somewhere the user is not looking.
  Removing `prompt_writer`'s fallback closes the case where JP has the signal
  and ignores it; routing every prompt through one transport is [RFD 018].

## Risks and Open Questions

### `/dev/tty` availability in containers and multiplexers

The probe is correct in the environments it was designed for.
Docker without `-t`, some CI runners, and remote agent harnesses are less
predictable, and a false negative there turns an interactive session unattended.
`--interactive` gives the user a way to find out immediately rather than by
inspecting the outcome, which bounds the damage but does not remove the risk.
Implementation should test these environments explicitly rather than reason
about them.

### The cost of probing at startup

Every invocation opens `/dev/tty`, including the ones that never prompt.
[RFD 048] made the prompt writer lazy for this reason.
If the open proves measurable, the probe can be deferred behind a `OnceCell`, at
the cost of promptability no longer being a plain value resolved in the shell.

### Background process groups

A process in a background process group holds a controlling terminal, so the
`/dev/tty` probe succeeds, but reading from it raises `SIGTTIN` and the default
action stops the process.
`jp query "…" &` therefore passes `--interactive`'s assertion and then suspends
at the first prompt.
The shell reports the stopped job, so the damage is visible and contained, but
the assertion promises more than it can deliver.

Two ways out: compare `tcgetpgrp(tty_fd)` with the process group when deriving
promptability, which is Unix-only and untested against the multiplexers below;
or narrow the contract to "a controlling-terminal endpoint exists" and let the
first prompt fail at runtime with a typed error.
Unresolved.

### Capability and writer destination must change together

[RFD 021] proposes swapping a printer's `out` and `err` writers at runtime.
Capability captured at construction goes stale the moment a writer is replaced
by one with different properties.
Whichever lands second owes the other an atomic swap of destination and
capability.

### `--interactive` on a partially available terminal

`/dev/tty` can in principle open for writing but not reading.
The proposal requires both and errors otherwise, which is the conservative
reading; whether any real environment produces that state is unknown.

## Implementation Plan

### Phase 1: Channel capability on `Printer`

Introduce `ChannelCaps` and `Printer::caps`.
Resolve `--format auto` per channel, keep explicit formats forcing both, and
derive `addressable` from `stderr.is_terminal()`.
Move the `Sink` strip decision onto `caps(target).escapes`.
Point `erase_line`, `spawn_line_timer`, and the retry notice at `can_repaint`
instead of `--format` and `is_tty`.
Add `Printer::memory_with`.

Fixes the accumulating-retry-line and log-file cases on its own.
Can be merged independently.

### Phase 2: Promptability from `/dev/tty`

Add the read side to `open_tty`, expose availability as a value rather than
absorbing it in `prompt_writer`'s fallback, and resolve promptability from it in
the shell.
Add `--interactive` with assert semantics.
Remove `Term::is_tty`.

Depends on Phase 1 for the `Printer`-side capability that the remaining `is_tty`
readers are re-pointed at.

### Phase 3: The rename sweep

Rename the sixteen `is_tty` sites to `interactive`, update their doc comments,
and correct `dispatch.rs`'s terminal-specific guidance.

Mostly mechanical, but not entirely: each site is classified against the
migration table above first, and any that feeds both a prompt decision and a
rendering one is split before it is renamed.
Depends on Phase 2, and is deliberately separate so the behavioural changes and
the mechanical ones bisect apart.

## References

- [RFD 048: Four-Channel Output Model][RFD 048] — defines the channels and
  specifies the stdout-versus-`/dev/tty` split this RFD implements.
- [RFD 049: Non-Interactive Mode and Detached Prompt Policy][RFD 049] — the
  detached prompt policy, which consumes this RFD's signal.
- [RFD 087: Session-Scoped Active Workspace][RFD 087] — its precedence ladder
  defers to "the same promptability signal JP already uses elsewhere"; this is
  that signal.
- [RFD 091: Printer-Owned Status Line][RFD 091] — its enabling predicate is
  this RFD's `can_repaint`, reached independently.
  See [Overlap with RFD 091](#overlap-with-rfd-091).
- [RFD 021: Printer Live Redirection][RFD 021] — swapping writers invalidates
  capability captured at construction.
- [\#1068] — the report this RFD starts from.
- `ssh -o BatchMode=yes` — prior art for failing rather than prompting.
- `git`, `sudo`, `fzf` — prior art for `/dev/tty` as the prompt channel.

[RFD 018]: 018-typed-prompt-routing-enum.md
[RFD 021]: 021-printer-live-redirection.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 087]: 087-session-scoped-active-workspace.md
[RFD 091]: 091-printer-owned-status-line.md
[\#1068]: https://github.com/dcdpr/jp/issues/1068
