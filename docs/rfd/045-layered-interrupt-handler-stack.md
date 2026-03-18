# RFD 045: Layered Interrupt Handler Stack

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-24

## Summary

This RFD replaces JP's ad-hoc interrupt handling with a layered handler stack. A
global signal router consumes OS signals once and dispatches notifications
through a LIFO stack of scoped handlers. Handlers are registered via RAII guards
and execute in the caller's context (not the router's), preserving the current
ability to show interactive terminal menus and return actions to the surrounding
event loop. When no handler is registered, SIGINT triggers a graceful shutdown
via a `CancellationToken`. Escalating Ctrl-C provides a reliable escape hatch:
first press invokes the handler, second press triggers graceful shutdown, third
press terminates immediately.

## Motivation

JP currently handles SIGINT in two places: during LLM streaming and during tool
execution. Both are wired up inside `run_turn_loop` by subscribing to a
broadcast channel and multiplexing signals alongside other events in a
`SelectAll` or `mpsc` event loop.

This works for those two phases, but leaves gaps. Between the time the CLI
binary starts and the streaming loop begins — during config loading, editor
interaction, MCP server startup, conversation selection, and the HTTP request to
start the LLM stream — nobody consumes signals from the broadcast channel. The
user presses Ctrl-C and nothing happens. The signals buffer in the channel
(capacity 128) and are silently discarded on the next `.resubscribe()`.

The same gap exists between tool execution cycles, during conversation
persistence, and during the transition from `Idle` to `Streaming` in the turn
loop's outer state machine. Any phase that doesn't explicitly subscribe to the
broadcast channel is a dead zone.

The result is unpredictable UX: sometimes Ctrl-C shows an interrupt menu,
sometimes it does nothing, and the user has to mash the key or wait. There is no
reliable "just stop the program" behavior.

A secondary problem: the two existing interrupt handlers
(`handle_streaming_signal` and `handle_tool_signal`) are standalone functions
called from specific points in the turn loop. Adding a third context (e.g., a
turn-level handler for graceful turn shutdown, or an interrupt handler during
editor interaction) requires threading signals to yet another ad-hoc consumption
point. The pattern doesn't scale.

## Design

### Overview

The design separates two concerns that the current implementation conflates:

1. **Signal routing** — consuming OS signals once, managing escalation
   (double/triple Ctrl-C), and notifying the right handler. This is a global,
   always-on system.
2. **Interrupt handling** — showing context-specific menus and returning an
   action to the surrounding code. This is a stack of scoped handlers that
   execute in the caller's context.

The critical constraint: handlers must execute in the event loop's context, not
on the router's async task. Today, when a signal arrives in the streaming
`SelectAll`, the handler is called synchronously on the same task. It shows an
interactive terminal menu (blocking for user input), then returns an
`InterruptAction` that the loop acts on immediately. This pauses the loop during
menu interaction, preventing interleaved terminal output. The new design
preserves this property.

### Escalating Ctrl-C

Ctrl-C follows a three-level escalation:

| Level | Behavior                                 |
|-------|------------------------------------------|
| 1st   | Invoke the topmost handler immediately.  |
|       | If no handler is registered, trigger     |
|       | graceful shutdown.                       |
| 2nd   | Bypass all handlers. Trigger graceful    |
|       | shutdown (cancel the root                |
|       | `CancellationToken`).                    |
| 3rd   | Immediate process exit                   |
|       | (`std::process::exit(130)`).             |

The 2nd level can be reached through two paths: the router receives a second
SIGINT (when the terminal is in normal mode), or a handler's interactive prompt
is cancelled by Ctrl-C (when the terminal is in raw mode, see [Dual Delivery
Paths](#dual-delivery-paths-and-prompt-escalation)). Both produce the same
result: graceful shutdown.

The router's escalation counter resets after a configurable cooldown (e.g. 2
seconds without a Ctrl-C). This prevents a Ctrl-C during streaming (1st press,
handled by the menu) from counting toward escalation minutes later.

SIGTERM always triggers graceful shutdown (cancels the root
`CancellationToken`). SIGQUIT always triggers immediate process exit. Neither
goes through the handler stack. Both are documented here so that systems relying
on the `CancellationToken` for graceful teardown — MCP server connections,
conversation persistence, printer flush — know that SIGTERM and the 2nd Ctrl-C
produce the same signal, giving them a chance to clean up before the process
exits.

### Signal Router

The `SignalRouter` replaces the current `SignalPair`. It is created once at
application startup (in `Ctx::new`) and lives for the duration of the process.

```rust
pub struct SignalRouter {
    inner: Arc<RouterInner>,
    _signal_task: tokio::task::JoinHandle<()>,
}

struct RouterInner {
    /// The handler stack.
    stack: Mutex<Vec<RegisteredHandler>>,

    /// Notifies the topmost handler's event loop that SIGINT arrived.
    /// Each handler registration installs its own notification channel;
    /// the router sends to whichever is on top.
    ///
    /// When the stack is empty, the router cancels shutdown_token instead.
    ///
    /// Escalation state (press count, last timestamp).
    escalation: Mutex<EscalationState>,

    /// Cancelled on graceful shutdown (2nd Ctrl-C, SIGTERM).
    /// Any async code can await this to stop cooperatively.
    shutdown_token: CancellationToken,
}
```

The signal task is a long-lived async task that:

1. Waits for OS signals (via `tokio::signal`, same as today's `os_signals`).
2. On SIGINT: updates escalation state, then acts based on the press count.
3. On SIGTERM: cancels `shutdown_token`.
4. On SIGQUIT: calls `std::process::exit`.

On SIGINT, the signal task does NOT call the handler. It sends a notification to
the handler's event loop via a channel. The handler's event loop — which is
already polling this channel alongside its other sources — wakes up and calls
the handler in its own context.

When the stack is empty (no handler registered), the signal task cancels the
`shutdown_token` directly. This is the "no dead zones" guarantee: Ctrl-C always
does something, even during config loading or MCP startup.

### Handler Stack

```rust
/// Outcome of an interrupt handler invocation.
pub enum InterruptOutcome {
    /// The handler fully processed the signal. No further propagation.
    Handled,

    /// The handler declines to act. The caller should check the next
    /// handler on the stack (if any) or fall back to graceful shutdown.
    Declined,

    /// The handler's interactive prompt was cancelled (Ctrl-C during
    /// raw mode). The caller should trigger graceful shutdown.
    /// See "Dual Delivery Paths and Prompt Escalation."
    Escalated,
}

struct RegisteredHandler {
    id: HandlerId,
    /// The router sends to this channel to notify the handler's event
    /// loop that SIGINT arrived. The event loop then calls the handler.
    notify_tx: mpsc::Sender<()>,
}
```

Handlers are not trait objects stored in the stack. The stack only stores
notification channels. The actual handler logic lives in the event loop that
registered the handler — it knows what to do when notified. This avoids the
problem of calling terminal-interactive code from the router's async task.

### Handler Registration

Registration returns an RAII guard and a notification receiver:

```rust
impl SignalRouter {
    /// Register a handler scope. Returns a guard (drop to deregister)
    /// and a receiver that fires when SIGINT arrives while this handler
    /// is topmost.
    pub fn push_handler(&self) -> (InterruptGuard, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel(1);
        let id = self.inner.push(tx);
        (InterruptGuard { inner: self.inner.clone(), id }, rx)
    }
}
```

The caller includes `rx` as a stream source in its event loop (e.g., as an
additional arm in `SelectAll` or `select!`). When it fires, the caller runs its
interrupt logic (show a menu, save state, whatever is appropriate) on its own
task, synchronously.

The guard deregisters the handler when dropped:

```rust
pub struct InterruptGuard {
    inner: Arc<RouterInner>,
    id: HandlerId,
}

impl Drop for InterruptGuard {
    fn drop(&mut self) {
        self.inner.remove(self.id);
    }
}
```

The `remove` operation uses the `HandlerId` (a monotonic counter), not stack
position. If an inner guard drops before an outer one due to early return or
panic unwinding, the stack remains consistent.

### Notification Dispatch

When SIGINT arrives and the escalation count is 1 (first press):

1. The signal task locks the stack, clones the topmost handler's `notify_tx`,
   and releases the lock.
2. It sends `()` to `notify_tx`. If the channel is full (handler hasn't consumed
   the previous notification), the send is a no-op — the handler already has a
   pending interrupt.
3. The handler's event loop wakes up and processes the interrupt.

If the handler's event loop has exited but the guard hasn't been dropped yet (a
brief window during cleanup), the `notify_tx.send()` fails. The signal task
treats this as "handler declined" and falls back to graceful shutdown.

### Concrete Handlers

The existing interrupt behavior maps onto this system. The handler logic stays
in the event loop — only the *routing* changes.

**Streaming loop.** Registers a handler at the start of the `SelectAll` loop.
The notification receiver is added as another `StreamSource` variant. When it
fires, the loop calls the existing `handle_streaming_signal` logic: flush the
renderer, show the interrupt menu, act on the user's choice. The guard drops
when the streaming loop exits.

**Tool execution.** Registers a handler at the start of
`execute_with_prompting`. The notification receiver is forwarded into the
`ExecutionEvent` channel (same pattern as today's signal forwarding). When it
fires, the existing `handle_tool_signal` logic runs: check `is_prompting`, show
the tool interrupt menu if appropriate, return the action. The guard drops when
execution completes.

**Turn-level handler (new).** Registered at the start of `run_turn_loop`, before
the inner streaming/tool handlers. This handler covers the gaps: between
streaming and tool execution, during persistence, during thread building. When
the notification fires, it saves partial state and transitions to
`TurnPhase::Complete`. It is the outermost handler within the turn, so it
catches signals that the streaming and tool handlers don't.

**No handler (stack empty).** The router cancels the `shutdown_token`. Any code
awaiting the token (or checking `is_cancelled()`) can clean up. The existing
cleanup in `Ctx::drop` (printer shutdown, workspace persistence) runs as part of
normal process teardown.

### Handler Nesting During a Turn

```txt
CLI starts:
  (stack empty — Ctrl-C → shutdown_token → process exits)
  ├── Config loading, MCP startup, editor...
  │
  Turn starts:
    push TurnInterruptHandler
    ├── Streaming cycle:
    │     push StreamingInterruptHandler   ← topmost
    │     ... LLM events flow ...
    │     Ctrl-C → streaming menu (Continue/Reply/Stop/Abort)
    │     drop StreamingInterruptHandler
    ├── (gap: persistence, thread building)
    │     TurnInterruptHandler is topmost
    │     Ctrl-C → save partial, exit turn gracefully
    ├── Tool execution:
    │     push ToolInterruptHandler        ← topmost
    │     ... tools run ...
    │     Ctrl-C → tool menu (Continue/Stop & Reply/Restart)
    │     drop ToolInterruptHandler
    ├── (gap: response processing)
    │     TurnInterruptHandler is topmost
    ├── Follow-up streaming cycle:
    │     push StreamingInterruptHandler
    │     ...
    │     drop StreamingInterruptHandler
    drop TurnInterruptHandler
  Turn ends.
  │
  (stack empty again)
```

At every point, Ctrl-C does something meaningful and does it instantly.

### Dual Delivery Paths and Prompt Escalation

Ctrl-C reaches the application through two different mechanisms depending on the
terminal mode:

| Terminal mode        | Ctrl-C becomes | Who sees it       |
|----------------------|----------------|-------------------|
| Normal (cooked)      | SIGINT         | Signal router     |
| Raw (during prompt)  | Byte `0x03`    | Prompt library    |

When a handler shows an interactive menu (e.g., the streaming or tool interrupt
menu), the prompt library (`jp_inquire`, backed by `inquire`) puts the terminal
in raw mode. While the menu is displayed, Ctrl-C is consumed by the prompt
library as a terminal byte and never reaches the signal router. The router's
escalation counter is not incremented.

Today this produces bad UX. The current code handles a cancelled prompt with
`inline_select(...).unwrap_or('c')` (streaming) or `.unwrap_or('c')` (tool),
which silently falls back to "Continue." The user presses Ctrl-C twice — once to
open the menu, once to dismiss it — and tool execution resumes as if nothing
happened.

The fix: **a Ctrl-C that cancels an interrupt menu is an escalation, not a
"continue."** When `inline_select` returns `OperationCanceled`, the handler
returns a new `InterruptOutcome::Escalated` variant. The event loop receives
this and cancels the `shutdown_token`, producing the same effect as the router's
2nd-Ctrl-C path.

```rust
pub enum InterruptOutcome {
    /// The handler fully processed the signal.
    Handled,

    /// The handler declines to act. Propagate to the next handler.
    Declined,

    /// The handler's interactive prompt was cancelled by Ctrl-C.
    /// The event loop should trigger graceful shutdown.
    Escalated,
}
```

This bridges the two delivery paths: whether the 2nd Ctrl-C arrives as SIGINT
(normal mode → router increments to 2 → graceful shutdown) or as byte `0x03`
(raw mode → prompt cancelled → handler returns `Escalated` → graceful shutdown),
the outcome is the same.

For a 3rd Ctrl-C: graceful shutdown cancels the `shutdown_token`, cleanup runs,
and the terminal returns to normal mode. The signal router is still listening
for SIGINT. The next Ctrl-C arrives through the normal SIGINT path and triggers
immediate process exit.

The full escalation sequence when a handler is showing a prompt:

```text
1. Ctrl-C (normal mode)
   → SIGINT → router (escalation=1) → notify handler
   → handler shows interactive menu (terminal enters raw mode)

2. Ctrl-C (raw mode, menu is showing)
   → byte 0x03 → prompt library → OperationCanceled
   → handler returns Escalated
   → event loop cancels shutdown_token (graceful shutdown begins)
   → terminal returns to normal mode

3. Ctrl-C (normal mode, cleanup is running)
   → SIGINT → router (escalation=2) → std::process::exit(130)
```

Note: `jp_inquire` currently wraps `inquire::InquireError::OperationCanceled`,
which conflates ESC and Ctrl-C. Both produce the same error. For interrupt menus
this is fine — cancelling an interrupt menu by any means should escalate. For
non-interrupt prompts (tool permissions, tool questions), distinguishing ESC
("skip this prompt") from Ctrl-C ("I want the interrupt menu") would be
valuable. An upstream change request to `inquire` to distinguish the two keys is
worth pursuing but is not a dependency for this RFD.

### Handler Decline and Propagation

A handler can decline to process the interrupt. The tool handler already does
this: when `is_prompting` is true, it suppresses the interrupt menu to let the
active tool prompt handle Ctrl-C.

With the new model, decline works as follows: the event loop receives the
notification, evaluates whether it should handle the interrupt, and if not,
calls `signal_router.decline()`. This tells the router to notify the next
handler down the stack. If no handler remains, the router cancels the
`shutdown_token`.

```rust
impl SignalRouter {
    /// Called by a handler's event loop when it declines to handle the
    /// current interrupt. The router notifies the next handler on the
    /// stack, or falls back to graceful shutdown.
    pub fn decline(&self) {
        self.inner.notify_next_or_shutdown();
    }
}
```

### Interaction With RFD 026 and RFD 027

[RFD 026] (Agent Loop Extraction) moves `SignalTo` and the interrupt handlers
into `jp_agent`. The handler stack, the notification protocol, and the concrete
handler logic (streaming, tool, turn-level) all belong in `jp_agent`. The
`SignalRouter` itself stays in `jp_cli`, since OS signal subscription is a
process-level concern. `jp_agent` defines a trait or receives a notification
receiver from the caller.

[RFD 027] (Client-Server Query Architecture) changes the interrupt source: the
client sends `Signal { kind: Interrupt }` over IPC instead of receiving an OS
signal. On the server side, the handler stack works identically — the router
accepts interrupts from IPC in addition to (or instead of) OS signals. The
notification-based dispatch model is transport-agnostic.

## Drawbacks

**Shared mutable state.** The `stream_alive` flag, the `is_prompting` flag, and
similar state that handlers need to make decisions must be shared between the
handler's event loop and the state that the handler inspects. Today this is
passed as function arguments. With the layered model, some of this state needs
to be in shared references (e.g., `Arc<AtomicBool>`) since the handler code runs
in the event loop but the state is updated by other tasks. This is
straightforward but adds wiring.

**Migration cost.** The existing interrupt handling code works and is
well-tested. Replacing it requires touching the turn loop, the streaming loop,
and the tool coordinator. The tests need to be updated. This is not a small
change, though the phased implementation plan mitigates risk.

**Decline propagation latency.** When a handler declines, the router must notify
the next handler down the stack. That handler's event loop must then wake up and
process the notification. In practice this is sub-millisecond (it's a channel
send + task wake), but it adds a round-trip that doesn't exist today.

## Alternatives

### Keep the broadcast channel, add more subscribers

Add signal consumption at every gap point: subscribe in the editor, subscribe
during MCP startup, subscribe during persistence. This is the minimal change.

Rejected because it leads to an unbounded number of ad-hoc subscription points,
each with its own interpretation of what to do with a signal. The set of gaps
grows as the application grows. The layered model handles all current and future
gaps with a single mechanism.

### Global `ctrlc` handler with a state enum

Use the `ctrlc` crate to register a single global handler. Store the current
application state in an `AtomicU8` or similar. The handler checks the state and
acts accordingly.

Rejected because a single handler with a state enum conflates routing and
handling. Adding a new state requires modifying the central handler. The layered
model is open for extension — new handlers are added without modifying existing
ones.

### CancellationToken tree as the primary mechanism

Use `CancellationToken` parent-child relationships for hierarchical
cancellation. Pressing Ctrl-C cancels the innermost token; if no one handles it
within a timeout, the parent token is cancelled.

Rejected as the *primary* mechanism because `CancellationToken` is a
cancellation primitive, not an interaction primitive. It can signal "stop" but
not "show a menu and let the user choose." The handler stack is the interaction
layer; `CancellationToken` complements it for non-interactive shutdown. The
`SignalRouter` owns a root `CancellationToken` that is cancelled when the
handler stack is empty, on the 2nd Ctrl-C, or on SIGTERM.

### Handler trait objects on the stack, invoked by the router

Store `Box<dyn InterruptHandler>` on the stack and have the signal task call
`handler.handle_interrupt()` directly.

Rejected because it forces handlers to run on the router's async task. Handlers
that do terminal I/O (interactive menus) would need `spawn_blocking`, and they
couldn't return actions directly to the event loop that registered them. The
notification model preserves the current execution flow: the event loop calls
the handler in its own context, has direct access to local state
(`TurnCoordinator`, `ConversationStream`, `Printer`), and acts on the result
immediately.

## Non-Goals

- **Per-signal handler registration.** Only SIGINT uses the handler stack.
  SIGTERM and SIGQUIT have fixed behavior (graceful shutdown and immediate exit,
  respectively).

- **Handler priorities beyond LIFO.** The stack is strictly last-in-first-out.
  No numeric priorities, no reordering. Scope-based guard nesting provides the
  right ordering automatically.

- **Cross-process interrupt routing.** [RFD 027]'s IPC interrupt protocol is a
  separate concern. This RFD covers in-process signal handling.

- **Async handler execution.** Handlers execute synchronously in the caller's
  event loop. Async handlers would require the stack to be `async`-aware and
  complicate the dispatch for minimal benefit.

## Risks and Open Questions

**Handler removal during notification.** If a handler's guard is dropped between
the router sending the notification and the event loop processing it, the event
loop receives a notification for a handler that no longer exists on the stack.
This is benign — the event loop processes the notification, does its work, and
the guard is already gone. The router has already moved on. No deadlock or
inconsistency.

**Escalation state across handlers.** The escalation counter tracks Ctrl-C
presses globally. A 1st press during streaming (handled by the streaming menu)
followed by a 2nd press during tool execution would trigger graceful shutdown,
even though the tool handler never got a chance. This is correct — the user is
escalating — but might surprise users who expect each handler to get a "fresh"
first press. The cooldown timer mitigates this: if enough time passes between
presses, the counter resets.

**Escalation counter and raw-mode presses.** Because Ctrl-C during a raw-mode
prompt doesn't generate SIGINT, the router's escalation counter and the
prompt-based escalation (`Escalated` outcome) are two independent paths to the
same result. The router counts SIGINT-delivered presses; the handler detects
prompt-cancelled presses. Both trigger graceful shutdown. The risk is that a
sequence mixing the two paths (e.g., SIGINT arrives during a brief normal-mode
window while the prompt is being set up) could behave unexpectedly. In practice
the window is negligible, and the outcome (graceful shutdown) is correct
regardless of which path triggers it.

**Turn-level handler granularity.** The `TurnInterruptHandler` covers all gaps
between streaming and tool execution. Some of these gaps are very brief (a few
milliseconds of persistence). Whether the handler can meaningfully act in such a
short window (save partial state) depends on what state is available at that
point. During the implementation, we may find that some gap phases need specific
logic. The handler can inspect `TurnPhase` to decide what to do.

## Implementation Plan

### Phase 0: Fix prompt cancellation behavior

Change the existing `handle_streaming_interrupt` and `handle_tool_interrupt`
methods to treat `OperationCanceled` from `inline_select` as an escalation
instead of falling back to "Continue." This is a bug fix independent of the
layered handler model — it can be done immediately with the current
architecture.

Replace `inline_select(...).unwrap_or('c')` with explicit error handling that
returns `InterruptAction::Stop` (streaming) or triggers graceful shutdown
(tool). This fixes the existing double-Ctrl-C UX problem without waiting for the
full router implementation.

No dependencies. Can be merged immediately.

### Phase 1: SignalRouter with empty-stack fallback

Introduce `SignalRouter` in `jp_cli::signals`. It consumes OS signals, tracks
escalation state, and owns a `shutdown_token`. With no handlers registered,
SIGINT cancels the shutdown token. Wire `Ctx::new` to create a `SignalRouter`
instead of the current `SignalPair`. Wire the existing process teardown to
respect the shutdown token.

At this point, Ctrl-C works everywhere — it just kills the process. This alone
fixes the worst UX issue (dead zones where Ctrl-C does nothing).

Can be reviewed and merged independently. No changes to the turn loop.

### Phase 2: Migrate streaming handler

Register a handler at the start of the streaming `SelectAll` loop. Add the
notification receiver as a `StreamSource` variant. When it fires, call the
existing `handle_streaming_signal` logic. Drop the guard when the loop exits.

Remove the direct broadcast channel subscription from the streaming loop —
signals now arrive via the router's notification channel.

Depends on Phase 1.

### Phase 3: Migrate tool handler

Register a handler at the start of `execute_with_prompting`. Forward the
notification receiver into the `ExecutionEvent` channel. When it fires, call the
existing `handle_tool_signal` logic. Drop the guard when execution completes.

Depends on Phase 1. Independent of Phase 2.

### Phase 4: Add turn-level handler

Implement the `TurnInterruptHandler` that covers gaps between streaming and tool
execution. Register it at the start of `run_turn_loop`, drop it at the end. This
handler saves partial state and transitions to `TurnPhase::Complete`.

The outer `loop` in `run_turn_loop` needs to check for pending notifications
between phase transitions. This can be done with a non-blocking `try_recv()` on
the notification channel at the top of each loop iteration.

Depends on Phase 1. Can be done alongside or after Phases 2–3.

### Phase 5: Remove legacy signal wiring

Remove `SignalPair`, the broadcast channel, and the signal-forwarding spawned
tasks in `execute_with_prompting`. Update tests to register handlers via the
router.

Depends on Phases 2–4.

## References

- [RFD 026: Agent Loop Extraction][RFD 026] — moves the turn loop and interrupt
  handlers into `jp_agent`.
- [RFD 027: Client-Server Query Architecture][RFD 027] — changes interrupt
  routing to use IPC messages; handler stack semantics remain the same.
- Current implementation: `crates/jp_cli/src/signals.rs`,
  `crates/jp_cli/src/cmd/query/interrupt/`.

[RFD 026]: 026-agent-loop-extraction.md
[RFD 027]: 027-client-server-query-architecture.md
