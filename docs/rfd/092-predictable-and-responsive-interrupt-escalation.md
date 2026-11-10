# RFD 092: Predictable and Responsive Interrupt Escalation

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Extends**: [RFD 045], [RFD 088]
- **Summary**: Hardens Ctrl-C handling with immediate startup routing, interrupt
  menus at every phase, and watchdog-bounded three-rung shutdown escalation.

## Summary

This RFD hardens the interrupt model from [RFD 045] so Ctrl-C is both
**Responsive** (acting immediately at any point in the process lifecycle) and
**Predictable** (on a TTY, the first press always opens a menu, the second
starts graceful shutdown, and the third exits immediately).
It adds startup-time signal routing, fallback interrupt menus for inter-phase
gaps and non-query commands, prompt-level Ctrl-C routing, a shutdown watchdog,
and an explicit non-TTY contract.

## Motivation

[RFD 045] built the routing machinery (a single signal consumer, a LIFO handler
stack, escalation, and a root shutdown token), but left the user experience at
each rung underspecified:

- Levels 2 and 3 are silent.
  A graceful shutdown gives no feedback, and a hung teardown has no time bound,
  leaving another manual Ctrl-C as the only escape.
- A first press opens a menu during streaming and tool execution, but not in the
  gaps between turn phases or during non-query commands.
  A user expecting the first press to open a cancellable menu can press Ctrl-C
  in an uncovered gap and end the run instead; a second press intended for the
  menu then triggers a hard exit.
- Pressing Ctrl-C during an active prompt (tool permission or tool question)
  either cancels the prompt or waits until the prompt completes.
  Neither path opens the interrupt menu.

Closing these gaps requires two properties:

- **Responsive**: Ctrl-C at any point in the program's lifecycle acts
  immediately, without noticeable delay.
- **Predictable** (TTY): 1x Ctrl-C always opens a menu, 2x always begins a
  graceful shutdown, and 3x always terminates immediately.

## Design

### Where the properties hold

Startup initializes the interrupt machinery before any workspace or command
work.
The process builds the async runtime first (which depends only on CLI
arguments), then creates the `SignalRouter` and registers the generic fallback
handler.
Workspace loading, config resolution, and command execution then run as a single
future, raced in the top-level `select` against the fallback handler's
notifications and the shutdown token.
The router starts with the default escalation cooldown and applies
`interrupt.escalation_cooldown_secs` once config resolves.
(Creating the router in `Ctx::new`, after workspace sanitization, session
resolution, and config resolution, leaves startup under the OS default
disposition, where SIGINT causes an immediate, teardown-free exit.)

Because workspace and config loading are synchronous, a first press during
startup is queued and opens the fallback menu at the first poll (at latest when
the command future starts, typically within tens of milliseconds).
Graceful-shutdown signals (SIGTERM or a second Ctrl-C) take effect as soon as
the router exists and are bounded by the shutdown watchdog.
Across the process lifecycle:

- **Before router creation** (microseconds): OS default disposition.
- **Router creation to first poll**: queued fallback menu on the first press, or
  watchdog-bounded graceful shutdown on SIGTERM or a second press.
- **Command execution**: governed by the scope table under [Handler
  policy](#handler-policy).

Two qualifications apply to rung 1:

- **Default configuration.** Opening a menu on the first press is the default
  setting.
  A fixed `interrupt.<scope>.action` (e.g. `interrupt.streaming.action =
  "stop"`) replaces rung 1 for that scope without affecting rungs 2 and 3.
- **Poll boundaries.** Handlers run on their event loop, so a press during
  synchronous work (such as a mid-turn persist or thread building) opens the
  menu at the next poll point, typically within milliseconds.
  A second press before that poll point escalates to rung 2 as usual.

### Interactive vs. machine output

Following [RFD 048]'s channel model, two independent predicates govern interrupt
behavior, and neither checks whether stdout is a TTY:

- **Interactive**: a controlling terminal (`/dev/tty`, `CONOUT$`) is available
  for prompt input and output.
  Menus and prompts gate exclusively on this predicate.
- **Pretty output**: a channel's format resolves to `text-pretty`.
  This governs how chrome (messages and countdown lines) renders, not whether a
  menu opens.

Under this split, `jp query --format=json` and `jp query | jq` still open menus
on `/dev/tty` when a controlling terminal is present, while stdout stays
machine-readable.
Without a controlling terminal, rung 1 collapses (see [Non-TTY
contract](#non-tty-contract)) and shutdown messages write to stderr in stderr's
configured format.

Existing prompt gates that check `stdout().is_terminal()` (`ctx.term.is_tty`),
including the lock-contention prompt and parts of the turn loop, migrate to the
Interactive predicate.

### Escalation ladder and shutdown watchdog

| Press | Behavior                                                        |
| ----- | --------------------------------------------------------------- |
| 1st   | Open the interrupt menu for the current scope (streaming, tool, |
|       | turn, or the generic fallback).                                 |
| 2nd   | Print `gracefully shutting down…`, cancel the shutdown token,   |
|       | save state, run full teardown, exit 130.                        |
| 3rd   | Print `terminating program…`, exit immediately (130).           |

A second SIGINT within the escalation cooldown
(`interrupt.escalation_cooldown_secs`) or Ctrl-C inside any interrupt menu
reaches the second rung.
`kill -QUIT` remains an immediate exit.

**Menu cancel contract.** In every interrupt menu, ESC means "continue": it
closes the menu and resumes the interrupted work, matching `[c]`.
Ctrl-C is the only key that climbs the ladder.
Continuing via ESC or `[c]` also resets the escalation counter, so a later
Ctrl-C acts as a fresh first press rather than escalating to rung 2.
This supersedes [RFD 045]'s rule that cancelling an interrupt menu by any means
escalates; only Ctrl-C on a menu escalates.

**Exit convention.** On Unix, interrupt-initiated exits terminate by re-raising
SIGINT with the default disposition (after cleanup at rung 2, or immediately at
rung 3 and on watchdog expiry), so parent shells observe a death-by-signal
(`WIFSIGNALED`) and abort loops and pipelines.
Shells report this as exit status 130, which is the shorthand used in the ladder
table.
On Windows, JP exits with code 130 directly.

**Shutdown watchdog.** When the shutdown token cancels, a watchdog task starts
and renders a countdown line after a configurable delay.
If the countdown reaches zero while the process is still alive, the watchdog
prints `terminating program…` and exits, bounding graceful shutdown.
The watchdog's countdown replaces the background-task drain's 2-second
"Cancelling background tasks…" line so a single countdown covers the entire
teardown.

```toml
[interrupt.shutdown]
show = true # render the countdown line
delay_secs = 1 # silence before the countdown appears
interval_ms = 100 # countdown update rate
timeout_secs = 10 # hard-kill deadline for the whole graceful shutdown
```

The configuration shape matches `style.lock_wait` and the other timer configs.
SIGTERM enters the same path: message, countdown, and watchdog.

| Key            | Default | Zero means                            | Affects      |
| -------------- | ------- | ------------------------------------- | ------------ |
| `show`         | `true`  | N/A                                   | Display only |
| `delay_secs`   | `1`     | Countdown appears immediately         | Display      |
| `interval_ms`  | `100`   | Clamped to a minimum update rate      | Display      |
| `timeout_secs` | `10`    | Zero graceful deadline: hard-kill now | Enforcement  |

Setting `show = false` suppresses the countdown line without disabling the
watchdog timeout.

**Ownership.** Every cancellation path (the router's second press, SIGTERM, an
unhandled first press, and menu escalations inside the turn loop) cancels the
shutdown token, so a single watcher covers them all.
`run_inner` spawns the watchdog once, as soon as the router exists.
The watchdog owns the countdown rendering (via the `Printer`) and the hard-exit
deadline, keeping the router free of terminal UI per [RFD 045].

**Output discipline.** Every shutdown message and countdown goes through the
`Printer` on the chrome channel (stderr), formatted according to the active
output mode:

- `text-pretty`: the countdown renders as a single `\r`-rewritten line, matching
  the existing timers, and the two shutdown messages print as plain lines.
- `text`: the two shutdown messages print as plain lines, with no in-place
  rewrites or countdown updates.
- `json`: the two shutdown messages emit stderr NDJSON records
  (`{"message":"gracefully shutting down…"}`), and countdown updates are
  suppressed while the watchdog timeout remains enforced.

The `Printer`'s stderr methods currently bypass NDJSON wrapping (only stdout
prints are wrapped), falling short of [RFD 048]'s requirement that stderr chrome
emit NDJSON under `--format json`.
Phase 1 extends NDJSON wrapping to stderr chrome.

### Rung 1 menus (Interactive)

**Turn menu.** Instead of ending the turn silently, the turn-level handler opens
a turn menu in the turn loop's context (matching the streaming menu) when a
first press lands in the gaps between turn phases (persistence, thread building,
and response processing):

```
[c] Continue turn
[r] Reply (inject a message, continue)
[s] Stop (save & exit)
```

Choosing `[s]` completes the turn cleanly (exit 0); cancelling the turn menu
with Ctrl-C escalates to rung 2 (exit 130), matching every other interrupt menu.
Reply reuses the streaming menu's machinery (commit partials, inject the
`ChatRequest`, and prepare a continuation), while the existing `sanitize` pass
covers pending tool calls.
The menu omits an Abort option because the previous cycle is already persisted
at a phase gap, leaving nothing unsaved to discard.

The turn scope uses the same configuration shape as the other menu scopes:

```toml
[interrupt.turn]
action = "prompt" # prompt | continue | reply | stop
compose_in_editor = false # same semantics as interrupt.streaming
```

Setting `compose_in_editor = true` sends the turn menu's Reply action directly
to `$EDITOR`, matching `interrupt.streaming`.

**Generic fallback menu.** `run_inner` registers a bottom-of-stack handler
before workspace and config loading, and the top-level `select` polls it
alongside the startup-and-command future (see [Where the properties
hold](#where-the-properties-hold)).
A first press outside any more specific handler opens a minimal menu:

```
[c] Continue
[q] Quit gracefully
```

Displaying the menu blocks the `select` task, pausing the command's main future
until the user chooses `[c]` (the same pattern as the streaming menu).
Spawned background work continues in the meantime; if the command finishes while
the menu is open, choosing `[c]` completes immediately.
This single bottom-of-stack handler provides a first-press menu across every
command without per-command migration.
Because the fallback menu can open outside any turn or conversation, it has no
Reply option, no straight-to-editor path, and no configuration surface; allowing
an `action` override here would reintroduce silent first-press exits.

**Menu erase-on-close.** Menus erase themselves after a choice.
`inquire` prompts clear their body and write a one-line answered summary that
the caller erases (cursor-up plus clear-line on its writer), and the
reedline-based `InlineReply` does the same.
If rendering the menu scrolled the terminal, earlier output remains shifted into
scrollback after the menu clears.
Because `InlineReply` is erased on submit, the normal rendering path must once
again echo the submitted reply (which was omitted while the surviving widget
remained on screen).

### Handler policy

A handler may consume a first press only by presenting a user-facing choice or
delegating to a child process; otherwise it declines (`SignalRouter::decline`)
so the next handler's menu opens.

| Scope           | Rung 1 behavior                           |
| --------------- | ----------------------------------------- |
| Streaming loop  | Streaming menu                            |
| Tool execution  | Tool menu                                 |
| Turn gaps       | Turn menu                                 |
| Lock wait       | Lock-contention prompt (the scope's menu) |
| Anything else   | Generic fallback menu                     |
| Plugin dispatch | Delegated to the plugin (see below)       |

**Plugin scope.** A plugin run delegates rung 1 to the child process, which may
render its own terminal UI:

1. **1st press**: the host forwards SIGINT to the plugin's process group.
   The child is spawned in its own process group so the terminal cannot
   broadcast Ctrl-C directly to it; the host controls signal delivery.
   A plugin that handles SIGINT shows its own interrupt UI, and one that does
   not exits as an ordinary CLI.
2. **2nd press**: JP takes over.
   The shutdown token cancels, the dispatch thread sends
   `HostToPlugin::Shutdown`, and the existing grace-then-SIGKILL window runs.
3. **3rd press**: without sending a protocol message, the router's exit path
   walks a process-global kill-on-exit registry to send SIGKILL to the child,
   and JP exits.

The plugin's piped standard streams remain protocol and tracing channels
throughout.
A plugin with its own interrupt UI renders it on the controlling terminal
(`/dev/tty`), matching JP's prompt model from [RFD 048].
This refines [RFD 072]'s statement that the plugin never writes directly to the
user's terminal for interactive prompts; [RFD 072] can incorporate that
alignment when it advances, without blocking this RFD.

Signal forwarding is Unix-only; Windows keeps the current behavior (see
[Non-Goals](#non-goals)).

### Ctrl-C inside active prompts

`inquire` distinguishes the two keys on its crossterm backend: ESC yields
`OperationCanceled`, and Ctrl-C yields `OperationInterrupted`.
JP's call sites currently treat both the same.
Under this RFD, the effect of Ctrl-C depends on what is on screen:

| Context                                 | Ctrl-C                            |
| --------------------------------------- | --------------------------------- |
| An interrupt menu                       | Escalate (rung 2)                 |
| A reply opened from an interrupt menu   | Back to that menu ([RFD 088], |
|                                         | unchanged)                        |
| Any other prompt (permission, question, | Swap to the scope's interrupt     |
| result edit)                            | menu, `[b] back` appended         |

- **ESC** cancels any standalone prompt as before, and means "continue" inside
  an interrupt menu (see the menu cancel contract).
- **Ctrl-C** in a standalone prompt swaps to the current scope's interrupt menu
  with a `[b] back to prompt` entry appended; choosing `back` re-displays the
  prompt from the queue, and pressing Ctrl-C again escalates to rung 2.

**Scope resolution.** The current scope is the topmost registered handler when
Ctrl-C arrives; prompts do not register scopes of their own:

| Prompt                              | Typical phase  | Menu shown        |
| ----------------------------------- | -------------- | ----------------- |
| Tool permission (streaming path)    | Streaming      | Streaming menu    |
| Tool permission (restart prep)      | Turn gap       | Turn menu         |
| Tool question / result edit         | Tool execution | Tool menu         |
| Reply opened from an interrupt menu | Any            | Back to that menu |

There is no dedicated permission menu: the tool menu's options assume a running
tool ("continue, waiting for tool" and "restart"), which does not apply during
pre-approval.
The ambient scope's menu plus `[b] back` covers permission prompts.

**Prompt ownership.** Swapping to an interrupt menu and returning requires every
prompt to be re-displayable, so every prompt must exist as queued data.
Only tool questions and result edits currently live in the pending-prompt queue
(`PendingPrompt`), while permission prompts are resolved synchronously by
calling `resolve_tool_call_decision` inline and awaiting the answer.
This RFD moves permission prompts into the same queued model, giving permission,
question, and result-edit prompts a single representation.
The streaming path preserves its ordering constraint by awaiting resolution
before processing further LLM events; the queue entry exists so `[b] back` can
re-display the prompt.

**Widget outcomes.** The reedline-based `InlineReply` separates `Signal::CtrlC`
from its other cancel paths: `ReplyOutcome::Interrupted` (Ctrl-C) is distinct
from `ReplyOutcome::Cancelled` (ESC and other local cancels), and the caller
decides how to handle each outcome.
Queued presses are handled at both boundaries: the coordinator drains any
pending interrupt notification before displaying a prompt, and the in-tree
widgets poll a cancellation source between terminal events so an active prompt
reacts immediately when a signal arrives.

**Relationship to [RFD 088].** [RFD 088] states that Ctrl-C inside `InlineReply`
is always a local reply cancellation rather than [RFD 045] escalation.
This RFD narrows that rule to replies opened from an interrupt menu and applies
the menu-swap behavior to all other prompts.
`InlineReply` retains [RFD 088]'s core design (outcomes are data and the caller
decides what they mean); [RFD 088]'s note will be updated in place when this RFD
is implemented.

### Non-TTY contract

Without a controlling terminal, signals arrive only via `kill(2)` and menus
cannot open (`inquire` fails with `NotTTY`).
Rung 1 collapses, and each signal advances one rung:

- **SIGINT**: begin a watchdog-bounded graceful shutdown and print the shutdown
  message on stderr; a second SIGINT exits immediately.
- **SIGTERM**: begin a watchdog-bounded graceful shutdown, completing well
  within a supervisor's TERM → wait → KILL window (90s by default in systemd).
- **SIGQUIT**: exit immediately.

Exits follow the exit convention above: death by re-raised SIGINT on Unix
(observed as status 130), and `exit(130)` on Windows.

## Drawbacks

- The generic fallback menu pauses the command's main future while displayed,
  while spawned background work continues running.
  A menu can remain open over a command that has already finished.
- The prompt-swap flow (`Ctrl-C` → interrupt menu → `back`) adds a state
  transition to the prompt queue and a re-display path that must preserve prompt
  content exactly.
- Unifying permission prompts into the pending-prompt queue requires refactoring
  the tool coordinator's prompt flow.
- The `[interrupt.shutdown]` section and the new stderr chrome messages expand
  the configuration and `--format` output surface.

## Alternatives

- **Swallow the first press during prompts** (do nothing until the prompt
  closes): rejected because it violates both properties.
  An ignored press trains users to press Ctrl-C again, skipping a rung.
- **Alternate screen buffer for menus** (exact screen restoration): rejected
  because it hides the streamed context the user needs while choosing.
- **Per-command interrupt scopes instead of the generic fallback menu**:
  rejected as the default path because it requires migrating every long-running
  command, whereas the bottom-of-stack handler covers all commands at once.
  Individual commands can still register richer scopes where useful.
- **Repurpose SIGQUIT as "graceful with core-dump semantics"**: rejected because
  [RFD 045] fixed SIGQUIT as the unconditional exit and supervisors do not send
  it expecting cleanup.
- **A `HostToPlugin::Interrupt` protocol message instead of forwarding SIGINT**:
  rejected because plugins are ordinary CLIs that already handle SIGINT; a
  protocol message requires a version bump and only reaches plugins that poll
  stdin mid-operation.

## Non-Goals

- Interrupt semantics for `inquire`-crate prompts beyond what its error variants
  already expose; deeper integration (such as widget-level cancellation polling
  inside `inquire` prompts) waits until the fork is inlined into
  `crates/contrib`.
- Signal handling on Windows beyond the existing Ctrl-C/Ctrl-Break mapping.
- Cross-process interrupt routing ([RFD 027]).

## Risks and Open Questions

- **Watchdog vs. slow-but-healthy teardown.** A teardown legitimately slower
  than `timeout_secs` (such as a large workspace persist on a slow disk) is
  terminated when the deadline expires.
  The default must accommodate real workloads; 10 seconds is an initial
  estimate, not a measured bound.
- **Menu over mid-line output.** The fallback menu can open while chrome or
  streamed content is mid-line; the renderer must start the menu on a clean line
  and erase back to the interruption point.
- **Prompt re-display fidelity.** Choosing `back` must re-render prompts whose
  preamble was built from transient state (such as diffs in permission prompts).
  The pending-prompt queue already carries the full prompt data, but each prompt
  kind needs verification.
- **Replay safety of saved interrupts.** Graceful shutdown and the menus' save
  options persist the same partial-event state JP writes today.
  Some providers may reject replay of interrupted reasoning blocks (issue
  [#829], Anthropic `reasoning_extraction` refusals) until partial-reasoning
  recovery lands.
  This RFD adds save triggers that write the existing event shapes.

## Implementation Plan

### Phase 1: Early router, messaged ladder, and shutdown watchdog

Restructure startup to build the async runtime first (using only CLI arguments),
create the `SignalRouter` with the default cooldown (updated once config
resolves), and run workspace loading, config resolution, and command execution
as a single future under the top-level `select`.
Introduce the Interactive predicate (controlling-terminal availability, exposed
via `Ctx`) to gate all interrupt menus added in later phases.
Add the `[interrupt.shutdown]` config, the `gracefully shutting down…` and
`terminating program…` messages, the exit convention, the watchdog task in
`run_inner` (replacing the drain's countdown), and the ESC-continue /
counter-reset menu cancel contract.
Route all shutdown output through the `Printer`, including NDJSON wrapping for
stderr chrome.
This phase is independent of the others.

### Phase 2: Turn menu

Update the turn-level handler to open the turn menu (`continue / reply / stop`)
instead of completing the turn silently, and add the `[interrupt.turn]` config
section.
Independent of the other phases.

### Phase 3: Generic fallback menu

Register the bottom-of-stack handler in `run_inner` with the `continue / quit`
menu, and implement menu erase-on-close across the interrupt menus (including
restoring the `InlineReply` submission echo).
Depends on Phase 1 for shutdown messaging.

### Phase 4: Prompt queue unification

Move permission prompts into the pending-prompt queue shared with tool questions
and result edits, preserving the streaming path's await-before-continue
ordering.
Independent of Phases 1 through 3; prerequisite for Phase 5.

### Phase 5: Ctrl-C inside prompts

Separate ESC from Ctrl-C at all prompt call sites (`OperationCanceled` vs.
`OperationInterrupted`), add `ReplyOutcome::Interrupted` to `InlineReply`,
implement the prompt-to-interrupt-menu swap with `back`, drain pending interrupt
notifications before displaying prompts, and poll a cancellation source inside
the in-tree widgets.
Depends on Phases 1 and 2 for the target interrupt menus and on Phase 4 for the
unified prompt queue.

### Phase 6: Plugin scope ladder

Forward SIGINT to the plugin's process group on the first press, and add the
kill-on-exit registry walked by the router's exit path on the third press.
Independent of the other phases.

### Phase 7: Interactive gating and the non-TTY contract

Migrate the existing `is_tty` prompt gates (lock wait and turn loop) to the
Interactive predicate, document and test the non-interactive ladder, and
implement the SIGINT re-raise exit convention.
Independent of the other phases.

## References

- [RFD 045]: the signal router, handler stack, and escalation model this RFD
  builds on.
- [RFD 088]: the `InlineReply` widget whose Ctrl-C contract this RFD refines.
- [RFD 048]: the four-channel output model that defines the Interactive and
  Pretty predicates.
- [RFD 027]: client-server query architecture; interrupt routing over IPC
  remains out of scope.

[#829]: https://github.com/dcdpr/jp/issues/829
[RFD 027]: 027-client-server-query-architecture.md
[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 072]: 072-command-plugin-system.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
