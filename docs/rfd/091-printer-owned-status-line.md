# RFD 091: Printer-Owned Status Line

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-03
- **Extends**: [RFD 048]

## Summary

This RFD makes the ephemeral status line — `⏱ Waiting… 9.2s (receiving
response data)` — a first-class concept owned by `jp_printer`.
At most one status line is visible at a time; the printer's worker thread draws
it, ticks its elapsed time, and clears it automatically before any
printer-managed write on any channel.
Seven bespoke timer and temp-line mechanisms across `jp_cli` become clients of
this one primitive.

## Motivation

JP renders ephemeral chrome — a single self-overwriting line on stderr — in
seven places, each with its own hand-rolled draw, tick, and clear logic:

| Site                                                      | Mechanism                                         |
| --------------------------------------------------------- | ------------------------------------------------- |
| Waiting indicator (`cmd/query/turn_loop.rs`)              | `LineTimer`                                       |
| Reasoning timer (`render/chat.rs`)                        | `LineTimer`                                       |
| Lock-wait countdown (`cmd/lock.rs`)                       | `LineTimer`                                       |
| Background-task drain timers (`lib.rs`, two sites)        | `LineTimer`                                       |
| Tool "preparing" temp line (`render/tool.rs`)             | `spawn_tick_sender` + manual `\r…\x1b[K` rewrites |
| Tool execution progress (`cmd/query/tool/coordinator.rs`) | `spawn_tick_sender`                               |

All of them fight over the same invariant: **an ephemeral line must be erased
before any persistent write, and must not disappear before the persistent write
arrives**.
Today that invariant is enforced by per-site discipline, and the codebase
carries the scars of getting it wrong: `clear_temp_line()` before the interrupt
menu, `cancel_reasoning_timer()` inside `flush_on_transition`, the "don't
pre-clear `line_active`" warning in `ToolRenderer::reset`, and the waiting
indicator's `finish().await` ordering dance in the turn loop.
Each was a bug fixed at one site; none of the fixes protects the next site.

The most user-visible instance was the waiting-indicator gap: the indicator was
torn down by the first provider event of any kind — including an SSE keep-alive
ping that renders nothing — leaving the user staring at a blank terminal for
many seconds while the model produced no visible output.
A user who sees a progress indicator vanish and *then* nothing happen reasonably
concludes the program crashed.
That instance is fixed: `LineTimer` (in `jp_cli::timer`) carries a status
channel, and the turn loop finishes the indicator only on events that render.
But the fix is one client-side patch on the missing abstraction; the bug class
remains open at every other site, and every future chrome feature reopens it.

Doing nothing means each new indicator re-implements draw/tick/clear, and each
new combination of chrome and content is a fresh opportunity for a stale line, a
clobbered row, or a premature disappearance.

## Design

### Concept

A **status line** is a single ephemeral chrome line: a subject, an elapsed time,
and an optional replaceable detail.

```
⏱ Waiting… 9.2s (receiving response data)
```

It is chrome in the [RFD 048] sense — written to stderr, never part of the
persistent transcript — with one added contract: the printer guarantees it is
cleared before any printer-managed write, on any channel, reaches the terminal.
One stderr writer lives outside the printer: the optional tracing layer (`-v`,
`--log`, `--log-file=-`), which [RFD 048] deliberately keeps out of the printer.
The worker cannot clear before writes it never sees, so the enabling predicate
below disables status lines while that layer is active.

### API

Callers acquire a status line from the printer and hold an RAII handle:

```rust
// Claim a status line. Returns a no-op handle when status lines are
// disabled (see "Enabling predicate" below).
let status = printer.status_line(StatusStyle {
    delay: Duration::from_secs(2),
    interval: Duration::from_millis(100),
    format: Box::new(|secs, detail| match detail {
        Some(d) => format!("⏱ Waiting… {secs:.1}s ({d})"),
        None => format!("⏱ Waiting… {secs:.1}s"),
    }),
});

status.set_detail("sending request");
// ... later ...
status.set_detail("receiving response data");

// Release: the printer clears the line. Dropping the handle does the same.
drop(status);
```

General callers holding a full `Printer` — the turn loop, lock acquisition, the
shutdown drain — acquire from `Printer` as above; chrome-only renderers acquire
through `ErrChannel::status_line` (below), keeping the stderr-only boundary.

The handle is `Send` and requires no async runtime: there is no
`finish().await`, because the caller no longer owns the clear-before-write
ordering — the printer does.
A client that renders content simply writes through the printer as it always
has; the worker clears the status line first.

The owning handle is not `Clone`; it releases the line on drop.
Components that only push detail (e.g. the turn loop's status transitions)
receive a cloneable `StatusDetail` updater split off the owner, so shared
ownership never blurs who releases the line.

The format closure (rather than a fixed template) is required by existing
clients: the lock-wait and drain timers render a *countdown*, not an elapsed
time.

Chrome renderers hold an [`ErrChannel`], not a `Printer` — the stderr-only view
exists precisely so tool chrome cannot reach stdout.
Status lines are chrome, so acquisition is part of the chrome-facing surface:

```rust
impl ErrChannel {
    pub fn status_line(&self, style: StatusStyle) -> StatusLineHandle;
}
```

`ToolRenderer` migrates through this method and keeps its `ErrChannel`; it does
not regain full-printer access.

### Release contract

- Releasing (dropping the owner) *enqueues* a release command.
  Commands enqueued from one thread stay ordered: a release followed by a print
  from the same thread is processed in that order.
- Every `Print` with non-empty content clears a drawn status line before
  writing, whether or not a pending release has been processed.
  A stale entry can never sit above content.
  "Non-empty" is byte-level, not glyph-level: newline-only and
  control-sequence-only writes clear too.
  Empty-content tasks are no-ops (no clear, no redraw), and the status line's
  own draw and clear writes are exempt — the worker does not recurse.
- A released entry is never redrawn once its release command is processed.
- Across threads, drop is eventual cleanup only: a released entry may be redrawn
  once more if another thread's print is processed before the release command
  drains.
  The stale window is bounded by the queue, and the second rule keeps the stale
  line below content.

The design deliberately provides no blocking release.
Same-thread ordering covers the release-then-render pattern used by every
current client, and a blocking release would reintroduce the async ordering
surface (`finish().await`) this design removes.

### Enabling predicate

A status line renders iff:

1. the resolved output format permits terminal control (`text-pretty`),
2. the chrome channel (stderr) is an interactive terminal, and
3. no tracing layer writes to stderr (`-v`, `--log`, or `--log-file=-`, absent
   `--quiet`).

| Situation                                     | Status line                                                                                         |
| --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| `--format json` / `json-pretty`               | off (chrome is NDJSON)                                                                              |
| `--format text`                               | off (the non-pretty `Sink` strips `\r` and escape sequences; the line would smear as repeated text) |
| `text-pretty`, stderr is a terminal           | on                                                                                                  |
| stderr is not a terminal                      | off, regardless of format                                                                           |
| `-v` / `--log` / `--log-file=-` on a terminal | off (stderr carries live logs, a persistent stream outside the printer)                             |

Condition 2 changes the tty *source* for this chrome from stdout to stderr:
today chrome gating keys off stdout (`ctx.term.is_tty`), so `jp query 2>file`
with stdout on a terminal writes `\r\x1b[K` bytes into the file — under this
predicate it does not.
`--format auto` continues to resolve by stdout tty-ness per [RFD 048]; this RFD
does not change format resolution.

Condition 3 mirrors the guarantee's scope: tracing writes to stderr directly,
behind the worker's back, and a user opting into live logs has chosen stderr as
a persistent stream where an ephemeral line cannot survive.
The shell knows this at logging setup, before the printer is constructed, so it
feeds the same constructor input as condition 2.

To make condition 2 explicit and testable, terminal capability is a constructor
input: `Printer::terminal` captures stderr's tty-ness at construction, and the
memory constructor accepts an explicit capability override so tests can exercise
draw, clear, and redraw without a real terminal.
The exact constructor shape is an implementation detail.

### Worker integration

The printer already serializes every write — stdout, stderr, and `/dev/tty` —
through one background worker thread (`Worker::run` in `jp_printer::printer`).
That choke point is the entire architectural argument for this design: it is the
only place in the process where "clear chrome, then write content" can be made
atomic with respect to all three channels.

Three changes to the worker:

1. **State.** The worker holds a stack of active status-line entries (claim
   `Instant`, format closure, current detail).
   The top entry is the one rendered.
   Claim, detail-update, and release arrive as new `Command` variants.
2. **Ticking.** The worker's `rx.recv()` becomes `rx.recv_timeout(interval)`
   while a status line is active; on timeout it redraws the top entry with
   updated elapsed time.
   With no active entry, it blocks as today.
   This removes the tokio timer tasks entirely — timing moves to the thread
   that owns the terminal.
3. **Clear-before-write, redraw-after.** Before processing any `Print` task with
   non-empty content, the worker writes `\r\x1b[K` to stderr if a status line is
   drawn; after the task completes, it redraws the line.
   Coexistence with streaming content is therefore at *task* granularity: the
   line redraws between print tasks, and a long typewriter task — one blocking
   loop inside the worker — hides it until the task completes.
   For instant prints (tool chrome, block-at-a-time streaming) this approximates
   the cargo/indicatif model.
   Clients that want disappear-on-content behavior (the waiting indicator)
   simply release the handle at that moment, as they already do.

### Concurrent claimants

Claims form a stack (LIFO): the most recent claim is rendered; releasing it
re-exposes the one below; releasing a non-top entry removes it from the middle.
A stack matches the actual nesting in the code — a tool "preparing" line
claimed during a streaming cycle sits on top of nothing today, but the moment
two indicators overlap (e.g. reasoning timer active when a tool call starts
streaming), LIFO produces the intuitive result without either site knowing about
the other.

### Interactive sessions

JP's prompts do not bypass the printer: prompt output flows through
`Printer::prompt_writer()` / `owned_prompt_writer()` as `PrintTarget::Tty`
tasks, serialized by the same worker.
The clear-before-write rule covers prompts exactly as it covers stdout and
stderr.

Clear-before-write is not sufficient for them, though.
A prompt session is a sequence of small `Tty` writes with the widget owning the
cursor in between; a status-line redraw landing between those writes corrupts
the widget.
Suspension is therefore tied to the prompt-writer boundary, not to call sites:
acquiring a prompt writer (`prompt_writer()` or `owned_prompt_writer()`)
suspends status rendering — the line is cleared and redraws are blocked — for
the writer's lifetime.
Prompt code carries no guard obligation; the prompt sites spread across `jp_cli`
(`ToolPrompter`, the interrupt handler, `cmd/init.rs`, `cmd/target.rs`, the
`conversation` subcommands) need no changes.

Two consequences for the implementation:

- `PrinterWriter` is currently `Copy`; a suspension-carrying writer needs a
  guard type.
  The concrete shape is an implementation detail.
- The lock-contention prompt renders via `err_writer()` today, outside the
  boundary; it migrates to `prompt_writer()` alongside the lock-wait countdown
  (phase 3).

One explicit guard remains, for the single writer genuinely outside the printer:
the external `$EDITOR`, which takes over the terminal as a child process and
touches no prompt writer.

```rust
let _pause = printer.suspend_status(); // clears the line, blocks redraws
// ... run $EDITOR ...
// guard drop: redraw resumes
```

### Migration

All seven sites become clients.
`jp_cli::timer` (`LineTimer`, `spawn_line_timer`, `spawn_tick_sender`) is
deleted once the last client migrates.
The tool "preparing" temp line keeps its separator bookkeeping and its
temp-to-permanent header conversion in `ToolRenderer`; only the draw, tick, and
clear mechanics move to the printer.

## Drawbacks

- **The printer becomes stateful across writes.** Today each `Print` task is
  independent; with this change the worker carries cross-task state (the claim
  stack, drawn/not-drawn) that every write path implicitly interacts with.
  Complexity is conserved ([Tesler]): it moves out of seven call sites into one
  primitive — but bugs in that primitive now affect all chrome at once.
- **`jp_printer` is foundational.** Every crate that prints depends on it; its
  API surface grows, and per Hyrum's Law the rendered chrome format becomes
  something users' scripts may match on.
- **The worker gains a timing loop.** `recv_timeout` polling is cheap but makes
  the worker's behavior time-dependent, which is harder to test than the current
  pure command-processing loop.

## Alternatives

- **Keep the status quo (`LineTimer` + per-site discipline).** The
  waiting-indicator fix shipped this way and works.
  Rejected as the end state: it fixes one site per bug, the clear-ordering
  guarantee needs an async context (`finish().await`) that synchronous renderers
  don't have (see `cancel_reasoning_timer`'s workaround), and the tool temp-line
  machinery remains bespoke.
- **A status-line actor outside the printer.** A separate task owning the line,
  with renderers notifying it before writes.
  Rejected: it recreates the ordering problem it is meant to solve —
  notifications race with writes unless every write goes through the actor, at
  which point it *is* the printer.
- **Last-writer-wins instead of a claim stack.** Simpler, but a released
  claimant would leave the screen blank even when an earlier claimant is still
  logically active.

## Non-Goals

- **Multi-line status regions** (parallel progress bars, spinner groups).
  The concept is deliberately one line; nothing in the design precludes
  extending the region later.
- **Changing what any indicator says or when clients claim/release.** Reasoning
  display modes, waiting-indicator status wording, and lock-wait countdown
  semantics are unchanged; only the mechanics move.
- **Status lines during interactive sessions.** A prompt session suspends the
  status line rather than coexisting with it; rendering chrome alongside an
  active prompt widget is out of scope.

## Risks and Open Questions

- **Typewriter granularity.** Under the task-boundary coexistence contract, a
  long typewriter print hides the status line (and freezes its elapsed display)
  for the task's duration.
  Accepted as a limitation: status lines rarely coexist with typewriter output.
  The known refinement, if it matters in practice, is yielding redraws between
  typewriter batches inside the worker loop.
- **Flicker.** Clear-redraw around every write during heavy streaming could
  flicker on slow terminals.
  Mitigation if observed: skip the redraw when another write is already queued,
  coalescing to one redraw per batch.
- **Windows console.** `\r\x1b[K` handling and the worker's `recv_timeout`
  resolution (~15.6ms scheduler tick) need verification on Windows, same as the
  existing typewriter batching did.

## Implementation Plan

Each phase is independently reviewable and mergeable.

1. **Printer primitive.** Add the claim stack, `Command` variants,
   `recv_timeout` ticking, clear-before-write/redraw-after, the enabling
   predicate, prompt-writer suspension, and the explicit `suspend_status` guard
   to `jp_printer`.
   Unit tests against `Printer::memory` with an explicit terminal-capability
   override.
2. **Waiting indicator.** Migrate the turn loop's indicator (including its
   status transitions) from `LineTimer` to the printer handle.
   The `turn_loop_tests` waiting-indicator suite carries over as the
   characterization tests.
3. **Simple timers.** Migrate the reasoning timer, lock-wait countdown, and both
   drain timers; move the lock-contention prompt from `err_writer()` to
   `prompt_writer()`.
   Delete `spawn_line_timer` and `LineTimer`.
4. **Tool temp line.** Migrate `ToolRenderer`'s preparing line and the
   execution-progress ticker; delete `spawn_tick_sender` and the manual
   rewrite/clear paths (`clear_temp_line`, `rewrite_temp_line`, the
   `line_active` bookkeeping).
   This is the largest phase and depends on phases 1–3 only for the primitive's
   API having settled.

## References

- [RFD 048] — the four-channel output model; defines "chrome" and the printer's
  ownership of stdout/stderr/tty.
- [RFD 088] — the unified editor service and inline reply widget; its
  cursor-owning prompt sessions are what prompt-writer suspension protects, and
  its open widget/printer coordination risk overlaps with the problem addressed
  here.
- `crates/jp_cli/src/timer.rs` — the `LineTimer` interim solution this RFD
  replaces.
- `crates/jp_printer/src/printer.rs` — the worker loop this RFD extends.

[RFD 048]: 048-four-channel-output-model.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[Tesler]: https://en.wikipedia.org/wiki/Law_of_conservation_of_complexity
[`ErrChannel`]: https://github.com/dcdpr/jp/blob/main/crates/jp_printer/src/printer.rs
