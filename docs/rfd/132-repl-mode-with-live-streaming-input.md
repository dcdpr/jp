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

`jp q --repl` turns a run into a REPL: when a turn ends, the input field stays
on screen and takes the next query instead of the process exiting.
The field is also there while the turn runs, pinned to the bottom of the
terminal as output scrolls above it into native scrollback, so typing and
pressing Enter redirects the assistant without `Ctrl+C` first.

The run's turn loop is [RFD 131]'s.
This RFD adds two settings that combine freely: `query.loop = "always"`, which
waits in the field for the next query at every turn end, and `query.live_input`,
which keeps the field on screen while turns run.
`--repl` sets both.
The field is reedline's editor and painter, drawn inside the printer's status
region ([RFD 091]) on the controlling terminal, which also hosts the status row
and the prompts a turn opens.
Outside a run with a field, the one change is that live tracing goes through the
printer.

## Motivation

Two frictions in the flagship command:

1. **Re-running `jp q` per turn.** Iterative work pays a process start, config
   load, and compose round-trip for every follow-up.
2. **Redirecting a streaming response is a detour.** Today the user presses
   `Ctrl+C`, waits for the interrupt menu, chooses `r`, then types ([RFD 088]).
   An input line that is always there collapses that to: type, press Enter.

[RFD 131] already makes a run hold more than one turn, and names `query.loop =
"always"` as REPL mode.
What it cannot provide is a way to type while a turn runs: reedline's
`read_line`, the blocking loop behind today's prompts, owns the keyboard until
it returns and cannot be one branch of the streaming event loop.
The field therefore needs a polled entry point into the same reedline machinery,
and the idle field between turns uses that same entry point, so it looks and
behaves identically whether or not a turn is running.

## Design

### User-facing behavior

`jp q --repl` composes and sends the first query as usual and runs the turn.
When the turn ends, the field takes the next query; the run continues until the
user ends it.

Two settings decide this, each answering its own question:

```toml
[query]
loop = "always"   # never | if_pending | always
live_input = true # show the field while turns run
```

- `query.loop` ([RFD 131]) decides whether the run goes on at a turn end.
  This RFD adds `always`: wait in the field for the next query.
- `query.live_input` decides whether the field is on screen while a turn runs,
  so the user can type during it.
  Defaults to `false`.

`--repl` sets `loop = "always"` and `live_input = true` for this invocation.
Like `--edit`, it stays out of `apply_cli_config` and is never recorded on the
conversation, so the next plain `jp q` on the same conversation is not a REPL.
`--cfg query.live_input=true` is recorded like any other `--cfg`.

Text typed into the field is a second thing that can be pending in 131's sense,
beside a queued reply, so `if_pending` with the field on keeps the run going
while the field holds text, and ends it at a turn end with an empty field, as a
one-shot `jp q` ends.
Every combination means something:

| `query.loop`           | `live_input = false` (default)                 | `live_input = true`                                                                                      |
| ---------------------- | ---------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `never`                | Today's `jp q`.                                | Type while the turn runs; the run ends when the turn ends, and text left in the field goes to the draft. |
| `if_pending` (default) | Today's `jp q`, plus 131's queued replies.     | The run goes on while anything is waiting to be sent: a queued reply, or text in the field.              |
| `always`               | A REPL whose field appears only between turns. | A REPL with the field always on screen: what `--repl` sets.                                              |

Without `live_input`, the field and raw mode exist only while the run waits
between turns; during a turn, keys and `Ctrl+C` behave as in today's `jp q`.

Both settings apply only to interactive runs (see
[Requirements](#requirements)): in one that is not, `always` acts as
`if_pending` and the field stays off.
`--repl` in a run that is not interactive fails before the first turn.
Set as a default, `live_input = true` puts the field, and raw mode, on every
interactive `jp q`, which is why it defaults to `false`.

Whether the field is up during turns is decided once, at run start, from
`--repl` or the first turn's config.
A later turn's config can change when the run ends, not whether the field shows
during turns.

Keys differ by phase because the turn transitions underneath them differ: a
streaming reply continues the turn, while input during tool execution has to
resolve the running tools first.
Every mapping reuses a transition the interrupt menus already have.
The streaming and tool-execution columns apply with `live_input` on.

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

Enter therefore does something different in each phase.
Making it always start a new turn needs its own design and is tracked in
T-1f6wn02; until then the field reuses these transitions as they are.

There is no Abort: [RFD 092] removes it from the interrupt menus, because
streamed content is saved as it commits.

**Turn ends.** A turn that completes, is stopped, or fails recoverably returns
to the idle field under `always`, and under `if_pending` when the field holds
text; otherwise the run ends.
A pending queued reply ([RFD 131]) is sent first, through its countdown.
Escalation and run-fatal failures end the run, as they do under 131.

A failure after the request is committed (provider transport exhausted after
retries) keeps the failed turn in the conversation and reports the error on the
chrome channel; the sent text does not come back to the field, because the turn
already holds it.
It comes back only after failures before the request enters the stream: a draft
preamble that does not parse, an editor that does not spawn.
Authentication, config resolution, persistence, and terminal failures end the
run with a non-zero exit.
`cmd::Error` has no recoverable/fatal axis today; the implementation adds one
rather than matching ad hoc.
A failed tool is not a turn error: it returns `Err` to the assistant as today.

**Unsent text.** A turn end never moves the field's text while the run goes on:
under `always` it stays in the field, and under `if_pending` it is what keeps
the run going.
Text still in the field when the run ends gracefully (a turn end under `never`,
the fallback menu's Quit, the second `Ctrl+C`, SIGTERM) is written to the
conversation's `QUERY_MESSAGE.md` under [RFD 131]'s draft rules, after a blank
line when a queued reply already holds the draft.
The next bare `jp q` seeds from it; a `jp q <query>` meets 131's unsent-draft
prompt.
There is no setting to discard it instead: 131's rule is that no path destroys
text the user typed.
An immediate exit (the third `Ctrl+C`, SIGQUIT) does no file I/O and loses it.

**`Ctrl+X`.** During a turn it opens the external editor on the field's text
alone, as the inline reply widget's escape does today: saving puts the edited
text back in the field, and a cancelled editor leaves the field as it was.
In the idle field, where the text becomes a query, it opens the conversation's
query document instead: `QUERY_MESSAGE.md`, the file `jp q --edit` opens, config
section included.
Its query text is whatever query text the document already holds (a draft left
by an earlier run, or a queued reply kept with `[k]`), a blank line, and the
field's text, as [RFD 131]'s `[e]` seeds it, so no draft text is lost.
Saving puts the document's query text back in the field.
The config section is recorded on the conversation when that text is sent, as
`jp q --edit` records it, and the send consumes the draft.
A cancelled editor leaves the field and the document as they were.
Once the field has taken in the draft's text this way, the unsent-text rule
replaces the draft's query text on exit rather than adding the field's text
after it.

Configuration lifetime, argument scopes, background tasks, and the lock are [RFD
131]'s: each turn is a config epoch, and the REPL adds no rule of its own.

### One field, one polled line editor

- **One engine.** reedline's editor state: `LineBuffer`, the emacs/vi keymaps
  and mode machine, undo/redo, kill-ring.
- **One painter.** reedline's `Painter` draws the prompt and buffer.
- **Two drivers.** A blocking driver parks on the keyboard until an outcome (the
  idle field); a polled driver takes keyboard events as one branch of the turn's
  select (the live field).
  Both step the same editor and paint through the same painter, so the field is
  identical in both states.

Existing prompts keep `read_line`.
`read_line` and the polled entry point are two doors into one reedline engine
and painter; rendering them identically is the fork's contract, not something JP
synchronizes.

### Concurrency

Driving reedline's `Editor` type directly is not an option: the keymaps, the
vi/emacs mode machine, event batching, and paste handling live in `Reedline` and
its `EditMode` (`read_line_helper`, `process_input_batch`, `handle_editor_event`
in the fork's `engine.rs`), and stepping around them would reimplement the
editor in JP.

The fork instead exposes a **polled line editor** over `Reedline`:

- begin (raw mode, prompt state);
- accept one terminal event, processed by the existing edit-mode and event
  machinery;
- repaint at an origin the caller supplies, and report where the caret belongs;
- suspend and resume, keeping the buffer;
- finish with an outcome (submit, cancel, host command).

Both drivers call it.
`jp_inquire` owns the field built on it, with a mock for tests.

### The field lives in the status region

[RFD 091] built the region this design needs: a block of rows at the bottom of
the terminal, drawn by the printer's worker and erased before every
printer-managed write, with relative row accounting, the terminal re-measured on
every draw, width truncation, row backgrounds, a claim stack, and suspension for
writers outside the printer.
[RFD 120]'s reasoning window already streams into it.
The worker remains the single serialized terminal writer ([RFD 048]); this RFD
adds what an editable row needs.

**Where it draws.** 091 draws regions on stderr, as chrome.
A run with a field draws its region on the controlling terminal instead: the
field is interactive input, which [RFD 048] puts on `/dev/tty`, and the status
row and prompts draw with it because the region is one block with one row count.
So the region gains a target, stderr by default, switched by a worker command
that erases on the old target and redraws on the new.
With `live_input` the region stays on the controlling terminal for the whole
run; without it, only while the field waits between turns.
The target covers every claim, [RFD 120]'s reasoning window included.
The stderr target keeps 091's enabling predicate; the controlling-terminal
target needs only an interactive run.
The worker erases and redraws around a write only when its stream reaches this
screen (see [Requirements](#requirements)): a write to a file neither moves the
screen nor leaves a line open on it.

**Layout.** With `live_input` the region renders, top to bottom: the rows of an
open prompt (see [Prompts inside the region](#prompts-inside-the-region)), the
top claim's window and status row (091, unchanged), and the field.

```
┌──────────────────────────────────────────┐
│ assistant output, tool chrome            │ ← scrolls into native scrollback
├──────────────────────────────────────────┤ ← erased before, redrawn after each write
│ Run tool fs_modify_file? [y,Y,n,N,…]     │ ← an open prompt
│ ⏱ Waiting… 9.2s                          │ ← the top claim's status row
│ › the user's live input                  │ ← the field
└──────────────────────────────────────────┘
```

Rows are budgeted field first (one row at least), then the prompt, then the
status row; 091's window gives up rows before any of them, being a preview.

**Interactive rows own the cursor.** A status row has no caret; the field and an
open prompt do.
The polled line editor paints at the origin the worker gives it and reports
where its caret belongs, a position it computes today to place the cursor.
After every frame the worker moves the cursor there with relative moves.
Before a content write it hides the cursor, walks to the region's last row and
erases as 091 does, writes, redraws, and returns the cursor to the caret.
Multi-line line editors already place their caret this way; the worker has the
row accounting to do it from outside.

**Staying drawn below an unfinished line.** 091 hides the region while content
has left a line open (`content_open`), because the region's first row would be
drawn over that line, and a long typewriter task hides it until the task ends.
For a timer that is a blink; for a field the user is typing into, it is the
field vanishing on every line.
Interactive rows are drawn starting one row below the open line instead.
Text continuing that line is written with a relative move up and back, leaving
the region untouched, and the region is erased and redrawn only when the line
ends or would wrap.
Typewriter tasks yield repaints between batches, 091's documented refinement, so
typed keys echo while a long line types out.

**No prompt-writer suspension with `live_input`.** 091 suspends the region
whenever a prompt writer is taken.
A run with `live_input` hosts its prompts in the region, so nothing takes a
prompt writer and the status row and field stay drawn.
The region suspends only for the external editor and for the prompt kinds the
region does not host yet.

**What the worker repairs.** Region repaints are atomic worker commands, cursor
operations go only to the controlling terminal, and after untrusted content the
worker re-asserts cursor visibility, margins, and the modes the terminal lease
owns.
It cannot undo destructive sequences such as scrollback erasure or an
alternate-screen switch, which stay possible in unsanitized content exactly as
outside `--repl`; [RFD 096] mitigates that class and is not a prerequisite.

**Redraw churn** is bounded by coalescing (one repaint per batch of queued
writes) and, on terminals with synchronized output (BSU/ESU), wrapped to remove
flicker.

The painter change is bounded: render bottom-anchored at the worker's origin,
report the caret, never scroll the screen itself.
Narrowing a terminal that reflows its contents (T-1d23tmt) affects these rows as
it affects every region row; it is not a blocker.

### Prompts inside the region

Prompts already reach the terminal through one seam: a turn takes an `Arc<dyn
PromptBackend>`, and the tool prompter and interrupt handler call it.
A run with `live_input` passes a second implementation that renders into the
region; no prompt call site changes.

- `inline_select` (permission prompts, interrupt menus) and `inline_reply`
  (replies, argument and result editing) render in the region's prompt rows.
  `inline_reply` uses the field's own polled line editor, so the two look and
  edit identically.
- `select` and `text` suspend the region and use the terminal widgets, as today.
  During a turn only tool questions with fixed options use `select`; moving it
  into the region is later work.
- The region backend returns exactly the outcomes the terminal backend returns
  for the same keys, and [RFD 092]'s split of `Interrupted` from `Cancelled`
  lands in both.
- An answered prompt's rows leave with it; the backend writes the same answered
  line the terminal widget leaves today, so scrollback reads as it does now.
- While a prompt is open it owns the keyboard, and the field stays drawn,
  inactive, buffer intact.

A run without `live_input` keeps the terminal backend, its field being up only
between turns, when no prompt opens.
Using the region backend whenever a region is active would change every prompt
in the default workflow, so it is future work.

### Input ownership: the event pump

Raw mode stops the terminal from turning `Ctrl+C` into SIGINT: while the field
is up it is byte `0x03` on the input stream, and a byte nobody reads is a dead
interrupt.
Without `live_input`, raw mode is held only while the field waits between turns,
and the field's blocking driver reads every key, forwarding `0x03` to the
`SignalRouter` itself.
With it, a run-owned **event pump** reads terminal events for the run's lifetime
and routes them by owner:

| Owner  | When                                | Behavior                                                                                         |
| ------ | ----------------------------------- | ------------------------------------------------------------------------------------------------ |
| Field  | idle, LLM streaming, tool execution | Keys step the polled line editor; `0x03` is forwarded to the `SignalRouter`.                     |
| Prompt | a prompt is open                    | The prompt takes the events and handles `Ctrl+C` per its own contract.                           |
| Pump   | phases with neither                 | `0x03` is forwarded immediately; other keys queue (bounded) and reach the field when it resumes. |

The field is ambient input, not a prompt: it never consumes `Ctrl+C`.
Interrupt bytes never queue, so a large paste cannot delay one.
On overflow the newest input is dropped, with a bell and a chrome notice;
dropping the oldest would corrupt the middle of what was typed.
Queued input is interpreted when it is delivered: a redirect typed during
provider connection setup stops the stream it was typed ahead of, which is what
the user typing it meant.

| Phase                                                      | Field           | Status row             | Keyboard           |
| ---------------------------------------------------------- | --------------- | ---------------------- | ------------------ |
| Idle                                                       | live            | when a claim is active | field              |
| LLM streaming                                              | live            | visible                | field              |
| Tool execution                                             | live            | visible                | field              |
| A region-hosted prompt                                     | drawn, inactive | visible                | prompt             |
| A `select` or `text` prompt                                | suspended       | suspended              | prompt             |
| Retry backoff, provider setup, persistence, thread rebuild | drawn, inactive | visible                | pump               |
| External editor                                            | suspended       | suspended              | the editor process |

Releasing and re-entering raw mode around each ownerless phase was rejected:
mode changes race with keys in flight and thrash on short, frequent windows.

### Interrupts: one ladder, two entry points

[RFD 092] sets what `Ctrl+C` does across JP; this RFD adds a second way for the
press to arrive.
While the field is up, the field or the pump forwards `0x03` into the same
`SignalRouter` that receives SIGINT outside one, so the escalation ladder ([RFD
045], refined by 092) is identical however the press arrived.

**Which presses count toward escalation.** The router counts a press only when
it arrives with nobody else reading the keyboard: a SIGINT outside raw mode, or
a `0x03` forwarded by the field or the pump.
A press read by an open prompt never reaches the counter.
The prompt turns it into an outcome, and 092's context table, applied where
prompts are hosted (the interrupt handler and the tool prompter), decides what
it means: on an interrupt menu it escalates to the second rung, in a reply
opened from a menu it returns to that menu, in any other prompt it swaps to the
scope's menu.
Nothing is configured per prompt.
Two presses inside a nested prompt climb two levels, and the run exits only when
the second level was the interrupt menu.

**Idle.** In the idle field, `Ctrl+C` reaches the router and opens 092's
fallback menu (`[c] Continue`, `[q] Quit gracefully`) whatever the buffer holds;
`Ctrl+U` clears the line.
Until 092 lands, an unhandled first press is a graceful shutdown, which saves
the buffer as described under unsent text.

**Exit.** `Ctrl+D` on an empty idle buffer ends the run cleanly.

**Terminal lease.** One process-wide owner of terminal-mode state (raw mode,
bracketed paste, keyboard-enhancement flags, cursor visibility, margins) with an
idempotent `emergency_reset()`.
It runs on every immediate-exit path: today the router's `std::process::exit`,
and after 092 its SIGINT re-raise, neither of which runs destructors.
The lease also releases and restores the terminal around the external editor.
`Drop` stays the normal cleanup path; the reset is the guarantee.

### Live tracing routes through the printer

The opt-in stderr tracing layer (`-v`, `--log`, `--log-file=-`) writes to stderr
directly, which is why 091 turns regions off while it is active
(`TerminalCapability::with_stderr_logging`).
A REPL cannot turn its field off, so the layer moves onto a printer-routed
writer: each event becomes an `Err` task and is erased around like any chrome.
Two guards keep what the direct path gave:

- `jp_printer`'s own targets stay file-only, so a failing printer never logs
  through itself.
- The file layer stays direct and complete, the record that survives a wedged
  worker or a hard exit.

With every terminal writer worker-arbitrated, 091's stderr-logging condition has
nothing left to protect, so regions also render under `-v` outside REPL runs.

### Requirements

A run hosts the field only when it is interactive, by the signal every prompt
uses: [RFD 104]'s promptability, which holds when a controlling terminal is
available and neither `--no-interactive` nor `JP_NONINTERACTIVE` is set.
Redirecting or piping stdout does not change it.
Nothing else decides it, whether the field was asked for with `--repl` or in
config.

`--repl` in a run that is not interactive fails before the first turn, the way
`jp q --edit` and `--quote` already do, with an error naming the reason: the
flag, the variable, or no terminal.
The check runs after `JP_NONINTERACTIVE` is folded into the flag, so one rule
covers all three; a clap conflict with `--no-interactive` would catch only the
flag spelled out.
`always` and `live_input` from config never fail a run: in one that is not
interactive, `always` acts as `if_pending` and the field stays off.

Today interactivity follows whether stdout is a terminal, so a run with stdout
redirected is treated as having nobody to ask; [RFD 104], which this RFD
requires, moves it to the controlling terminal.

The output format does not matter.
The region draws on the controlling terminal, whose bytes are never stripped
([RFD 048]), and `--format` decides how content and chrome are encoded, not
whether a field exists: [RFD 092]'s split between being interactive and printing
pretty output.

The worker keeps its row count only if it sees every byte that reaches the
screen.
At run start JP classifies stdout and stderr, checking for a terminal first and
then the stream's file type (`is_fifo` and `is_socket` on Unix, `GetFileType` on
Windows):

- **A terminal.** The worker writes it, and erases and redraws around each
  write.
- **A file, or a device such as `/dev/null`.** Nothing written there reaches the
  screen, so the worker writes it without erasing.
- **A pipe or a socket.** Written like a file, since JP's own write never
  reaches the screen.
  The program reading it, though, may write to this terminal at any moment, and
  nothing about the pipe says whether it will.

The terminal check comes first because an MSYS terminal on Windows (mintty, Git
Bash) is itself a named pipe; Rust's `is_terminal` already recognizes it.

Most programs put after a REPL do write to the terminal, since otherwise the
answers never reach the screen: `tee`, `cat`, `jq`, `bat`, a pager.
`tee` prints each chunk wherever the cursor is, which is the caret inside the
field, and the next redraw erases from the wrong row; a pager also competes for
keystrokes and switches to the alternate screen.
Programs that do not write to the terminal (`| gzip > log.gz`, `| pbcopy`) work,
and leave the answers off the screen exactly as `> file` does.

JP cannot tell the two kinds apart, so a pipe is the user's call.
A run that shows the field (`always`, or `live_input`) with a pipe on stdout or
stderr proceeds, after one notice on the terminal per piped stream, before the
first turn:

```
⚠ stdout is a pipe. A program reading it that writes to this terminal (tee,
  cat, a pager) garbles the screen around the input field; the conversation
  is unaffected. `> file`, or `jp conversation print` afterwards, avoids it.
```

A pipe never turns the field off by itself.
Reading "nobody is there" from a redirected or piped stdout is the heuristic
[RFD 104] removes from prompts, and a script that wants no field says so with
`--no-interactive`, as it does to avoid prompts.

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

If the terminal is too small to hold content and region, the field suspends
while a turn runs and returns between turns.

### Naming

"REPL" is not yet in JP's vocabulary; a run ([RFD 131]) holds turns, and a REPL
is a run that waits for its next query at every turn end (`query.loop =
"always"`).
The **input field** is where that query is typed, and with `live_input` it is
also on screen while turns run.
The glossary gains both entries.

## Drawbacks

- **Raw mode while the field is up**, which with `live_input` is the whole run.
  Restoring the terminal on hard exits depends on the terminal lease, a
  process-wide invariant every exit path has to honor.
- **An always-on event pump** with `live_input`.
  Keyboard routing gains a run-lifetime task whose owner table has to stay right
  across every phase transition.
- **One more config key.** `query.live_input` sits beside `query.loop`, and the
  docs have six combinations to explain; each of them is a mode someone wants,
  which is why the field is a key of its own.
- **Redraw churn.** Every content write erases and repaints the region; slow
  terminals may flicker where they do not today.
- **The region becomes interactive.** Field, prompts, and status row now share
  one primitive, so a bug in it hits all three at once (Tesler: the complexity
  moves into one place rather than disappearing).
- **Two prompt backends to keep equivalent.** Every outcome the terminal backend
  returns, the region backend must return for the same keys.
- **Live stderr traces share the worker queue** and can lag behind content; the
  file layer stays immediate.
- **The reedline fork grows** by the polled entry point and the bottom-anchored
  painter, on top of [RFD 088]'s I/O patches (Lehman's Law).
- **Pipes are not protected.** JP can see that stdout is a pipe but not what
  reads it, so a piped `--repl` warns and runs, and a reader that writes to the
  terminal garbles the screen.

## Alternatives

- **A turn loop of the REPL's own.** Rejected: [RFD 131]'s loop already runs
  several turns per run, and `always` is a value on its axis.
- **A fourth `query.loop` value for "go on while text is typed".** Rejected: it
  bundles two independent choices, when the run goes on and whether the field
  shows during turns.
  `always` without the field during turns, and the field during turns with
  `never`, would each need yet another value.
- **Showing the field under `if_pending`.** Rejected: `if_pending` is the
  default, so every interactive `jp q` would hold raw mode, and `jp q | less`
  would garble by default.
- **JP renders the field itself, reedline as a headless engine.** Rejected: it
  reimplements the painter (width-aware wrapping, multi-line caret placement,
  repaint, resize) that [RFD 088] chose reedline to avoid writing.
- **A fork-exposed `Editor` step.** Rejected: the keymaps and mode machine live
  above `Editor`, so JP would reimplement the translation layer.
- **Suspending the region for every prompt, as 091 does today.** Rejected for
  REPL runs: the field and status row would vanish on every permission prompt.
- **Drawing the region on stderr and requiring both streams to be the
  terminal.** Rejected: it refuses `> file` and `2> file`, which never touch the
  screen, and contradicts [RFD 048], which puts interactive input on `/dev/tty`.
- **Hosting a configured field only when stdout is the terminal.** Rejected: it
  reads "nobody is there" from a redirected stdout, the heuristic [RFD 104]
  removes, and gives the field a different idea of interactive than every prompt
  has.
  `--no-interactive` is the signal a script gives.
- **Refusing pipes.** Rejected: it also refuses readers that never touch the
  terminal, JP cannot tell those apart from the ones that do, and it would make
  `--repl` the one place JP rejects a pipe.
- **Hiding the field during turns when stdout is a pipe.** Rejected: the reader
  drains the pipe after JP's last write, so its final output can still land in
  the idle field.
- **Redrawing only once the pipe is empty.** Rejected: an empty pipe says the
  reader has read, not that it has written.
- **A DECSTBM scroll region** (content scrolled inside margins, region rows
  pinned).
  Fewer redraws, but lines scrolled within a partial region do not enter
  scrollback on most terminals.
  Kept only as a spike fallback if redraw flicker is unacceptable.
- **reedline owns the terminal and the printer feeds it** (the `ExternalPrinter`
  model generalized to partial styled chunks).
  Keeps the painter but inverts terminal ownership and drags typewriter pacing,
  status regions, and channel routing through reedline.
  The fallback if the region fails outright.
- **Migrating every prompt to the polled line editor now.** Rejected as
  unnecessary: both entry points share one engine and painter, so it buys no
  consistency and risks long-stable prompt flows.

## Non-Goals

- **Opening the field on the first keystroke.** Doing it for every `jp q` would
  put every query in raw mode, take keys the shell would otherwise receive, and
  conflict with pagers reading the same terminal.
  A run shows the field during turns only when `query.live_input` asks for it.
- **Prefilling the shell's command line** with unsent text on exit.
  A child process cannot write to its parent shell's input line without
  per-shell integration; the draft file is the shell-independent stand-in.
- **Region-hosted prompts in runs without `live_input`.**
- **One meaning for Enter in every phase.** Making Enter always start a new turn
  is tracked in T-1f6wn02.
- **An alternate screen or a TUI.** The conversation stays in native scrollback.
- **Caller-contract changes.** `PromptBackend`'s outcomes and every call site's
  empty-Enter policy are unchanged.
- **Non-interactive runs** ([RFD 049]); a REPL needs a terminal.
- **New editing capability.** One external editor, the existing edit modes.

## Risks and Open Questions

- **Two spikes gate the work.** Phase 0 gates the field itself: drawn on the
  controlling terminal, the caret returned after every repaint, a multi-line
  buffer growing and shrinking, and resize.
  Phase 4's spike gates typing during turns: interactive rows kept coherent
  below an open content line and through long typewriter tasks, the caret
  returned after every content write, and prompt rows above the status row, with
  native scrollback intact and flicker acceptable with and without synchronized
  output.
  DECSTBM is the fallback for flicker in Phase 4; reedline owning the terminal
  is the fallback if the region fails outright.
  A Phase 4 failure leaves Phase 3's `always` standing.
- **Where a write leaves the cursor.** Terminals differ at the last column
  (deferred wrap); where the worker cannot be sure, it redraws from a known
  position after a newline.
  Phase 4's spike checks this per terminal.
- **[RFD 092] is not implemented.** Idle `Ctrl+C` falls back to a graceful
  shutdown until its fallback menu exists, and the `Interrupted` outcome split
  lands with it.
- **Backend parity.** Outcome tests have to run against both prompt backends.
- **Windows.** Raw-mode input, relative cursor movement, synchronized output,
  and stream classification need the same console verification 091 got.
- **[Issue #829].** The mid-stream reply replays unsigned partial reasoning;
  re-check against the recovery work before Phase 4.

## Implementation Plan

### Phase 0: region spike (gate)

On a real terminal through `region_probe` and the `jp_pty` screen model, on Unix
and then Windows: the region drawn on the controlling terminal; the painter
rendering at a worker-supplied origin and reporting its caret; the caret
returned after every repaint; a multi-line buffer growing and shrinking; resize.
Go/no-go for the rest.
What only typing during turns needs is spiked at the start of Phase 4.

### Phase 1: printer

Interactive rows in the region: the region target, caret placement, the row
budget, and the suspension rules.
The printer-routed stderr tracing writer.
The terminal lease, with `emergency_reset()` on the router's exit path.
Tested against `Printer::memory` and the `jp_pty` model.
Depends on Phase 0.

### Phase 2: fork and widgets

The polled line editor over `Reedline`.
The field in `jp_inquire`, with both drivers and a mock.
No existing prompt changes.
Depends on Phases 0–1.

### Phase 3: `always`, with the field between turns

`query.loop = "always"` with the field up only between turns: the blocking
driver, the idle keys and `Ctrl+X`, the `Ctrl+D` exit, idle `Ctrl+C` forwarded
to the router, the turn-end, error, and unsent-text rules, and the fallback in
runs that are not interactive.
In the printer, erasing only around streams that reach the screen; stream
classification and the pipe notice.
Shippable on its own: a REPL set in config, without typing during turns, and
without the open-line work, region-hosted prompts, or event pump that typing
during turns needs.
Depends on Phase 2, on [RFD 131]'s turn loop and Run scope (its phases 1–2),
and on [RFD 104]'s promptability (its phase 2).

### Phase 4: `live_input` and `--repl`

Opens with a spike on a real terminal, Unix and then Windows: interactive rows
below an open content line and through a long typewriter task, the caret
returned after every content write, and a prompt row above the status row above
the field.
Go/no-go for this phase only; a failure leaves Phase 3 standing.

In the printer: open-line coexistence and typewriter yields.
The region `PromptBackend` (`inline_select` and `inline_reply` in the region;
`select` and `text` through suspension), with outcome tests run against both
backends.
The event pump; the keyboard branch in the streaming select and the
tool-execution wait; the phase matrix and per-phase keys; `Ctrl+C` forwarding
and the counting rule.
`query.live_input` and `--repl`, with its refusal in runs that are not
interactive.
Depends on Phase 3.

### Phase 5: documentation and glossary

`--repl`, `query.live_input`, the keys, phases, and requirements in the usage
docs; **REPL** and **Input Field** glossary entries relating them to runs and
turns.

## References

- [RFD 131]: the turn loop, Run scope, config epochs, argument lifetimes, and
  draft rules this RFD builds on.
- [RFD 091]: the status region this RFD makes interactive.
- [RFD 120]: the reasoning window, another client of the same region.
- [RFD 088]: the inline editor widget and the reedline fork.
- [RFD 092] and [RFD 045]: the interrupt model both `Ctrl+C` entry points feed.
- [RFD 048]: the four-channel output model the worker serializes.
- [RFD 096]: sanitization for the destructive sequences the region cannot
  repair.
- [RFD 104]: promptability from the controlling terminal, the signal that
  decides whether a run hosts the field.
- [RFD 049]: non-interactive mode, untouched.
- [Issue #829]: partial-reasoning replay refusals the reply path inherits.
- `crates/jp_printer/src/region.rs`: the region and its row accounting.
- `crates/jp_printer/src/bin/region_probe.rs`: the real-terminal probe.
- `crates/jp_inquire/src/inline_reply.rs`: the `read_line` widget, untouched.
- `crates/jp_inquire/src/prompt.rs`: `PromptBackend`, the seam the region
  backend implements.
- `crates/jp_cli/src/cmd/query/turn_loop.rs`: the streaming select the keyboard
  branch joins.
- `crates/jp_cli/src/signals.rs`: the `SignalRouter` and its exit path.

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
