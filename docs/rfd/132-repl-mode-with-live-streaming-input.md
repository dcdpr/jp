# RFD 132: REPL Mode with Live Streaming Input

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-26
- **Extends**: [RFD 088], [RFD 091], [RFD 131]
- **Requires**: [RFD 104]
- **Summary**: Adds `jp q --repl`, a multi-turn run with a live reedline input
  field pinned below streaming output.

## Summary

`jp q --repl` runs `jp query` as a REPL: when a turn finishes, an input field
remains on screen to accept the next query instead of exiting the process.
The input field also stays visible while a turn runs, pinned to the bottom of
the terminal as output scrolls above it into native scrollback, so the user can
type a message and press `Enter` to redirect the assistant without pressing
`Ctrl+C` first.

The REPL builds on the turn loop from [RFD 131] and adds two independent
configuration settings: `query.loop = "always"`, which waits in the input field
for the next query at the end of every turn, and `query.live_input`, which keeps
the input field visible while turns run.
Passing `--repl` enables both settings for that invocation.
The input field uses reedline's editor and painter, rendered inside the
printer's status region ([RFD 091]) on the controlling terminal alongside the
status row and any active prompts.
When no input field is active, the only behavior change is routing live stderr
tracing through the printer.

## Motivation

Two points of friction slow down iterative `jp q` workflows:

1. **Re-running `jp q` for every turn.** Each follow-up query pays the cost of
   process startup, configuration loading, and query composition.
2. **Redirecting a streaming response requires a multi-step detour.** Today, the
   user must press `Ctrl+C`, wait for the interrupt menu, select `r`, and then
   type their reply ([RFD 088]).
   Keeping an input field on screen reduces that to typing a reply and pressing
   `Enter`.

[RFD 131] introduces the multi-turn loop and reserves `query.loop = "always"`
for REPL mode, but it cannot accept input while a turn is running.
Reedline's `read_line` (the blocking loop behind existing prompts) owns the
keyboard until submission and cannot run as one branch of the streaming `select`
loop.
Supporting live input requires a polled entry point into the same reedline
engine.
Using that same engine and painter for the idle prompt between turns guarantees
that the input field looks and behaves identically whether a turn is running or
idle.

## Design

### User-Facing Behavior

`jp q --repl` composes and sends the initial query as usual and executes the
first turn.
When that turn ends, the input field waits for the next query, and the run
continues until the user exits.

Two orthogonal settings control this behavior:

```toml
[query]
loop = "always"   # never | if_pending | always
live_input = true # show the field while turns run
```

- `query.loop` ([RFD 131]) controls whether the run continues after a turn ends.
  This RFD adds the `always` variant, which waits in the input field for the
  next query.
- `query.live_input` controls whether the input field stays on screen while a
  turn runs so the user can type ahead or redirect the assistant.
  Defaults to `false`.

Passing `--repl` sets `loop = "always"` and `live_input = true` for the current
invocation.
Like `--edit`, `--repl` is excluded from `apply_cli_config` and is not recorded
in the conversation's config deltas, so a subsequent bare `jp q` on the same
conversation does not enter REPL mode.
By contrast, `--cfg query.live_input=true` is recorded like any other `--cfg`
override.

Text typed into the live input field counts as pending input alongside a queued
reply ([RFD 131]).
When `query.loop = "if_pending"` and `live_input = true`, the run continues at
turn end if the input field contains text, and exits if both the queue slot and
the input field are empty.
All six combinations of the two settings have distinct behavior:

| `query.loop`           | `live_input = false` (default)                           | `live_input = true`                                                                                             |
| ---------------------- | -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- |
| `never`                | Single-turn `jp q`.                                      | Type while the turn runs; when the turn ends, the run exits and any unsent text in the field is saved as draft. |
| `if_pending` (default) | Single-turn `jp q`, continuing only for queued replies.  | Continue at turn end while either a queued reply or text in the input field is pending.                         |
| `always`               | REPL mode with the input field shown only between turns. | Full REPL mode with the input field always on screen (equivalent to `--repl`).                                  |

When `live_input = false`, the input field and raw terminal mode are active only
while waiting between turns; during a turn, keystrokes and `Ctrl+C` behave as in
a standard `jp q` run.

Both settings take effect only in interactive runs (see
[Requirements](#requirements)).
In a non-interactive run, `always` degrades to `if_pending` and the input field
remains disabled, whereas passing `--repl` explicitly fails before the first
turn starts.
Enabling `live_input = true` by default would place every interactive `jp q` in
raw terminal mode, so `live_input` defaults to `false`.

Whether the live input field is active during turns is decided once at the start
of the run from `--repl` or the first turn's configuration.
Per-turn config changes on subsequent turns can update `query.loop` (changing
when the run exits), but cannot toggle `live_input` mid-run.

Keybindings in the input field depend on the current phase because the
underlying turn transitions differ: replying during LLM streaming continues the
current turn, whereas submitting during tool execution must cancel the running
tools first.
Every keybinding reuses an existing interrupt-menu transition.
The streaming and tool-execution columns apply when `live_input = true`:

| Key                                        | Idle (between turns)                                            | LLM streaming                                                               | Tool execution                                                                                                   |
| ------------------------------------------ | --------------------------------------------------------------- | --------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------- |
| `Enter` (non-empty)                        | Send as the next turn.                                          | Stop the response and reply **within the current turn** (the menu's Reply). | Stop & respond: cancel the running tools; the buffer becomes their response, and the follow-up request proceeds. |
| `Enter` (empty)                            | No-op.                                                          | No-op.                                                                      | No-op.                                                                                                           |
| `Ctrl+S`                                   | No-op.                                                          | Stop: end the turn, keep partial content, return to idle.                   | Stop: cancel the tools, record their cancellation responses, end the turn, return to idle.                       |
| `Ctrl+X`                                   | External editor on the query document (see `Ctrl+X` below).     | External editor seeded with the buffer.                                     | Same as streaming.                                                                                               |
| `Shift+Enter` / `Alt+Enter`                | Insert a newline.                                               | Same.                                                                       | Same.                                                                                                            |
| `Ctrl+A`, `Ctrl+E`, `Ctrl+U`, kill-ring, … | [RFD 088]'s line editing, unchanged.                        | Same.                                                                       | Same.                                                                                                            |
| `Ctrl+D`                                   | End the run when the buffer is empty; forward-delete otherwise. | Forward-delete.                                                             | Forward-delete.                                                                                                  |
| `Ctrl+C`                                   | See [Interrupts](#interrupts-one-ladder-two-entry-points).      | Same.                                                                       | Same.                                                                                                            |

`Enter` therefore triggers a different transition in each phase.
Making `Enter` always start a new turn requires a separate design (tracked in
T-1f6wn02); until then, the input field reuses the existing phase transitions.

There is no Abort keybinding: [RFD 092] removes Abort from the interrupt menus
because streamed events are persisted as they commit.

**Turn completion and error recovery.** When a turn completes, is stopped via
`Ctrl+S` or `[s] Stop`, or fails with a recoverable error, the run returns to
the idle input field if `query.loop = "always"`, or if `query.loop =
"if_pending"` and the input field holds text; otherwise, the run exits.
If a queued reply ([RFD 131]) is pending, its countdown runs and sends that
reply before returning to the idle input field.
Interrupt escalation and fatal run errors exit the run immediately, as in [RFD
131].

When a turn fails after its `ChatRequest` has been committed to the conversation
stream (for example, when provider transport retries are exhausted), the failed
turn remains in the conversation and the error is printed on the chrome channel;
the submitted query is not restored to the input field because it is already
recorded in the turn.
Submitted text returns to the input field only when a failure occurs before the
request is committed to the stream (such as a malformed draft preamble or an
external editor that fails to spawn).
Authentication, config resolution, persistence, and terminal errors are fatal
and exit the run with a non-zero status.
`cmd::Error` gains an explicit recoverable-versus-fatal classification rather
than relying on ad-hoc matching at the call site.
Tool execution failures are not turn errors: a failed tool returns `Err` to the
assistant as part of the turn.

**Unsent text in the input field.** Ending a turn never clears or moves text
already typed into the input field while the run continues: under `always`, the
buffer stays in the idle field, and under `if_pending`, the non-empty buffer is
what keeps the run going.
If the input field still contains text when the run exits gracefully (at turn
end under `never`, via Quit in the fallback menu, on a second `Ctrl+C`, or on
`SIGTERM`), that text is persisted to `QUERY_MESSAGE.md` under [RFD 131]'s draft
rules, appended after a blank line if a queued reply is already stored in the
draft.
A subsequent bare `jp q` seeds from that draft, and a direct `jp q <query>`
triggers [RFD 131]'s unsent-draft prompt.
Following [RFD 131], no graceful exit path discards user-authored text.
Immediate termination (a third `Ctrl+C` or `SIGQUIT`) performs no file I/O and
loses the unpersisted buffer.

**`Ctrl+X` (external editor).** During an active turn, pressing `Ctrl+X` opens
the external editor with only the input field's buffer, matching the `Ctrl+X`
escape in the inline reply widget ([RFD 088]): saving updates the input field's
buffer, and cancelling leaves the buffer unchanged.
When idle between turns, the buffer represents the next turn's query, so
`Ctrl+X` opens the conversation's full `QUERY_MESSAGE.md` `QueryDocument`
(including the config preamble, just like `jp q --edit`).
To avoid losing an existing draft, the document's query body is seeded with any
existing `QUERY_MESSAGE.md` text (from an earlier failed run or a queued reply
kept with `[k]`), a blank line, and the input field's current buffer, matching
`[e]` in [RFD 131].
Saving in the external editor loads the edited query body back into the input
field; when the user submits that text, the edited config preamble is recorded
as a conversation config delta (matching `jp q --edit`) and the draft is
consumed.
Cancelling the external editor leaves both the input field and
`QUERY_MESSAGE.md` unchanged.
Once `Ctrl+X` has merged the existing draft into the input field, a graceful
exit writes the input field's buffer over the draft's query body rather than
appending it a second time.

Configuration resolution, CLI argument lifetimes, background-task draining, and
conversation locking follow [RFD 131] without change: each turn resolves its own
config epoch.

### Shared Engine and Polled Line Editor

The input field and the existing inline reply widget share a single reedline
implementation:

- **One engine**: reedline's editor state (`LineBuffer`, emacs and vi keymaps
  and mode state machines, undo/redo stacks, and kill-ring).
- **One painter**: reedline's `Painter`, which renders the prompt and buffer.
- **Two drivers**: a blocking driver that reads keyboard input until completion
  (used by the idle input field between turns), and a polled driver that accepts
  individual keyboard events as one branch of the turn's `select!` loop (used by
  the live input field during turns).
  Because both drivers step the same editor state and draw through the same
  painter, the input field behaves identically in both modes.

Existing standalone prompts continue using `Reedline::read_line`.
Both `read_line` and the polled interface drive the same reedline engine and
painter, so visual and behavioral parity is maintained inside the reedline fork
rather than duplicated in JP.

### Polled Editor Interface

JP cannot drive reedline's inner `Editor` struct directly: keymap lookup, the vi
and emacs mode machines, event batching, and bracketed-paste handling all live
in `Reedline` and `EditMode` (`read_line_helper`, `process_input_batch`, and
`handle_editor_event` in the fork's `engine.rs`), and bypassing them would force
JP to reimplement that input layer.

Instead, the reedline fork exposes a **polled line editor** API on `Reedline`
with five operations:

- Enter raw mode and initialize prompt state.
- Process a single terminal event through the existing edit-mode and event
  machinery.
- Repaint at a caller-supplied row origin and return the cursor's target
  coordinates (caret position).
- Suspend and resume while preserving the buffer.
- Finish and return an outcome (submit, cancel, or host command).

Both the blocking driver and the polled driver use this API.
`jp_inquire` wraps the polled editor in an input-field abstraction and provides
a mock implementation for tests.

### Rendering the Input Field in the Status Region

[RFD 091] introduced the status-region primitive that this design builds on: a
block of rows at the bottom of the terminal drawn by the printer's worker and
erased before every printer-managed write, complete with relative row
accounting, per-draw terminal size measurement, width truncation, row
backgrounds, a claim stack, and suspension for external writers.
[RFD 120]'s reasoning window already streams into that region.
The printer worker remains the single serialized terminal writer ([RFD 048]);
this RFD extends the status region to support editable, cursor-owning rows.

**Output target.** [RFD 091] draws status regions on stderr as chrome.
When an input field is active, the status region renders on the controlling
terminal instead: interactive input belongs on `/dev/tty` ([RFD 048]), and the
status row and prompts render alongside the input field because the region is a
single contiguous block of rows.
The status region therefore gains a configurable output target (`stderr` by
default), switched via a worker command that erases the region on the previous
target and redraws it on the new one.
When `live_input = true`, the region stays on the controlling terminal for the
entire run; when `live_input = false`, it switches to the controlling terminal
only while the idle input field waits between turns.
This target applies to every active region claim, including [RFD 120]'s
reasoning window.
The `stderr` target retains [RFD 091]'s enabling predicate, whereas the
controlling-terminal target requires only an interactive run.
The worker erases and redraws the region around a write only when that write's
stream reaches the terminal screen (see [Requirements](#requirements)); writes
redirected to a regular file neither move the screen cursor nor leave a line
open on the terminal.

**Layout.** When `live_input = true`, the region renders from top to bottom: any
active prompt rows (see [Prompts Inside the
Region](#prompts-inside-the-region)), the top claim's output window and status
row ([RFD 091]), and the input field:

```
┌──────────────────────────────────────────┐
│ assistant output, tool chrome            │ ← scrolls into native scrollback
├──────────────────────────────────────────┤ ← erased before, redrawn after each write
│ Run tool fs_modify_file? [y,Y,n,N,…]     │ ← an open prompt
│ ⏱ Waiting… 9.2s                          │ ← the top claim's status row
│ › the user's live input                  │ ← the field
└──────────────────────────────────────────┘
```

Terminal rows are allocated to the input field first (at least one row), then to
any active prompt, and then to the status row; [RFD 091]'s preview window yields
rows before any of those three.

**Cursor ownership.** Unlike a passive status row, the input field and an active
prompt own the terminal cursor (caret).
The polled line editor renders at the row origin supplied by the worker and
returns its caret coordinates (which reedline already computes to position the
cursor).
After rendering each frame, the worker positions the terminal cursor at those
coordinates using relative cursor moves.
Before writing persistent content, the worker hides the cursor, moves to the
region's bottom row, erases the region as in [RFD 091], writes the content,
redraws the region, and restores the cursor to the caret position.

**Coexisting with an unfinished content line.** In [RFD 091], the worker hides
the status region whenever persistent output leaves a line open without a
trailing newline (`content_open`), because drawing the region at column 0 would
overwrite that unfinished line, and multi-batch typewriter tasks keep the region
hidden until the task completes.
While brief hiding is barely noticeable for a timer, hiding the input field on
every streaming line would make the field flicker out of view while the user is
typing.
Instead, interactive rows render starting one row below the unfinished content
line.
Subsequent chunks that continue the unfinished line move the cursor up and back
to append in place without touching the region; the worker erases and redraws
the region only when the content line finishes with a newline or wraps to the
next row.
Typewriter tasks yield between batches to allow region repaints (the refinement
noted in [RFD 091]), so typed characters echo promptly while a long line streams
out.

**Prompt suspension.** [RFD 091] suspends the status region whenever a caller
acquires a prompt writer.
When `live_input = true`, prompts render inside the status region itself rather
than acquiring an external prompt writer, so the status row and input field stay
visible.
The region suspends only for the external editor and for prompt types not yet
hosted in the region (`select` and `text`).

**Terminal state recovery.** Region repaints execute as atomic worker commands,
cursor movements are emitted only to the controlling terminal, and after writing
untrusted content the worker re-asserts cursor visibility, scroll margins, and
the terminal modes owned by the terminal lease.
The worker cannot undo destructive escape sequences such as scrollback erasure
or alternate-screen switches emitted in unsanitized output; [RFD 096] mitigates
those sequences independently and is not a prerequisite here.

**Bounding redraw churn.** The worker coalesces region repaints to at most one
repaint per batch of queued writes and wraps redraws in synchronized-output
sequences (BSU/ESU) on terminals that support them to prevent flicker.

Changes to reedline's painter are limited to rendering bottom-anchored at the
worker's row origin, reporting caret coordinates, and never scrolling the
terminal directly.
Terminal reflow artifacts when narrowing the window (T-1d23tmt) affect these
rows the same way they affect other status-region rows and do not block this
design.

### Prompts Inside the Region

All prompts in a turn already route through a single trait boundary: the turn
receives an `Arc<dyn PromptBackend>`, which both the tool prompter and the
interrupt handler call.
When `live_input = true`, the run supplies a region-backed `PromptBackend`
implementation, requiring no changes at prompt call sites:

- `inline_select` (permission prompts and interrupt menus) and `inline_reply`
  (mid-turn replies and tool argument or result edits) render in the status
  region's prompt rows.
  `inline_reply` reuses the polled line editor, giving it the same appearance
  and editing behavior as the input field.
- `select` and `text` suspend the status region and use the existing terminal
  widgets.
  During a turn, only tool questions with fixed option lists use `select`;
  moving `select` into the region is deferred to future work.
- The region backend returns the exact same outcome values for each keystroke as
  the terminal backend, including [RFD 092]'s separation of `Interrupted` from
  `Cancelled`.
- When a prompt is answered, its region rows are removed and the backend writes
  the same one-line answered summary that the terminal widget writes today,
  preserving scrollback output.
- While a prompt is open, it owns keyboard input; the input field remains drawn
  below it in an inactive state with its buffer preserved.

When `live_input = false`, the run uses the standard terminal `PromptBackend`,
since the input field is active only between turns when no other prompts open.
Using the region backend whenever a status region is active would alter prompt
rendering across default single-turn runs and is left as future work.

### Input Ownership and the Event Pump

Enabling raw terminal mode prevents the terminal driver from translating
`Ctrl+C` into `SIGINT`: while the input field is active, `Ctrl+C` arrives as raw
byte `0x03` on the input stream, and if nothing reads that stream, interrupts
stop working.
When `live_input = false`, raw mode is active only while the idle input field
waits between turns, and the blocking driver reads every keystroke and forwards
`0x03` to `SignalRouter` directly.
When `live_input = true`, a run-scoped **event pump** reads terminal events for
the lifetime of the run and routes them to the active owner:

| Owner  | When                                | Behavior                                                                                         |
| ------ | ----------------------------------- | ------------------------------------------------------------------------------------------------ |
| Field  | idle, LLM streaming, tool execution | Keys step the polled line editor; `0x03` is forwarded to the `SignalRouter`.                     |
| Prompt | a prompt is open                    | The prompt takes the events and handles `Ctrl+C` per its own contract.                           |
| Pump   | phases with neither                 | `0x03` is forwarded immediately; other keys queue (bounded) and reach the field when it resumes. |

Because the input field is ambient input rather than a modal prompt, it never
consumes `Ctrl+C` itself.
`0x03` bytes bypass the input queue and are forwarded immediately so a large
paste cannot delay an interrupt.
If the bounded key queue fills during an ownerless phase, the pump drops the
newest keystrokes, rings the terminal bell, and prints a chrome notice (dropping
the oldest keystrokes would corrupt the middle of typed text).
Queued keystrokes are interpreted when delivered to the input field: a reply
typed during provider connection setup stops the stream once streaming begins,
matching the user's intent.

| Phase                                                      | Field           | Status row             | Keyboard           |
| ---------------------------------------------------------- | --------------- | ---------------------- | ------------------ |
| Idle                                                       | live            | when a claim is active | field              |
| LLM streaming                                              | live            | visible                | field              |
| Tool execution                                             | live            | visible                | field              |
| A region-hosted prompt                                     | drawn, inactive | visible                | prompt             |
| A `select` or `text` prompt                                | suspended       | suspended              | prompt             |
| Retry backoff, provider setup, persistence, thread rebuild | drawn, inactive | visible                | pump               |
| External editor                                            | suspended       | suspended              | the editor process |

Leaving and re-entering raw mode around each ownerless phase was rejected
because mode transitions race with in-flight keystrokes and thrash during short,
frequent phase transitions.

### Interrupts: One Ladder, Two Entry Points

[RFD 092] defines what `Ctrl+C` does across JP; this RFD adds a second delivery
path for that keystroke.
While raw mode is active, the input field or the event pump forwards `0x03` to
the same `SignalRouter` that receives `SIGINT` outside raw mode, so the
escalation ladder ([RFD 045], refined by [RFD 092]) behaves identically
regardless of how `Ctrl+C` arrived.

**Which presses count toward escalation.** `SignalRouter` increments its
escalation counter only when `Ctrl+C` arrives while no modal prompt owns the
keyboard (either as `SIGINT` outside raw mode, or as `0x03` forwarded by the
input field or event pump).
A `Ctrl+C` read by an open prompt is not counted by the router.
Instead, the prompt returns an `Interrupted` outcome, and the caller hosting the
prompt (the interrupt handler or tool prompter) applies [RFD 092]'s context
rules: on an interrupt menu it escalates to rung 2, in a reply opened from an
interrupt menu it returns to that menu, and in any other prompt it swaps to the
current scope's interrupt menu.
No per-prompt configuration is needed.
Pressing `Ctrl+C` twice inside a nested prompt backs out one level per press,
exiting the run only when a press lands on the interrupt menu itself.

**Idle behavior.** Pressing `Ctrl+C` in the idle input field forwards `0x03` to
the router and opens [RFD 092]'s fallback menu (`[c] Continue`, `[q] Quit
gracefully`) regardless of whether the buffer contains text; `Ctrl+U` clears the
line.
Until [RFD 092] lands, an unhandled first `Ctrl+C` triggers a graceful shutdown,
persisting any unsent buffer text to `QUERY_MESSAGE.md`.

**Clean exit.** Pressing `Ctrl+D` on an empty idle buffer exits the run cleanly.

**Terminal lease.** A single process-wide terminal lease tracks terminal-mode
state (raw mode, bracketed paste, keyboard-enhancement flags, cursor visibility,
and scroll margins) and exposes an idempotent `emergency_reset()` method.
`emergency_reset()` runs on every immediate-exit path (today's
`std::process::exit` in `SignalRouter`, and [RFD 092]'s `SIGINT` re-raise,
neither of which runs Rust destructors).
The lease also suspends and restores terminal state around external editor
invocations.
`Drop` remains the normal cleanup path, while `emergency_reset()` guarantees
restoration on forced exits.

### Routing Live Tracing Through the Printer

The opt-in stderr tracing layer (`-v`, `--log`, `--log-file=-`) currently writes
directly to stderr, which is why [RFD 091] disables status regions whenever
stderr logging is active (`TerminalCapability::with_stderr_logging`).
Because a REPL cannot disable its input field when `-v` is passed, the stderr
tracing layer is routed through a printer-backed writer: each trace event is
queued as an `Err` print task so the worker erases and redraws the status region
around it like any other chrome output.
Two safeguards preserve the reliability of tracing:

- `jp_printer`'s own tracing targets remain file-only, preventing recursive
  logging if the printer itself fails.
- The log-file tracing layer continues writing directly to disk, preserving a
  complete trace even if the printer worker hangs or the process exits abruptly.

Once every terminal writer is serialized through the printer worker, [RFD 091]'s
stderr-logging restriction is removed, so status regions also render under `-v`
outside REPL mode.

### Requirements

A run hosts the input field only when the session is interactive, using the same
promptability signal as every prompt ([RFD 104]): a controlling terminal is
available and neither `--no-interactive` nor `JP_NONINTERACTIVE` is set.
Redirecting or piping stdout does not disable promptability, and no other
condition governs whether the input field can be shown.

Passing `--repl` in a non-interactive session fails before the first turn
starts, matching `jp q --edit` and `--quote`, with an error message naming the
cause (`--no-interactive`, `JP_NONINTERACTIVE`, or no controlling terminal).
This check runs after `JP_NONINTERACTIVE` is folded into the flag so a single
check covers all three cases (a clap conflict with `--no-interactive` would
catch only the explicit CLI flag).
By contrast, setting `query.loop = "always"` or `query.live_input = true` in
configuration never fails a non-interactive run: `always` degrades to
`if_pending` and the input field stays disabled.

Today, interactivity checks whether stdout is a terminal, treating redirected
stdout as non-interactive; [RFD 104] (a prerequisite for this RFD) moves that
check to the controlling terminal.

The `--format` flag does not affect the input field.
The status region renders on the controlling terminal, where ANSI escape
sequences are never stripped ([RFD 048]), while `--format` controls how stdout
content and stderr chrome are encoded (matching [RFD 092]'s separation between
interactivity and pretty output).

Maintaining accurate row accounting requires the printer worker to know which
writes reach the terminal screen.
At startup, JP classifies stdout and stderr by checking `is_terminal` first and
then inspecting the file descriptor type (`is_fifo` and `is_socket` on Unix,
`GetFileType` on Windows):

- **Terminal**: the worker erases the status region before each write and
  redraws it afterward.
- **Regular file or device (such as `/dev/null`)**: output never reaches the
  terminal screen, so the worker writes without erasing or redrawing the region.
- **Pipe or socket**: JP's own writes do not reach the screen and are written
  without erasing the region, but the downstream process reading the pipe may
  write to the same terminal at any time.

`is_terminal` runs before file-type inspection because MSYS terminals on Windows
(mintty and Git Bash) are implemented as named pipes that Rust's `is_terminal`
already detects.

Most programs piped after a REPL write to the terminal so the user can see the
output (`tee`, `cat`, `jq`, `bat`, or a pager).
For example, `tee` writes each chunk at the current cursor position (inside the
input field), causing the next region redraw to erase from the wrong row, while
a pager also competes for keystrokes and switches to the alternate screen.
Programs that do not write to the terminal (`| gzip > log.gz` or `| pbcopy`)
work cleanly and keep output off the screen just like `> file`.

Because JP cannot tell whether the downstream reader writes to the terminal, a
run with an active input field (`always` or `live_input`) and a piped stdout or
stderr prints a one-time warning on the controlling terminal per piped stream
before the first turn and then proceeds:

```
⚠ stdout is a pipe. A program reading it that writes to this terminal (tee,
  cat, a pager) garbles the screen around the input field; the conversation
  is unaffected. `> file`, or `jp conversation print` afterwards, avoids it.
```

Piping stdout or stderr never disables the input field automatically.
Inferring non-interactivity from a piped stdout is the heuristic [RFD 104]
removes from prompts; scripts that want non-interactive behavior pass
`--no-interactive`.

| Invocation                                                                   | Result                                                                             |
| ---------------------------------------------------------------------------- | ---------------------------------------------------------------------------------- |
| `jp q --repl`                                                                | Works.                                                                             |
| `jp q --repl > answer.md`                                                    | Works; answers go to the file, as with `jp q > answer.md`.                         |
| `jp q --repl 2> chrome.log`                                                  | Works; chrome goes to the file, and the field and status row stay on the terminal. |
| `jp q --repl --format json`                                                  | Works; content and chrome are NDJSON, and the field is unchanged.                  |
| `cat prompt.md \| jp q --repl`                                               | Works; stdin seeds the first query, and keys are read from the terminal.           |
| `jp q --repl \| gzip > log.gz`                                               | Works, after the pipe notice.                                                      |
| `jp q --repl \| tee log.md`                                                  | Runs after the pipe notice; `tee`'s echo garbles the screen.                       |
| `jp q --repl` with `--no-interactive`, `JP_NONINTERACTIVE=1`, or no terminal | Fails before the first turn.                                                       |
| `jp q "…" > out.md` from a terminal, with `always` configured                | Hosts the field and waits for the next query, like any interactive run.            |
| The same with `--no-interactive`                                             | Acts as `if_pending`: the turn runs and the run ends.                              |

If the terminal height is too small to fit both persistent content and the
status region, the input field suspends while a turn runs and reappears between
turns.

### Naming

A **run** ([RFD 131]) is a single `jp` invocation executing one or more turns,
and a **REPL** is a run that waits for the next query at the end of every turn
(`query.loop = "always"`).
The **input field** is the line editor where that query is composed, and when
`query.live_input = true` it remains on screen while turns run.
Both **REPL** and **Input Field** are added to the ubiquitous-language glossary.

## Drawbacks

- **Raw terminal mode for the duration of the field**, which under `live_input`
  spans the entire run.
  Restoring the terminal on forced exits depends on every exit path calling the
  terminal lease's `emergency_reset()`.
- **An always-on event pump** when `live_input = true`.
  Keyboard routing requires a run-scoped task whose ownership table must stay
  synchronized across every turn phase transition.
- **Additional configuration surface.** Adding `query.live_input` alongside
  `query.loop` creates six configuration combinations to document, though each
  combination represents a distinct user-facing mode.
- **Redraw churn.** Every persistent content write erases and redraws the status
  region, which can cause visible flicker on slow terminals without synchronized
  output.
- **Interactive status region complexity.** The input field, prompts, and status
  row all share the status-region primitive, so a bug in region row accounting
  affects all three surfaces.
- **Two prompt backends to maintain in lockstep.** The region `PromptBackend`
  must return the exact same outcomes as the terminal `PromptBackend` for every
  keystroke.
- **Queued stderr trace output.** Routing live stderr tracing through the
  printer worker means stderr trace lines share the worker queue and can lag
  behind content, though log-file writes remain immediate.
- **Larger reedline fork surface.** The fork gains the polled line editor API
  and bottom-anchored painter on top of [RFD 088]'s I/O patches.
- **Unprotected terminal output from pipe readers.** Because JP cannot inspect
  what a downstream pipe reader does, piping `--repl` to a command that writes
  to the same terminal (such as `tee`) garbles the screen after printing a
  warning.

## Alternatives

- **A separate REPL turn loop.** Rejected because [RFD 131]'s turn loop already
  executes multiple turns per run, and `always` is a third variant on its
  `query.loop` axis.
- **A fourth `query.loop` variant for live input.** Rejected because it
  conflates two independent choices: when the run continues after a turn ends
  and whether the input field is visible during turns.
  Supporting `always` without live input during turns, or live input with
  `never`, would require still more enum variants.
- **Enabling the live input field under `if_pending` by default.** Rejected
  because `if_pending` is the default `query.loop` setting, so every interactive
  `jp q` would enter raw mode and `jp q | less` would break by default.
- **Rendering the input field in JP with reedline as a headless state machine.**
  Rejected because it would reimplement reedline's `Painter` (width-aware line
  wrapping, multi-line caret placement, repainting, and resize handling), which
  [RFD 088] adopted reedline to avoid maintaining.
- **Stepping reedline's inner `Editor` directly.** Rejected because keybindings,
  vi/emacs mode transitions, and paste handling live above `Editor` in
  `Reedline` and `EditMode`.
- **Suspending the status region for every prompt, as [RFD 091] does today.**
  Rejected for `live_input` runs because the input field and status row would
  disappear on every tool permission prompt.
- **Drawing the interactive region on stderr and requiring both stdout and
  stderr to be terminals.** Rejected because it would break `> file` and `2>
  file` (which never write to the screen) and would contradict [RFD 048], which
  places interactive I/O on `/dev/tty`.
- **Showing a configured input field only when stdout is a terminal.** Rejected
  because inferring non-interactivity from a redirected stdout is the heuristic
  [RFD 104] removes, and scripts that want non-interactive execution use
  `--no-interactive`.
- **Refusing to run when stdout or stderr is a pipe.** Rejected because
  downstream readers that never write to the terminal (such as `| gzip >
  log.gz`) work cleanly, and JP cannot distinguish them from readers that write
  to the terminal.
- **Hiding the input field during turns when stdout is a pipe.** Rejected
  because the downstream reader drains the pipe asynchronously after JP's final
  write, so its output can still collide with the idle input field between
  turns.
- **Redrawing only when the pipe buffer is empty.** Rejected because an empty
  pipe only indicates that the downstream process has read the bytes, not that
  it has finished writing to the terminal.
- **Using a `DECSTBM` scroll region** (scrolling content within top/bottom
  margins while keeping region rows pinned).
  This reduces redraws, but lines scrolled inside a partial scroll region do not
  enter native scrollback on most terminals.
  Retained only as a fallback if redraw flicker proves unacceptable during the
  Phase 4 spike.
- **Having reedline own the terminal while `Printer` feeds it** (generalizing
  reedline's `ExternalPrinter` to arbitrary styled chunks).
  This preserves reedline's painter but inverts terminal ownership, routing
  typewriter pacing, status regions, and channel serialization through reedline.
  Retained as a fallback if status-region hosting fails the Phase 0 spike.
- **Migrating all standalone prompts to the polled line editor immediately.**
  Rejected as unnecessary churn: `read_line` and the polled interface share the
  same engine and painter, so migrating stable standalone prompts provides no
  consistency benefit.

## Non-Goals

- **Opening the input field on the first keystroke during a standard `jp q`.**
  Detecting that first keystroke would require placing every `jp q` in raw mode,
  capturing keystrokes meant for the shell and conflicting with pagers.
  The input field appears during turns only when `query.live_input = true`.
- **Pre-filling the parent shell's command line** with unsent text on exit.
  A child process cannot populate its parent shell's input buffer without
  shell-specific integration; saving to `QUERY_MESSAGE.md` provides
  shell-independent recovery.
- **Hosting prompts inside the status region when `live_input = false`.**
- **Unifying `Enter` behavior across all phases.** Making `Enter` always start a
  new turn is tracked separately in T-1f6wn02.
- **Alternate-screen or full-screen TUI mode.** Conversation output remains in
  the terminal's native scrollback.
- **Changing `PromptBackend` caller contracts.** `PromptBackend` outcome types
  and per-call-site empty-`Enter` policies remain unchanged.
- **Supporting REPL mode in non-interactive runs** ([RFD 049]).
- **New line-editing features** beyond the existing edit modes and external
  editor integration.

## Risks and Open Questions

- **Two prototype spikes gate implementation.** Phase 0 gates the input field
  itself: rendering on the controlling terminal, restoring the caret after every
  repaint, growing and shrinking a multi-line buffer, and handling terminal
  resize.
  Phase 4's spike gates live input during turns: keeping interactive rows
  aligned below an unfinished content line and across multi-batch typewriter
  tasks, restoring the caret after every content write, and stacking prompt rows
  above the status row and input field while preserving native scrollback and
  avoiding unacceptable flicker.
  `DECSTBM` scroll margins are the fallback for flicker in Phase 4, and
  reedline-owned terminal rendering is the fallback if status-region hosting
  fails in Phase 0.
  If Phase 4's spike fails, Phase 3 (`query.loop = "always"` with the field
  shown between turns) still ships on its own.
- **Cursor position at the right margin.** Terminals differ in how they handle a
  cursor at the last column (deferred wrap); when the cursor position is
  ambiguous, the worker redraws from a known column after emitting a newline.
  The Phase 4 spike verifies this behavior across target terminals.
- **Dependency on [RFD 092].** Until [RFD 092]'s fallback menu lands, pressing
  `Ctrl+C` in the idle input field triggers a graceful shutdown (saving any
  unsent buffer text).
  The separation of `Interrupted` from `Cancelled` prompt outcomes also lands
  with [RFD 092].
- **Prompt backend parity.** Prompt outcome tests must run against both the
  terminal and region `PromptBackend` implementations.
- **Windows console compatibility.** Raw-mode input, relative cursor movement,
  synchronized output, and stream file-type classification require the same
  Windows console verification performed for [RFD 091].
- **[Issue #829].** Mid-stream replies replay unsigned partial reasoning blocks;
  verify interaction with partial-reasoning recovery before Phase 4.

## Implementation Plan

### Phase 0: Region Spike (Gate)

Validate status-region input-field rendering on real terminals (Unix and
Windows) using `region_probe` and the `jp_pty` screen model: drawing on the
controlling terminal, painting at a worker-supplied row origin, reporting and
restoring caret coordinates after every repaint, growing and shrinking a
multi-line buffer, and resizing the terminal.
This spike is a go/no-go gate for subsequent phases; live-input coexistence
during turns is spiked separately at the start of Phase 4.

### Phase 1: Printer Support

Add interactive row support to the status region in `jp_printer`: configurable
output target (`stderr` vs. controlling terminal), caret placement, row
budgeting, and prompt suspension rules.
Route the opt-in stderr tracing layer through the printer worker, and add the
process-wide terminal lease with `emergency_reset()` wired into `SignalRouter`'s
forced-exit paths.
Test against `Printer::memory` and the `jp_pty` screen model.
Depends on Phase 0.

### Phase 2: Reedline Fork and `jp_inquire` Field

Expose the polled line editor API on `Reedline` in the reedline fork, and build
the input-field abstraction in `jp_inquire` with blocking and polled drivers
plus a test mock.
Existing standalone prompts remain unchanged.
Depends on Phases 0 and 1.

### Phase 3: `query.loop = "always"` (Input Field Between Turns)

Implement `query.loop = "always"` with the input field active between turns: the
blocking driver, idle keybindings and `Ctrl+X`, `Ctrl+D` exit, forwarding idle
`Ctrl+C` to `SignalRouter`, turn-end and error-recovery rules, unsent-buffer
draft persistence, and non-interactive fallback to `if_pending`.
In `jp_printer`, add stream classification so the worker erases the region only
around streams that reach the screen, and emit the one-time warning when stdout
or stderr is a pipe.
This phase is shippable on its own as a config-driven REPL without live input
during turns, unfinished-line coexistence, region-hosted prompts, or the event
pump.
Depends on Phase 2, [RFD 131] Phases 1 and 2 (turn loop and `Run` scope), and
[RFD 104] Phase 2 (controlling-terminal promptability).

### Phase 4: `query.live_input` and `--repl`

Start with a real-terminal spike on Unix and Windows covering interactive rows
below an unfinished content line and across multi-batch typewriter tasks, caret
restoration after every content write, and stacked prompt, status, and input
rows.
This spike gates Phase 4 only; if it fails, Phase 3 remains complete.

Implement unfinished-line coexistence and typewriter repaint yields in
`jp_printer`.
Add the region `PromptBackend` (`inline_select` and `inline_reply` rendered in
the region; `select` and `text` suspending the region) and run outcome tests
against both prompt backends.
Add the run-scoped event pump, the keyboard branches in the streaming `select!`
loop and tool-execution wait, per-phase keybindings, and `Ctrl+C` forwarding and
escalation counting.
Expose `query.live_input` and `--repl`, failing before the first turn when
`--repl` is passed in a non-interactive session.
Depends on Phase 3.

### Phase 5: Documentation and Glossary

Document `--repl`, `query.live_input`, keybindings, phase behavior, and terminal
requirements in the user documentation, and add **REPL** and **Input Field**
entries to the ubiquitous-language glossary.

## References

- [RFD 131]: Turn loop, `Run` scope, per-turn config epochs, CLI argument
  lifetimes, and `QUERY_MESSAGE.md` draft rules that this RFD builds on.
- [RFD 091]: Printer-owned status region that this RFD extends with interactive
  rows.
- [RFD 120]: Ephemeral reasoning window sharing the status region.
- [RFD 088]: Inline editor widget and the reedline fork.
- [RFD 092] and [RFD 045]: Interrupt escalation model fed by both `Ctrl+C`
  delivery paths.
- [RFD 048]: Four-channel output model serialized by the printer worker.
- [RFD 096]: Terminal output sanitization for destructive escape sequences that
  the status region cannot undo.
- [RFD 104]: Controlling-terminal promptability signal that determines whether a
  run can host the input field.
- [RFD 049]: Non-interactive mode, which remains unchanged.
- [Issue #829]: Partial-reasoning replay refusals inherited by mid-stream
  replies.
- `crates/jp_printer/src/region.rs`: Status-region implementation and row
  accounting.
- `crates/jp_printer/src/bin/region_probe.rs`: Real-terminal status-region probe
  binary.
- `crates/jp_inquire/src/inline_reply.rs`: Existing `read_line`-based inline
  reply widget.
- `crates/jp_inquire/src/prompt.rs`: `PromptBackend` trait implemented by the
  region backend.
- `crates/jp_cli/src/cmd/query/turn_loop.rs`: Streaming `select!` loop that the
  polled keyboard branch joins.
- `crates/jp_cli/src/signals.rs`: `SignalRouter` and its exit path.

[Issue #829]: https://github.com/dcdpr/jp/issues/829
[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[RFD 091]: 091-printer-owned-status-region.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
[RFD 096]: 096-terminal-output-sanitization-for-untrusted-content.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
[RFD 120]: 120-ephemeral-reasoning-window.md
[RFD 131]: 131-queued-replies.md
