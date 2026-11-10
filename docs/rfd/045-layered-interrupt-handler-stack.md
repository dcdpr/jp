# RFD 045: Layered Interrupt Handler Stack

- **Status**: Implemented
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2025-07-24
- **Extended by**: [RFD 092]
- **Summary**: Replaces ad-hoc interrupt handling with a global signal router
  and a LIFO stack of RAII-scoped handlers.

## Summary

This RFD replaces ad-hoc SIGINT handling with a global signal router that
dispatches notifications through a LIFO stack of RAII-scoped handlers executing
in the caller's context.
When no handler is registered, SIGINT triggers graceful shutdown via a
`CancellationToken`.
Unanswered Ctrl-C presses escalate from the active handler to graceful shutdown
and then immediate termination, while handled menu actions reset the escalation
counter.

## Motivation

JP currently handles SIGINT in only two places: LLM streaming and tool
execution.
Both subscribe to a broadcast channel inside `run_turn_loop` and multiplex
signals alongside other events in a `SelectAll` or `mpsc` event loop.

Any phase that does not subscribe to the broadcast channel is a dead zone where
pressing Ctrl-C does nothing.
Before the streaming loop begins (during config loading, editor interaction, MCP
server startup, conversation selection, and the initial HTTP request to start
the LLM stream), nothing consumes signals from the channel.
The same gap occurs between tool execution cycles, during conversation
persistence, and during the transition from `Idle` to `Streaming` in the turn
loop's outer state machine.
Signals buffer in the channel (capacity 128) and are silently discarded on the
next `.resubscribe()`.

This makes Ctrl-C unpredictable: sometimes it opens an interrupt menu, and
sometimes it does nothing, forcing the user to mash the key or wait without a
reliable way to stop the program.

In addition, the two existing interrupt handlers (`handle_streaming_signal` and
`handle_tool_signal`) are standalone functions called from specific points in
the turn loop.
Adding another interrupt context, such as a turn-level handler for graceful turn
shutdown or a handler during editor interaction, requires threading signals to
another ad-hoc consumption point.

## Design

### Overview

The design separates two concerns that the current implementation conflates:

1. **Signal routing**: A global, always-on system that consumes OS signals once,
   manages escalation across repeated Ctrl-C presses, and notifies the active
   handler.
2. **Interrupt handling**: A stack of scoped handlers that execute in the
   caller's context, display context-specific menus, and return an action to the
   surrounding code.

Handlers must execute in the caller's event loop rather than on the router's
async task.
In the current streaming `SelectAll` loop, a signal invokes the handler
synchronously on the same task: it blocks on the interactive terminal menu and
returns an `InterruptAction` that the loop acts on immediately, pausing the loop
so terminal output does not interleave with the menu.
Running handlers in the caller's context preserves this behavior.

### Escalating Ctrl-C

Ctrl-C follows a three-level escalation ladder that counts only unanswered
presses:

| Level | Behavior                                |
| ----- | --------------------------------------- |
| 1st   | Invoke the topmost handler immediately. |
|       | If no handler is registered, trigger    |
|       | graceful shutdown.                      |
| 2nd   | Bypass all handlers and trigger         |
|       | graceful shutdown (cancel the root      |
|       | `CancellationToken`).                   |
| 3rd   | Exit the process immediately            |
|       | (`std::process::exit(130)`).            |

The 2nd level is reached either when the router receives a second SIGINT in
normal terminal mode, or when Ctrl-C cancels a handler's interactive prompt in
raw terminal mode (see [Dual Delivery
Paths](#dual-delivery-paths-and-prompt-escalation)).
Both paths trigger graceful shutdown.

The router's escalation counter resets in two ways:

- **Handled interrupt**: When a handler acts on a press, it reports the press as
  handled and clears the count immediately.
  Opening the streaming menu, choosing "Continue", and pressing Ctrl-C again a
  second later starts a fresh first press rather than triggering shutdown.
  Only unanswered presses accumulate.
- **Cooldown expiry**: A configurable cooldown (2 seconds by default via
  `interrupt.escalation_cooldown_secs`) resets the counter so an unanswered
  press does not trigger escalation minutes later.

SIGTERM and SIGQUIT bypass the handler stack: SIGTERM cancels the root
`CancellationToken` for graceful shutdown, and SIGQUIT exits the process
immediately.
Subsystems that await the `CancellationToken` for teardown (MCP server
connections, conversation persistence, and printer flush) clean up on both
SIGTERM and the 2nd Ctrl-C.

### Signal Router

The `SignalRouter` replaces the current `SignalPair`.
It is created once at application startup (in `Ctx::new`) and lives for the
duration of the process.

```rust
pub struct SignalRouter {
    inner: Arc<RouterInner>,
    _signal_task: tokio::task::JoinHandle<()>,
}

struct RouterInner {
    /// The handler stack.
    stack: Mutex<Vec<RegisteredHandler>>,

    /// Notifies the topmost handler's event loop that SIGINT arrived.
    /// Each handler registration installs its own notification channel; the
    /// router sends to whichever is on top.
    ///
    /// When the stack is empty, the router cancels shutdown\_token instead.
    ///
    /// Escalation state (press count, last timestamp).
    escalation: Mutex<EscalationState>,

    /// Cancelled on graceful shutdown (2nd Ctrl-C, SIGTERM).
    /// Any async code can await this to stop cooperatively.
    shutdown_token: CancellationToken,
}
```

The signal task is a long-lived async task that:

1. Waits for OS signals via `tokio::signal` (same as `os_signals`).
2. On SIGINT: updates escalation state and acts based on the press count.
3. On SIGTERM: cancels `shutdown_token`.
4. On SIGQUIT: calls `std::process::exit`.

On SIGINT, the signal task does not invoke the handler directly.
Instead, it sends a notification over the topmost handler's channel.
The handler's event loop polls this channel alongside its other event sources,
wakes up, and calls the handler in its own context.

When the stack is empty, the signal task cancels `shutdown_token` directly, so
Ctrl-C always takes effect even during config loading or MCP startup.

### Handler Stack

```rust
/// Outcome of an interrupt handler invocation.
pub enum InterruptOutcome {
    /// The handler fully processed the signal.
    /// No further propagation.
    Handled,

    /// The handler declines to act.
    /// The caller should check the next handler on the stack (if any) or fall
    /// back to graceful shutdown.
    Declined,

    /// The handler's interactive prompt was cancelled (Ctrl-C during raw mode).
    /// The caller should trigger graceful shutdown.
    /// See "Dual Delivery Paths and Prompt Escalation."
    Escalated,

    /// The handler's interactive prompt could not run at all.
    /// Nothing was decided, so the caller leaves its work untouched and leaves
    /// the press on the escalation ladder.
    /// See "Dual Delivery Paths and Prompt Escalation."
    PromptFailed,
}

struct RegisteredHandler {
    id: HandlerId,
    /// The router sends an [`InterruptNotice`] to this channel to notify the
    /// handler's event loop that SIGINT arrived.
    /// The event loop then calls the handler and resolves the notice.
    notify_tx: mpsc::Sender<InterruptNotice>,
}
```

The stack stores notification channels rather than handler trait objects.
The handler logic stays in the event loop that registered the channel, avoiding
terminal-interactive calls from the router's async task.

### Handler Registration

Registration returns an RAII guard and a notification receiver:

```rust
impl SignalRouter {
    /// Register a handler scope.
    /// Returns a guard (drop to deregister) and a receiver that yields an
    /// [`InterruptNotice`] when SIGINT arrives while this handler is topmost.
    pub fn push_handler(&self) -> (InterruptGuard, mpsc::Receiver<InterruptNotice>) {
        let (tx, rx) = mpsc::channel(1);
        let id = self.inner.push(tx);
        (InterruptGuard { inner: self.inner.clone(), id }, rx)
    }
}
```

The caller includes `rx` as a stream source in its event loop (for example, as
an arm in `SelectAll` or `select!`).
When `rx` yields a notice, the caller runs its interrupt logic (such as showing
a menu or saving state) synchronously on its own task.

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

`remove` looks up the handler by its monotonic `HandlerId` rather than by stack
position, so the stack remains consistent if an inner guard drops before an
outer one during an early return or panic unwind.

### Resolving a Delivered Press

Each delivered press arrives as an `InterruptNotice`, which the event loop
resolves once its interrupt logic finishes:

```rust
impl InterruptNotice {
    /// The handler acted on this press.
    /// Clears the escalation count.
    pub fn handled(self) { ... }

    /// The handler declined this press.
    /// Notifies the next handler down the stack, or requests a graceful shutdown
    /// when this was the last one.
    /// The press keeps its place on the escalation ladder.
    pub fn decline(self) { ... }
}
```

Dropping a notice without calling `handled()` or `decline()` leaves the press on
the escalation ladder.
If a notified handler exits without taking visible action, the next Ctrl-C press
still escalates.

Putting resolution on the `#[must_use]` `InterruptNotice` rather than on
`SignalRouter` ensures handlers cannot receive a press and forget to update the
escalation state.
Every handler resolves the notice it receives through the same interface,
keeping escalation behavior uniform across handlers.

### Notification Dispatch

When SIGINT arrives on the first press (escalation count 1):

1. The signal task locks the stack, clones the topmost handler's `notify_tx`,
   and releases the lock.
2. It sends an `InterruptNotice` to `notify_tx`.
   If the channel is full because the handler has not consumed the previous
   notification, the send is a no-op: the handler already has a pending
   interrupt, and dropping the undelivered notice leaves the new press on the
   escalation ladder.
3. The handler's event loop wakes up and processes the interrupt.

If the handler's event loop has exited before dropping its guard (a brief window
during cleanup), `notify_tx.send()` fails.
The signal task treats the failed send as a declined interrupt and falls back to
graceful shutdown.

### Concrete Handlers

Existing interrupt handler logic stays in each event loop; only signal routing
changes.

- **Streaming loop**: Registers a handler at the start of the `SelectAll` loop
  and adds the notification receiver as a `StreamSource` variant.
  When notified, the loop runs `handle_streaming_signal` to flush the renderer,
  show the interrupt menu, and act on the user's choice.
  The guard drops when the streaming loop exits.
- **Tool execution**: Registers a handler at the start of
  `execute_with_prompting` and forwards the notification receiver into the
  `ExecutionEvent` channel (matching existing signal forwarding).
  When notified, `handle_tool_signal` checks `is_prompting`, shows the tool
  interrupt menu if appropriate, and returns the chosen action.
  The guard drops when tool execution completes.
- **Turn-level handler (new)**: Registers at the start of `run_turn_loop`,
  outside the streaming and tool handlers.
  As the outermost handler in the turn, it catches signals during the gaps
  between streaming and tool execution, during persistence, and during thread
  building, saving partial state and transitioning to `TurnPhase::Complete`.
- **Empty stack (no handler)**: The router cancels `shutdown_token`.
  Tasks awaiting the token or checking `is_cancelled()` clean up, and
  `Ctx::drop` (printer shutdown and workspace persistence) runs during normal
  process teardown.

### Tool Interrupt Actions

The tool interrupt menu offers four choices:

| Key | Action               | Effect                                     |
| --- | -------------------- | ------------------------------------------ |
| `c` | Continue             | Resume waiting for the running tools.      |
| `r` | Stop & respond       | Cancel the running tools and compose a     |
|     |                      | message sent to the assistant in place of  |
|     |                      | their results.                             |
| `s` | Stop (cancel & exit) | Cancel the running tools, record their     |
|     |                      | cancellation responses, and end the turn   |
|     |                      | without a follow-up request.               |
| `t` | Restart              | Cancel the running tools and run the batch |
|     |                      | again.                                     |

Every cancelled tool call records a response so that every tool call in the
conversation stream has a matching result.
When the user provides a message via "Stop & respond", that message is recorded
for each cancelled call.
When no message is provided (an empty "Stop & respond" reply, a menu-less
`Ctrl+C` during reply composition, or "Stop (cancel & exit)"), each cancelled
call records its configured cancellation response instead:
`conversation.tools.defaults.cancellation_response` sets the global default (a
canned rejection notice asking the assistant to reflect on why the call was
cancelled), and `conversation.tools.<name>.cancellation_response` overrides it
per tool.

Setting `interrupt.tool_call.action` to `continue`, `restart`, `respond`, or
`stop` skips the menu and runs the configured action directly.

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
    │     Ctrl-C → tool menu (Continue/Stop & respond/Stop (cancel & exit)/Restart)
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

Every phase of the turn has an active handler, and phases outside a turn fall
back to `shutdown_token`.

### Dual Delivery Paths and Prompt Escalation

Ctrl-C reaches the application through two mechanisms depending on the terminal
mode:

| Terminal mode       | Ctrl-C becomes | Who sees it    |
| ------------------- | -------------- | -------------- |
| Normal (cooked)     | SIGINT         | Signal router  |
| Raw (during prompt) | Byte `0x03`    | Prompt library |

When a handler shows an interactive menu (such as the streaming or tool
interrupt menu), the prompt library (`jp_inquire`, backed by `inquire`) puts the
terminal in raw mode.
While the menu is displayed, the prompt library consumes Ctrl-C as byte `0x03`,
so the press never reaches the signal router and does not increment its
escalation counter.

In the current implementation, `inline_select(...).unwrap_or('c')` (streaming)
and `.unwrap_or('c')` (tool) treat a cancelled prompt as "Continue".
When the user presses Ctrl-C once to open the menu and a second time to dismiss
it, execution resumes as if nothing happened.

To fix this, **cancelling an interrupt menu with Ctrl-C is treated as an
escalation rather than "Continue"**.
When `inline_select` returns `OperationCanceled`, the handler returns
`InterruptOutcome::Escalated`, and the event loop cancels `shutdown_token`,
matching the router's 2nd-Ctrl-C path.

A prompt that fails to run (because of an I/O error or a missing terminal) must
not escalate, because doing so would abort the run silently without user input.
Instead, the handler returns `PromptFailed`: the event loop leaves its work
untouched and leaves the press on the escalation ladder.
The next Ctrl-C reaches the router with the count intact and escalates through
the ladder, so exiting never depends on the prompt succeeding.

```rust
pub enum InterruptOutcome {
    /// The handler fully processed the signal.
    Handled,

    /// The handler declines to act.
    /// Propagate to the next handler.
    Declined,

    /// The handler's interactive prompt was cancelled by Ctrl-C.
    /// The event loop should trigger graceful shutdown.
    Escalated,

    /// The handler's interactive prompt could not run at all.
    /// The event loop changes nothing and leaves the press on the ladder.
    PromptFailed,
}
```

Whether the 2nd Ctrl-C arrives as SIGINT in normal mode (incrementing the router
count to 2) or as byte `0x03` in raw mode (cancelling the prompt and returning
`Escalated`), both paths cancel `shutdown_token` to begin graceful shutdown.
Once graceful shutdown starts, cleanup runs and the terminal returns to normal
mode while the signal router continues listening for SIGINT; a 3rd Ctrl-C
arrives as SIGINT and triggers immediate process exit:

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

With the `crossterm` backend, `inquire` distinguishes ESC
(`InquireError::OperationCanceled`) from Ctrl-C
(`InquireError::OperationInterrupted`).
Cancelling an interrupt menu with either key escalates.
Every other `InquireError` (`NotTTY`, an I/O error, or a custom error) means the
prompt never ran and maps to `PromptFailed` rather than `Escalated`.

For non-interrupt prompts (tool permissions and tool questions), distinguishing
ESC ("skip this prompt") from Ctrl-C ("open the interrupt menu") remains useful
and requires no upstream change.

### Handler Decline and Propagation

A handler can decline an interrupt.
For example, when `is_prompting` is true, the tool handler suppresses its
interrupt menu so the active tool prompt can handle Ctrl-C.

When an event loop decides not to handle an `InterruptNotice`, it calls
`decline()`, which tells the router to notify the next handler down the stack,
or cancel `shutdown_token` if no handler remains.
Because declining only forwards the press to another handler without acting on
it, the press remains on the escalation ladder and counts toward escalation if
the user presses Ctrl-C again.

### Interaction With RFD 026 and RFD 027

[RFD 026] (Agent Loop Extraction) moves `SignalTo`, the handler stack, the
notification protocol, and the concrete handlers (streaming, tool, and
turn-level) into `jp_agent`.
`SignalRouter` stays in `jp_cli` because OS signal subscription is a
process-level concern, and `jp_agent` either defines a trait or receives a
notification receiver from the caller.

[RFD 027] (Client-Server Query Architecture) changes the interrupt source from
OS signals to `Signal { kind: Interrupt }` messages sent by the client over IPC.
Because notification dispatch is transport-agnostic, the server-side handler
stack works identically when the router accepts interrupts from IPC in addition
to (or instead of) OS signals.

## Drawbacks

- **Shared mutable state**: Flags such as `stream_alive` and `is_prompting` are
  currently passed to handlers as function arguments.
  Because handler code runs in the event loop while other tasks update those
  flags, the state must be shared via atomic references (such as
  `Arc<AtomicBool>`), which adds wiring.
- **Migration cost**: Replacing the existing interrupt handling touches the turn
  loop, the streaming loop, the tool coordinator, and their test suites, though
  the phased implementation plan limits the risk of each step.
- **Decline propagation latency**: When a handler declines, the router notifies
  the next handler down the stack, waking its event loop.
  A channel send and task wake take less than a millisecond, but add a
  round-trip that direct calls do not have.

## Alternatives

### Keep the broadcast channel and add more subscribers

Subscribing to the existing broadcast channel at every gap (in the editor,
during MCP startup, and during persistence) is the smallest change, but leads to
a growing number of ad-hoc subscription points that each interpret signals
independently.
The handler stack covers all current and future gaps with a single mechanism.

### Global `ctrlc` handler with a state enum

Registering a single global handler via the `ctrlc` crate that inspects an
`AtomicU8` application state conflates signal routing with interrupt handling.
Adding a new state requires modifying the central handler, whereas the handler
stack lets callers register new scoped handlers without modifying existing ones.

### `CancellationToken` tree as the primary mechanism

Using parent-child `CancellationToken` relationships for hierarchical
cancellation (where Ctrl-C cancels the innermost token and escalates to the
parent token after a timeout) does not support interactive handling.
`CancellationToken` is a cancellation primitive rather than an interaction
primitive: it can signal "stop", but cannot show a menu and let the user choose.
Instead, the handler stack provides the interaction layer while `SignalRouter`
owns a root `CancellationToken` that is cancelled when the stack is empty, on
the 2nd Ctrl-C, or on SIGTERM.

### Handler trait objects on the stack, invoked by the router

Storing `Box<dyn InterruptHandler>` on the stack and having the signal task call
`handler.handle_interrupt()` directly forces handlers to run on the router's
async task.
Handlers that perform terminal I/O for interactive menus would need
`spawn_blocking` and could not return actions directly to the event loop that
registered them.
Notification channels preserve the current execution flow: the event loop calls
the handler in its own context, has direct access to local state
(`TurnCoordinator`, `ConversationStream`, and `Printer`), and acts on the result
immediately.

## Non-Goals

- **Per-signal handler registration**: Only SIGINT uses the handler stack.
  SIGTERM and SIGQUIT have fixed behavior (graceful shutdown and immediate exit,
  respectively).
- **Handler priorities beyond LIFO**: The stack is strictly last-in-first-out,
  with no numeric priorities or reordering.
  Scope-based guard nesting provides the required ordering automatically.
- **Cross-process interrupt routing**: This RFD covers in-process signal
  handling; [RFD 027]'s IPC interrupt protocol is a separate concern.
- **Async handler execution**: Handlers execute synchronously in the caller's
  event loop, avoiding an `async`-aware stack and dispatch path.

## Risks and Open Questions

- **Handler removal during notification**: If a handler's guard drops after the
  router sends a notification but before the event loop processes it, the event
  loop receives a notification for a handler that is no longer on the stack.
  This is benign: the event loop processes the notification and the router has
  already moved on, with no deadlock or inconsistency.
- **Escalation state across handlers**: The escalation counter tracks Ctrl-C
  presses globally rather than per handler.
  A press answered by the streaming menu clears the count, so a later press
  during tool execution starts fresh and reaches the tool handler.
  An unanswered press carries across handler boundaries: if the streaming menu
  never appeared, the next press escalates even though the tool handler never
  ran.
  That is the intended behavior when an interrupt produced no response, and the
  cooldown bounds how long an unanswered press stays armed.
- **Escalation counter and raw-mode presses**: Because Ctrl-C during a raw-mode
  prompt does not generate SIGINT, the router's SIGINT counter and the handler's
  `Escalated` outcome are independent paths to graceful shutdown.
  Mixing the two paths (for example, if SIGINT arrives during the brief
  normal-mode window while a prompt is being set up) is harmless because the
  window is negligible and both paths trigger graceful shutdown.
  However, the raw-mode path relies on the prompt library reporting Ctrl-C: if a
  prompt library swallowed Ctrl-C without returning an error, the router could
  not see the press to compensate.
  `PromptFailed` covers the observable failure case (where a prompt fails to
  run) by keeping the press on the escalation ladder so the next press
  escalates; closing the silent-swallow gap would require keeping signal
  delivery enabled during prompts in `reedline` and `inquire`.
- **Turn-level handler granularity**: `TurnInterruptHandler` covers all gaps
  between streaming and tool execution, some of which last only a few
  milliseconds during persistence.
  Whether the handler can save partial state in that window depends on what
  state is available at each point; if specific gap phases need custom handling,
  the handler can inspect `TurnPhase`.

## Implementation Plan

### Phase 0: Fix prompt cancellation behavior

Update `handle_streaming_interrupt` and `handle_tool_interrupt` to treat
`OperationCanceled` from `inline_select` as an escalation instead of falling
back to "Continue" via `inline_select(...).unwrap_or('c')`.
Return `InterruptAction::Stop` during streaming or trigger graceful shutdown
during tool execution.
This fixes the double-Ctrl-C bug in the current architecture without waiting for
the full router implementation, has no dependencies, and can be merged
immediately.

### Phase 1: `SignalRouter` with empty-stack fallback

Introduce `SignalRouter` in `jp_cli::signals` to consume OS signals, track
escalation state, and own `shutdown_token`.
With no handlers registered, SIGINT cancels `shutdown_token`.
Update `Ctx::new` to create a `SignalRouter` instead of `SignalPair`, and wire
process teardown to respect `shutdown_token`.

This eliminates signal dead zones across the CLI so Ctrl-C always terminates the
process.
It requires no changes to the turn loop and can be reviewed and merged
independently.

### Phase 2: Migrate streaming handler

Register a handler at the start of the streaming `SelectAll` loop, add the
notification receiver as a `StreamSource` variant, and call
`handle_streaming_signal` when it fires.
Drop the guard when the loop exits and remove the direct broadcast channel
subscription from the streaming loop.

Depends on Phase 1.

### Phase 3: Migrate tool handler

Register a handler at the start of `execute_with_prompting`, forward the
notification receiver into the `ExecutionEvent` channel, and call
`handle_tool_signal` when it fires.
Drop the guard when execution completes.

Depends on Phase 1 and is independent of Phase 2.

### Phase 4: Add turn-level handler

Implement `TurnInterruptHandler` to cover gaps between streaming and tool
execution.
Register it at the start of `run_turn_loop` and drop it at the end; when
notified, save partial state and transition to `TurnPhase::Complete`.
Check for pending notifications between phase transitions with a non-blocking
`try_recv()` on the notification channel at the top of each iteration of the
outer `loop` in `run_turn_loop`.

Depends on Phase 1 and can be done alongside or after Phases 2 and 3.

### Phase 5: Remove legacy signal wiring

Remove `SignalPair`, the broadcast channel, and the signal-forwarding spawned
tasks in `execute_with_prompting`.
Update tests to register handlers via the router.

Depends on Phases 2 through 4.

## References

- [RFD 026: Agent Loop Extraction][RFD 026]: Moves the turn loop and interrupt
  handlers into `jp_agent`.
- [RFD 027: Client-Server Query Architecture][RFD 027]: Changes interrupt
  routing to use IPC messages while keeping handler stack semantics unchanged.
- Current implementation: `crates/jp_cli/src/signals.rs`,
  `crates/jp_cli/src/cmd/query/interrupt/`.

[RFD 026]: 026-agent-loop-extraction.md
[RFD 027]: 027-client-server-query-architecture.md
[RFD 092]: 092-predictable-and-responsive-interrupt-escalation.md
