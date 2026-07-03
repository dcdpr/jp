# RFD 092: Predictable and Responsive Interrupt Escalation

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Extends**: [RFD 045], [RFD 088]

## Summary

This RFD hardens the interrupt model from [RFD 045] around two named properties
— **Responsive** (a Ctrl-C acts immediately, anywhere in the program's
lifecycle) and **Predictable** (in a terminal, the first press always opens a
menu, the second always begins a graceful shutdown, the third always terminates)
— and specifies the missing pieces: a messaged escalation ladder with a
shutdown watchdog, an interrupt menu for every scope (a turn menu for the gaps
between turn phases, a process-wide fallback), Ctrl-C handling inside active
prompts, and an explicit non-TTY contract.

## Motivation

[RFD 045] built the routing machinery: a single signal consumer, a LIFO handler
stack, escalation, and a root shutdown token.
What it deliberately did not specify is the *experience* at each rung:

- Levels 2 and 3 are silent.
  A graceful shutdown gives no feedback, and a hung teardown has no bound — the
  only escape is another human press.
- Some first presses show a menu (streaming, tools) and some don't (gaps between
  turn phases, non-query commands).
  A user who has learned "1x = menu, with a chance to back out" is surprised
  when a press in an uncovered moment ends the run: their second press —
  intended for the menu — becomes a hard exit.
- A Ctrl-C while an interactive prompt is active (tool permission, tool
  question) is either consumed by the prompt as a cancel or deferred until the
  prompt completes; neither offers the interrupt menu.

The two properties make these gaps failures by definition:

- **Responsive**: Ctrl-C during ANY point in the program's lifecycle acts
  immediately, without noticeable delay.
- **Predictable** (TTY): 1x Ctrl-C always opens a menu, 2x always begins a
  graceful shutdown, 3x always terminates immediately.

## Design

### Where the properties hold

Startup restructures so the interrupt machinery exists before any real work: the
async runtime is built first (it depends only on CLI arguments), the
`SignalRouter` and the generic fallback handler come immediately after, and only
then do workspace loading, config resolution, and the command itself run — as a
single future raced by the top-level select against the fallback handler's
notifications and the shutdown token.
The router starts with the default escalation cooldown;
`interrupt.escalation_cooldown_secs` is applied once config resolves.
(Today the router is created in `Ctx::new` — after workspace sanitization,
session resolution, and config resolution — so a SIGINT during startup gets the
OS default: an immediate, teardown-free exit.)

Startup is synchronous code with no await points, so a press during it opens the
fallback menu at the first poll — at latest when the command future starts,
typically tens of milliseconds in.
The press is never lost and never silent: the graceful-shutdown paths (SIGTERM,
a second press) act from the first lines of the run, watchdog-bounded.
In one line per lifecycle region: before the router exists, OS default
(microseconds); from router creation to the first poll, queued fallback menu or
watchdog-bounded graceful shutdown; from command execution on, the scope table
under Handler policy.

Two qualifications:

- **Default configuration.** "1x opens a menu" describes default settings.
  A fixed `interrupt.<scope>.action` (e.g. `interrupt.streaming.action =
  "stop"`) deliberately replaces rung 1 for that scope; rungs 2 and 3 are
  unaffected.
- **Poll boundaries.** Handlers run in their event loop's context, so a press
  during synchronous work (a mid-turn persist, thread building) opens the menu
  at the next poll point — normally within milliseconds.
  A second press inside that window escalates to rung 2 as usual; that is the
  ladder working, not a missed menu.

### Interactive vs. machine output

Following [RFD 048]'s channel model, two independent facts govern interrupt
behavior, and neither is "stdout is a TTY":

- **Interactive**: a controlling terminal (`/dev/tty`, `CONOUT$`) is available
  for prompt input and output.
  Menus and prompts gate on this, and only this.
- **Pretty output**: a channel's format resolved to `text-pretty`.
  This governs how chrome (messages, countdown lines) renders — never whether a
  menu exists.

Consequences: `jp query --format=json` in a terminal still opens menus (on
`/dev/tty`) while stdout stays machine-readable; `jp query | jq` with a
controlling terminal still opens menus; without a controlling terminal, rung 1
collapses (see the non-TTY contract) and shutdown messages go to stderr in
whatever format stderr is configured for.

Implementation note: several existing gates use `stdout().is_terminal()`
(`ctx.term.is_tty`) where they mean prompt availability — the lock-contention
prompt and parts of the turn loop.
Those migrate to the Interactive predicate as part of this RFD.

### The escalation ladder, messaged and bounded

| Press | Behavior                                                        |
| ----- | --------------------------------------------------------------- |
| 1st   | Open the interrupt menu for the current scope (streaming, tool, |
|       | turn, or the generic fallback).                                 |
| 2nd   | Print `gracefully shutting down…`, cancel the shutdown token,   |
|       | save state, run full teardown, exit 130.                        |
| 3rd   | Print `terminating program…`, exit immediately (130).           |

The second rung is reached by a second SIGINT within the escalation cooldown
(`interrupt.escalation_cooldown_secs`) or by cancelling any interrupt menu with
Ctrl-C — both paths exist today.
`kill -QUIT` remains an immediate exit.

**Menu cancel contract.** In every interrupt menu, ESC means "continue": close
the menu and resume the interrupted work, exactly like choosing `[c]`.
Ctrl-C is the only key that climbs the ladder.
A deliberate continue — ESC or `[c]` — also resets the escalation counter, so
a later press is a fresh first press rather than an accidental rung 2.
This supersedes [RFD 045]'s "cancelling an interrupt menu by any means should
escalate" for ESC; Ctrl-C on a menu still escalates.

**Exit convention.** On Unix, interrupt-initiated exits terminate by re-raising
SIGINT with the default disposition — after cleanup at rung 2, immediately at
rung 3 and on watchdog expiry — so parent shells observe a death-by-signal
(`WIFSIGNALED`) and abort loops and pipelines correctly.
Shells report this as exit status 130, which is the shorthand the ladder table
uses.
On Windows, JP exits with code 130 directly.

**Shutdown watchdog.** When the shutdown token cancels, a watchdog task starts.
After a configurable delay it renders a countdown line; if the countdown reaches
zero with the process still alive, it prints `terminating program…` and exits
— a graceful shutdown can no longer hang.
The watchdog's countdown *absorbs* the drain's current 2-second "Cancelling
background tasks…" line: one countdown covers the whole teardown.

```toml
[interrupt.shutdown]
show = true # render the countdown line
delay_secs = 1 # silence before the countdown appears
interval_ms = 100 # countdown update rate
timeout_secs = 10 # hard-kill deadline for the whole graceful shutdown
```

The shape mirrors `style.lock_wait` and the other timer configs.
SIGTERM enters the same path: message, countdown, watchdog.

| Key            | Default | Zero means                            | Affects      |
| -------------- | ------- | ------------------------------------- | ------------ |
| `show`         | `true`  | —                                     | Display only |
| `delay_secs`   | `1`     | Countdown appears immediately         | Display      |
| `interval_ms`  | `100`   | Clamped to a minimum update rate      | Display      |
| `timeout_secs` | `10`    | Zero graceful deadline: hard-kill now | Enforcement  |

`show = false` never disables the watchdog: the shutdown stays bounded, just
silent.

**Ownership.** Every cancellation path — the router's second press, SIGTERM, an
unhandled first press, and the menu escalations inside the turn loop —
converges on the shutdown token, so a single watcher covers them all:
`run_inner` spawns the watchdog once, as soon as the router exists.
The watchdog owns the countdown rendering (via the `Printer`) and the hard-exit
deadline; the router stays free of terminal UI, per [RFD 045].

**Output discipline.** Every new message and countdown goes through the
`Printer` on the chrome channel (stderr), with a concrete shape per format:

- `text-pretty`: the countdown is a single `\r`-rewritten line, like the
  existing timers; the two messages are plain lines.
- `text`: the two messages print as plain lines; no in-place rewrites and no
  countdown updates.
- `json`: the two messages are stderr NDJSON records (`{"message":"gracefully
  shutting down…"}`); countdown updates are suppressed entirely — a
  display-only suppression, the watchdog still enforces.

Implementation note: the `Printer`'s stderr methods currently bypass NDJSON
wrapping (only stdout prints wrap), which already falls short of [RFD 048]'s
"chrome on stderr is NDJSON under `--format json`" rule; phase 1 extends the
wrapping to stderr chrome.

### A menu at every rung 1 (Interactive)

**Turn menu.** The turn-level handler ends the turn silently today.
It gains a menu — the turn menu — hosted in the turn loop's own context like
the streaming menu, shown when a press lands in the gaps between turn phases
(persistence, thread building, response processing):

```
[c] Continue turn
[r] Reply (inject a message, continue)
[s] Stop (save & exit)
```

Choosing `[s]` is a deliberate completion (exit 0); cancelling the turn menu
with Ctrl-C is a second press (escalate, exit 130) — the same rule as every
other menu.
Reply reuses the streaming menu's machinery (commit partials, inject the
`ChatRequest`, prepare a continuation); pending tool calls are covered by the
existing `sanitize` pass.
No Abort option: at a gap the previous cycle is already persisted, so there is
nothing unsaved to discard.

The turn scope gets the same configuration shape as the other menu scopes:

```toml
[interrupt.turn]
action = "prompt" # prompt | continue | reply | stop
compose_in_editor = false # same semantics as interrupt.streaming
```

In particular, `compose_in_editor` gives the turn menu's Reply the same
straight-to-editor opt-in as the streaming menu.

**Generic fallback menu.** `run_inner` registers a bottom-of-stack handler
before workspace and config loading; the top-level select polls it alongside the
startup-and-command future (see "Where the properties hold").
A first press anywhere without a more specific handler opens a minimal menu:

```
[c] Continue
[q] Quit gracefully
```

Because the menu blocks the select task, the command's main-future work pauses
while the menu is up (the same pattern as the streaming menu) and resumes on
`[c]`.
Spawned background work continues meanwhile; if the command finishes the moment
the user picks continue, it simply completes.
This delivers "1x always = menu" program-wide with one handler instead of a
per-command migration.
The fallback menu has no Reply option — it can fire outside any turn or even
any conversation — so there is no straight-to-editor path, and it has no
configuration surface: it is the invariant floor of the ladder, and an `action`
override here would reintroduce "1x silently does something".

**Menu erase-on-close.** Menus wipe themselves after a choice: `inquire` prompts
already clear their body and rewrite a one-line answered prompt, which the
caller then erases (cursor-up + clear-line on the writer it owns); the
reedline-based `InlineReply` does the same.
Caveat: if rendering the menu scrolled the terminal, earlier content shifts into
scrollback — the screen ends up clean but shifted.
Erasing the `InlineReply` widget after submit requires restoring the explicit
echo of the submitted reply through the normal rendering path (the echo was
previously removed precisely because the surviving widget already showed it).

### Handler policy

A handler may consume a first press only by presenting a user-facing choice;
anything else declines (`SignalRouter::decline`) so the next handler's menu
shows.
Scopes that delegate the interrupt experience elsewhere are named here, not
implied.

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
   The child is spawned in its own process group precisely so the terminal
   cannot broadcast Ctrl-C into it — delivery is the host's deliberate choice.
   A plugin that handles SIGINT shows its own interrupt UX; one that doesn't
   dies, as any CLI would.
2. **2nd press**: JP takes over — the shutdown token cancels, the dispatch
   thread sends `HostToPlugin::Shutdown`, and the existing grace-then-SIGKILL
   window runs.
3. **3rd press**: no protocol message — the child is SIGKILLed via a
   process-global kill-on-exit registry that the router's exit path walks, and
   JP exits.

The plugin's piped std streams remain protocol and tracing channels throughout;
a plugin that offers its own interrupt UX renders it on its controlling terminal
(`/dev/tty`), mirroring JP's own prompt model from [RFD 048].
This refines [RFD 072]'s "the plugin never writes directly to the user's
terminal" for the interactive case — an alignment to settle in 072 when it
advances; this RFD takes no dependency on it.

Signal forwarding is Unix-only; Windows keeps the current behavior (see
Non-Goals).

### Ctrl-C inside active prompts

`inquire` distinguishes the keys: ESC yields `OperationCanceled`, Ctrl-C yields
`OperationInterrupted` (crossterm backend).
JP's call sites currently collapse both.
What Ctrl-C means depends on what is on screen:

| Context                                 | Ctrl-C                            |
| --------------------------------------- | --------------------------------- |
| An interrupt menu                       | Escalate (rung 2)                 |
| A reply opened from an interrupt menu   | Back to that menu ([RFD 088], |
|                                         | unchanged)                        |
| Any other prompt (permission, question, | Swap to the scope's interrupt     |
| result edit)                            | menu, `[b] back` appended         |

- **ESC** backs out of any prompt (its existing cancel meaning); in interrupt
  menus it means continue (see the menu cancel contract).
- **Ctrl-C** in a standalone prompt swaps to the current scope's interrupt menu,
  with a `[b] back to prompt` entry appended; `back` re-shows the prompt from
  the queue.
  Cancelling that menu escalates as usual.

**Scope resolution.** "The current scope" is the topmost registered handler at
the moment of the press; no prompt installs a scope of its own:

| Prompt                              | Typical phase  | Menu shown        |
| ----------------------------------- | -------------- | ----------------- |
| Tool permission (streaming path)    | Streaming      | Streaming menu    |
| Tool permission (restart prep)      | Turn gap       | Turn menu         |
| Tool question / result edit         | Tool execution | Tool menu         |
| Reply opened from an interrupt menu | Any            | Back to that menu |

There is no dedicated permission menu: the tool menu's options assume a running
tool ("continue — wait for tool", "restart"), which a pre-approval prompt does
not have; the ambient scope's menu plus `[b] back` covers the need.

**Prompt ownership.** The swap-and-return flow requires every prompt to be
re-showable, which means every prompt must exist as queued *data*.
Today only tool questions and result edits live in the pending-prompt queue
(`PendingPrompt`); permission prompts are resolved synchronously — the
streaming path calls `resolve_tool_call_decision` inline and awaits the answer.
This RFD unifies them: permission prompts join the queued model (one
representation for permission, question, and result-edit prompts).
The streaming path keeps its ordering constraint — it still awaits resolution
before processing further LLM events; the queue entry exists so `[b] back` has
somewhere to return to.

**Widget outcomes.** The reedline-based `InlineReply` splits `Signal::CtrlC`
from its other cancel paths: `ReplyOutcome::Interrupted` (Ctrl-C) becomes
distinct from `ReplyOutcome::Cancelled` (ESC and other local cancels); the
caller decides what each means, as before.
Queued presses are handled at both ends: the coordinator drains a pending
interrupt notification before displaying any prompt, and the in-tree widgets
poll a cancellation source between terminal events so an already-displayed
prompt reacts to a press that arrived as a signal.

**Relationship to [RFD 088].** RFD 088 states that Ctrl-C inside `InlineReply`
is a local reply cancellation, never [RFD 045] escalation.
This RFD narrows that rule to the context where it holds — replies opened from
an interrupt menu — and gives all other prompts the swap behavior.
The widget keeps 088's core principle (outcomes are data; the caller decides
what they mean); 088's note is corrected in place when this RFD is implemented.

### Non-TTY contract

Without a controlling terminal there is no Ctrl-C — only signals delivered via
`kill(2)` — and no menus (`inquire` refuses with `NotTTY`).
Rung 1 collapses; every press still climbs exactly one rung:

- **SIGINT**: begin a graceful shutdown (message on stderr, watchdog-bounded); a
  further SIGINT exits immediately.
- **SIGTERM**: graceful shutdown, watchdog-bounded — comfortably inside a
  supervisor's TERM → wait → KILL contract (systemd default: 90s).
- **SIGQUIT**: immediate exit.

Exits follow the exit convention above: death by re-raised SIGINT on Unix
(observed as status 130), `exit(130)` on Windows.

## Drawbacks

- The generic fallback menu pauses the command's main-future work while
  displayed; spawned work races on.
  A menu over a finished command is possible and harmless, but slightly odd.
- The prompt-swap flow (`Ctrl-C` → interrupt menu → `back`) adds a state
  transition to the prompt queue and a re-display path that must preserve prompt
  content exactly.
- Unifying permission prompts into the pending-prompt queue is a real refactor
  of the tool coordinator's prompt flow, not just new UI.
- More configuration surface (`interrupt.shutdown`) and more chrome output to
  keep correct across `--format` modes.

## Alternatives

- **Swallow the first press during prompts** (do nothing until the prompt
  closes): rejected — it violates both properties; a press that does nothing
  visible trains users to press again, which then skips a rung.
- **Alternate screen buffer for menus** (perfect restoration): rejected — it
  hides the streamed context the user needs while choosing.
- **Per-command interrupt scopes instead of the generic fallback menu**:
  rejected as the default path — it requires migrating every long-running
  command; the bottom-of-stack handler covers all of them at once.
  Commands can still register richer scopes where useful.
- **Repurpose SIGQUIT as "graceful with core-dump semantics"**: rejected; [RFD
  045] fixed SIGQUIT as the unconditional exit and supervisors do not send it
  expecting cleanup.
- **A `HostToPlugin::Interrupt` protocol message instead of forwarding SIGINT**:
  rejected — plugins are ordinary CLIs that already understand SIGINT; a
  protocol message needs a version bump and only reaches plugins that poll stdin
  mid-operation.

## Non-Goals

- Interrupt semantics for `inquire`-crate prompts beyond what its error variants
  already expose; deeper integration (e.g. widget-level cancellation inside
  `inquire` prompts) waits until the fork is inlined into `crates/contrib`.
- Signal handling on Windows beyond the existing Ctrl-C/Ctrl-Break mapping.
- Cross-process interrupt routing ([RFD 027] territory).

## Risks and Open Questions

- **Watchdog vs. slow-but-healthy teardown.** A teardown legitimately slower
  than `timeout_secs` (large workspace persist on a slow disk) gets killed.
  The default must be generous enough for real workloads; 10 seconds is a
  starting point, not a measurement.
- **Menu over mid-line output.** The fallback menu can appear while chrome or
  streamed content is mid-line; the renderer must start the menu on a clean line
  and erase back to the interruption point.
- **Prompt re-display fidelity.** `back` must re-render prompts whose preamble
  was built from transient state (e.g. diffs in permission prompts); the
  pending-prompt queue already carries the full prompt data, but this needs
  verification per prompt kind.
- **Replay safety of saved interrupts.** Graceful shutdown and the menus' save
  options persist the same partial-event state JP preserves today.
  Some providers may reject replay of interrupted reasoning blocks (issue
  [#829], Anthropic `reasoning_extraction` refusals) until the partial-reasoning
  recovery work lands.
  This RFD adds save paths, not new saved shapes.

## Implementation Plan

### Phase 1: Early router, messaged ladder, and shutdown watchdog

Restructure startup: build the runtime first (it depends only on CLI arguments),
create the `SignalRouter` (default cooldown, updated once config resolves), and
run workspace/config/command as one future under the top-level select.
Introduce the Interactive predicate (controlling-terminal availability, exposed
via `Ctx`); every new menu in later phases gates on it.
`[interrupt.shutdown]` config, the `gracefully shutting down…` / `terminating
program…` messages, the exit convention, the watchdog task owned by
`run_inner`, absorbing the drain's countdown, and the ESC-continue /
counter-reset menu cancel contract.
All output through the `Printer` per the output discipline, including NDJSON
wrapping for stderr chrome.
Independent of the other phases.

### Phase 2: Turn menu

The turn-level handler shows the turn menu (`continue / reply / stop`) instead
of silently completing the turn, with the `[interrupt.turn]` config block.
Independent; small.

### Phase 3: Generic fallback menu

Bottom-of-stack handler in `run_inner` with the `continue / quit` menu, plus
menu erase-on-close for the interrupt menus (including the `InlineReply` echo
restoration).
Depends on phase 1 for consistent messaging.

### Phase 4: Prompt queue unification

Move permission prompts into the pending-prompt model shared with tool questions
and result edits, preserving the streaming path's await-before-continue
ordering.
Independent of phases 1–3; prerequisite for phase 5.

### Phase 5: Ctrl-C inside prompts

Split ESC from Ctrl-C at all prompt call sites (`OperationCanceled` vs
`OperationInterrupted`), the distinct `ReplyOutcome::Interrupted` in
`InlineReply`, the prompt → interrupt-menu swap with `back`,
pending-notification drain before prompts, and widget-level cancellation polling
in the in-tree widgets.
The largest phase; depends on phases 1–2 for the menus it swaps to and on phase
4 for the queue it returns through.

### Phase 6: Plugin scope ladder

Forward SIGINT to the plugin's process group on the first press; add the
kill-on-exit registry the router's exit path walks on the third.
Independent.

### Phase 7: Interactive gating and the non-TTY contract

Migrate the existing `is_tty` prompt gates (lock wait, turn loop) to the
Interactive predicate; document and test the non-interactive ladder; implement
the SIGINT re-raise exit convention.
Independent.

## References

- [RFD 045] — the signal router, handler stack, and escalation this RFD builds
  on.
- [RFD 088] — the `InlineReply` widget whose Ctrl-C contract this RFD refines.
- [RFD 048] — the four-channel output model the Interactive / Pretty
  distinction follows.
- [RFD 027] — client-server query architecture; interrupt routing over IPC
  remains out of scope here.

[#829]: https://github.com/dcdpr/jp/issues/829
[RFD 027]: 027-client-server-query-architecture.md
[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 048]: 048-four-channel-output-model.md
[RFD 072]: 072-command-plugin-system.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
