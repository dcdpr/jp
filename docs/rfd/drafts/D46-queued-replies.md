# RFD D46: Queued Replies

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Extends**: [RFD 045]
- **Summary**: Adds a `[q] Queue reply` interrupt action that composes the next
  turn's message without stopping the current turn.

## Summary

A new `[q] Queue reply` interrupt action lets the user compose a message without
stopping the assistant.
The current turn runs to completion, a countdown gives a chance to cancel or
edit, and the queued text then starts a new turn in the same `jp` invocation.
Because the queued text is stored in the conversation's `QUERY_MESSAGE.md`
draft, a bare `jp q` recovers it if the run fails.

## Motivation

Two interrupt actions let the user send a message mid-turn: `[r] Reply` while
streaming, and `[r] Stop & respond` while tools run.
Both abort the rest of the current turn to send that message.
There is no way to let the assistant finish its turn and queue what to do next.

This matters when the user wants to step away from the keyboard.
When the assistant is working through a long turn and its direction is already
clear, the user may know the next instruction before the turn ends.
Today, sending that instruction without discarding the current response requires
waiting for the turn to finish and then running `jp q`.

## Design

### Behavior

Both interrupt menus gain a `[q] Queue reply` entry:

```
Interrupted
[c] Continue  [r] Reply (stop & respond)  [q] Queue reply (finish, then respond)
[s] Stop (save & exit)  [a] Abort (discard & exit)
```

```
Interrupted
[c] Continue  [r] Stop & respond  [q] Queue reply (finish, then respond)
[s] Stop (cancel & exit)  [t] Restart
```

Pressing `[q]` opens the same compose surface as `[r]` (the inline widget or
external editor, configured by that menu's `compose_in_editor` setting).
Once the user submits or cancels, the turn resumes where it left off: streaming
continues and running tools keep executing.

In configuration, both menus use the action name `queue_reply`.
The immediate-response actions use different names because their side effects
differ (`reply` stops the stream, whereas `respond` cancels running tools), but
`queue_reply` behaves identically in both contexts.

As with `[r]`, terminal output pauses while the user composes and resumes after
submission.
The provider stream is not polled while the composer is open.
If a long compose drops the connection, the existing retry path handles recovery
(retrying on a transient error or continuing from the partial response).

### Queue Slot

A conversation holds at most one queued reply, not a list.
Pressing `[q]` when a reply is already queued seeds the composer with the
existing text.

| Input                  | Effect                                            |
| ---------------------- | ------------------------------------------------- |
| Submit, unchanged      | Keep the existing queued text.                    |
| Submit, edited         | Replace the queued text.                          |
| Submit, buffer emptied | Clear the slot, remove the draft, return to menu. |
| `Ctrl+C`               | Leave the slot unchanged and return to menu.      |

To withdraw a queued reply, the user clears the buffer and submits.
`Ctrl+C` cancels the edit and returns to the interrupt menu without modifying
the slot, matching the `[r]` reply path; using `Ctrl+C` to withdraw the reply
would make it the only place in the interrupt system where `Ctrl+C` discards
user-authored text.

### Draft File Lifecycle

Queued text is persisted to the conversation's `QUERY_MESSAGE.md` draft file
(the same file managed by `preserve_query_message_file`), with the in-memory
slot acting as a cache.
Two rules govern the file's lifetime:

- The file is written when a reply is queued.
- The file is deleted only when the queued reply is **sent** as a new turn or
  when the user **explicitly discards** it.

Every other exit path (fatal provider error, `[s] Stop`, `[a] Abort`, interrupt
escalation, or `query.loop = "never"`) leaves `QUERY_MESSAGE.md` on disk.
Abnormal exits never discard text the user typed, and a subsequent bare `jp q`
automatically seeds from the surviving draft.

Preserving the draft across turns requires two changes to existing
`QUERY_MESSAGE.md` handling:

- **Post-loop cleanup.** `cleanup_query_message_file` currently runs at the end
  of every successful turn, which would delete a reply queued during that turn.
  Draft cleanup moves to the outer turn loop and runs only after the loop exits
  with an empty slot.

  This rule also replaces `DraftRemoval::IfUnchanged`, which fingerprints the
  draft when `jp query` composes a request and deletes the file on turn
  completion only if the fingerprint is unchanged (preserving drafts written
  mid-turn via the plugin protocol's `write_draft`).
  Checking whether the slot is empty after the turn loop covers both cases, so
  the fingerprint check can be removed.

- **Preamble preservation.** `QUERY_MESSAGE.md` is a `QueryDocument` containing
  a config preamble and query text, whereas `preserve_query_message_file` writes
  raw text.
  Writing a queued reply must preserve any existing preamble along with its
  `<!-- CONFIG_SEED: ... -->` comment, which records the config values used to
  pre-fill the preamble.
  When the draft is reopened, `jp query` compares the preamble against that seed
  to distinguish the user's config edits from stale pre-filled values ([RFD
  093]).
  Dropping the seed comment would discard those edits.

Crash recovery requires user-local storage.
`preserve_query_message_file` writes only when a user-local store is configured
so scratch files never land in the committed workspace directory.
Without user-local storage, queued replies still work in memory, and only crash
recovery is unavailable (matching existing failed-turn recovery).

### Turn Loop and Run Scope

`jp query` currently runs a single turn per invocation: `run_turn_loop` returns
when the turn phase reaches `Complete`, and nothing above it loops.
Supporting queued replies requires an outer loop in `handle_turn` that can run
multiple turns in one invocation.

Because `run_turn_loop` only drives the phases of a single turn (Idle →
Streaming → Executing → Complete), it is renamed to `run_turn_phases` (and
`turn_loop.rs` to `turn_phases.rs`).
"Turn loop" then refers to the outer loop in `handle_turn`, matching the
`query.loop` config key.

Each iteration of the turn loop rebuilds the per-turn state (turn coordinator,
tool coordinator, renderer, and retry budget), preserving the state machine's
turn boundary.
The queued reply becomes the next turn's `ChatRequest`, stamped with the user
author for transcript attribution just like an interrupt reply.

`run_turn_phases` returns `TurnEnd { Completed, Stopped, Aborted }` instead of
`()`.
Returning `()` hides whether a turn completed normally or ended via `[s] Stop`
or `[a] Abort`, and the turn loop needs that distinction in one place rather
than split across two interrupt handlers.

**A new turn starts only on `TurnEnd::Completed`.** `[s] Stop`, `[a] Abort`,
interrupt escalation, between-phase interrupts, and fatal errors all end the run
and leave the draft file intact.

Running multiple turns in one invocation also affects inquiry persistence.
`PersistLevel` is currently defined as:

```rust
pub enum PersistLevel { None, Turn }
```

When an invocation runs only one turn, turn scope and invocation (`Run`) scope
are identical.
With multi-turn runs, they diverge.
Rather than adding a third variant, this RFD renames `Turn` to `Run`:

```rust
pub enum PersistLevel { None, Run }
```

`TurnState::persisted_inquiry_responses` moves to the turn-loop driver, and the
two inquiry prompt labels change to "remember for this run".
A tool permission approved with `[Y]` during the first turn carries over to
subsequent queued turns, so an unattended run does not stall on a repeated
permission prompt.
Keeping `Turn` alongside `Run` was rejected because exposing both would require
a new prompt option forcing users to choose between two scopes.

### Send Countdown

Before a queued reply is sent, a countdown runs on the chrome channel:

```
⏳ Sending queued reply in 7s… (^C to review)
```

`spawn_line_timer` drives the countdown with `delay = 0`, a short tick interval,
and a format closure that renders `duration_secs - elapsed`.
Like the streaming loop, the countdown runs a `select!` over the completion
timer, the tick stream, and an interrupt notification from its own
`signals.push_handler()` scope.
Because the countdown runs between turns after the completed turn is flushed,
interrupting it cannot lose committed work.

Pressing `Ctrl+C` during the countdown opens a third interrupt menu:

```
Queued reply
[n] Send now  [e] Edit draft  [k] Keep draft (don't send)  [d] Discard draft
```

The keys `s` and `c` are omitted because they mean Stop and Continue in the
mid-turn interrupt menus; binding them to Send or Cancel here would invert their
meaning.
`[d] Discard draft` is the only action that deletes the queued text.

`[e] Edit draft` lets the user revise the queued reply after seeing the
assistant's completed response.
It cancels the countdown timer before opening the composer (configured by
`interrupt.loop_countdown.compose_in_editor`) so the timer cannot fire mid-edit.
Submitting modified text updates the draft and returns to this menu with the
timer stopped, since the user is actively at the keyboard.
Pressing `Ctrl+C` in the composer leaves the draft unchanged and returns to the
menu.
Submitting an empty buffer clears the slot, deletes the draft (equivalent to
`[d] Discard draft`), and ends the invocation.

Pressing `Ctrl+C` at the countdown menu escalates ([RFD 045]), keeping the draft
on disk and starting a graceful shutdown.
Escalation never discards the draft.

When running without a TTY, the countdown is skipped and the queued reply is
sent immediately, matching `spawn_waiting_indicator`.

### Configuration

Both mid-turn interrupt action enums accept `queue_reply`:

```toml
[interrupt.streaming]
action = "queue_reply"

[interrupt.tool_call]
action = "queue_reply"
```

Turn-end looping is configured under `[query]`, since it is a property of the
invocation rather than of `Ctrl+C`:

```toml
[query]
# When to run another turn after one ends.
#
# - `never`: end the invocation. A queued reply stays as a draft for the next
#   `jp q`.
# - `if_pending`: run another turn when a reply is queued (default).
loop = "if_pending"
```

The `loop` key answers a single question (when to start another turn), so its
variants lie on one axis, and `loop = "never"` reads more clearly than a key
such as `run = "none"`.

Setting `loop = "never"` turns `[q]` into a mid-turn scratchpad: the user can
compose notes while the assistant works and keep them as the draft for the next
`jp q` without sending them automatically.
When a turn ends with a saved draft under `loop = "never"`, JP prints a chrome
line confirming that the draft was kept.

Nothing mutates `query.loop` at runtime.
It is a static preference read at each turn boundary and layered like any other
config key: a workspace can set a default, and `--cfg query.loop=never`
overrides it for a single invocation.
Per-turn control lives in the countdown menu, where `[k] Keep draft` declines
sending a specific reply (just as `y` approves a single tool call while `Y`
remembers the choice).

**`query.loop` is a turn-end policy, not a startup policy.** A draft left behind
by a failed run is not sent automatically on the next `jp q`; instead, it seeds
the composer according to [RFD 093]'s draft rules for the user to review and
submit.
Otherwise, `if_pending` would silently send stale text at the start of a new
invocation.

The countdown has its own interrupt section:

```toml
[interrupt.loop_countdown]
# How long the countdown before sending runs.
# Set to `0` to send immediately, with no countdown and no chance to interrupt.
duration_secs = 10

# What Ctrl-C does during the countdown.
#
# - `prompt`: show the menu (default).
# - `send`: send the queued reply immediately.
# - `keep`: keep the draft and end the invocation.
# - `discard`: delete the draft and end the invocation.
# - `edit`: open the composer, then restart the countdown.
action = "prompt"

# Where `[e] Edit draft` composes: inline widget or external editor.
# Accepts `true`/`false` or `"always"`/`"never"`, as in the other contexts.
compose_in_editor = false
```

Editor selection follows one rule across all three interrupt contexts: **each
menu's compose action uses that menu's own `compose_in_editor` setting.**
Pressing `[q]` uses the setting of the active mid-turn menu (`streaming` or
`tool_call`), and pressing `[e]` uses
`interrupt.loop_countdown.compose_in_editor`.
The turn loop does not need to track which menu created the queued reply, and
pressing `[q]` a second time from the other mid-turn menu requires no special
handling.

Setting `query.loop = "never"` makes all three `interrupt.loop_countdown` keys
unreachable.
Setting `duration_secs = 0` skips the countdown window and sends immediately,
which makes `action` and `compose_in_editor` unreachable.

`[interrupt.loop_countdown]` is named after the countdown window rather than the
queued reply because the window owns the timer and the interrupt handler.
Although the countdown currently runs only when a reply is queued, other
turn-end actions (such as automatic compaction) could share this window later,
turning the menu entry into a `[q] Queued reply →` submenu while keeping shared
actions at the top level.
A dedicated queued-reply section would instead give each turn-end action its own
timer, leaving behavior undefined when two actions are pending at the same turn
boundary.
Placing `duration_secs` in an `[interrupt]` section follows the precedent of
`InterruptConfig::escalation_cooldown_secs`.

[RFD 093] also introduces the `[query]` section; whichever RFD lands first
creates `[query]`, and `compose_in_editor` and `loop` are independent keys
within it.

## Drawbacks

- **Crash recovery requires user-local storage.** Without a user-local store,
  there is no safe place to write a scratch file, so a queued reply lost to a
  crash cannot be recovered (though in-memory queueing still works).
- **Public API and refactoring churn.** Renaming `PersistLevel::Turn` to `Run`
  changes a public type in `jp_tool` and two user-visible prompt labels, and
  renaming `run_turn_loop` to `run_turn_phases` moves `turn_loop.rs` to
  `turn_phases.rs`.
  The new turn boundary requires both renames, but they add churn to an already
  structural diff.
- **Run-scoped approvals cover more tool calls.** A permission approval keyed by
  tool name carries over to later turns in the same run, even if the tool is
  called with different arguments.
  The approval is still bounded to a single invocation rather than persisted,
  and narrowing tool permissions by argument belongs to tool access grants ([RFD
  076]) rather than `PersistLevel`.
- **A third interrupt menu.** Users encounter a third `Ctrl+C` menu with its own
  keybindings right after the two mid-turn menus.
- **The turn loop reuses the resolved `AppConfig`.** If a tool mutates the
  conversation config during a turn, the queued turn does not pick up that
  change.
  This matches how `[r] Reply` behaves within a turn, but it remains a known
  limitation.

## Alternatives

- **Transition `Complete → Idle` inside the phase machine.** Looping inside the
  phase machine would make turn-scoped state (coordinator, retry budget, pending
  tools, and replay trim) run-scoped, requiring manual resets between turns and
  collapsing the turn boundary.
- **Thread the queued text through return types** (`InterruptAction` →
  `ToolInterruptResult` → `ExecutionResult` → turn loop).
  This attaches a message payload to `ExecutionOutcome` that is unrelated to
  tool execution.
  Giving the turn-loop driver a single-slot sink keeps `InterruptHandler` pure
  and leaves both result types unchanged.
- **Persist the queued reply, exit, and let the next `jp q` pick it up.**
  Avoiding a turn loop leaves the user to start the next turn manually, and
  silently prepending a stale message to a later query would be surprising.
  Users who want scratchpad behavior without automatic sending can set
  `query.loop = "never"`.
- **A `persist_permissions` config key.** This duplicates the per-prompt choice
  that already exists (`y` to approve once, `Y` to remember for the run).
- **A top-level `[queued_reply]` section.** Adding a top-level config section is
  unnecessary when `[query]` and `[interrupt]` already provide a clear home for
  each key.
- **`interrupt.loop_countdown.auto_send` as a boolean.** Although this keeps all
  settings in one section, turn-end looping is not `Ctrl+C` behavior, and a
  boolean cannot express a future `always` variant for REPL mode.
- **Store the queued text in `query.message`.** Recording the queued reply in
  config requires two `config_delta` events per reply (one to set it and one to
  clear it) and duplicates the message in both the config delta and the eventual
  `ChatRequest`.
  Because config deltas are permanent, the text remains in the conversation
  history forever, and clearing the field cleanly would require negative config
  deltas ([RFD 070]) rather than an empty-string sentinel.
- **A `QueuedRequest` event kind.** A plain `ChatRequest` is on the
  `is_provider_visible` allowlist and would be sent to the provider mid-turn
  like an immediate reply, so staging the message in the conversation stream
  would require a new non-provider-visible `EventKind`.
  Consuming that event at turn end would then require popping it and appending a
  `ChatRequest`, performing non-append mutations on an append-only log to use it
  as temporary storage.
- **Store the pending reply in conversation metadata.** Conversation metadata
  can be mutated without a delta trail, but `jp conversation ls` reads every
  conversation's metadata file, and storing a message body there would bloat
  listing reads.
- **Store `query.loop` in conversation metadata.** Metadata's only advantage
  over config is runtime mutation without a delta trail, and nothing mutates
  `query.loop` at runtime.
  Treating `query.loop` as mutable state would risk overwriting the user's
  configured preference (for example, resetting `always` after consuming a
  queued reply) and would still require a separate flag for whether a reply is
  pending, which the existence of `QUERY_MESSAGE.md` already indicates.
  Using `jp_config` also provides config layering, `--cfg` overrides, and
  generated `config.toml` and JSON Schema documentation.
- **Name the action "follow-up" or "deferred reply".** `Action::SendFollowUp`
  already names the intra-turn provider cycle after tool execution, and `defer`
  names the detached prompt policy ([RFD 049]).
  "Queued reply" pairs directly with `[r] Reply`: same noun, different delivery
  timing.

## Non-Goals

- **Type-ahead.** Allowing the user to type while the assistant streams without
  pressing `Ctrl+C` first builds on the queue slot and turn loop introduced
  here, but this RFD only adds the interrupt-menu entry.
- **Composing without pausing output.** Streaming assistant output while reading
  line input requires a printer-owned status region ([RFD 091]).
  Pausing terminal output during composition is the intended behavior.
- **Assistant-initiated messages.** [RFD 083] and [RFD 094] cover tools that let
  the assistant address the user mid-turn, which is a separate mechanism.
- **System notification delivery.** The notification queue in [RFD 011] injects
  system messages into an active turn's thread, whereas a queued reply is a
  user-authored `ChatRequest` that starts a new turn.
- **`query.loop = "always"`.** Always starting another turn and prompting the
  user when no reply is queued is REPL mode.
  That mode can be added later as a third variant on `query.loop` along with its
  turn-boundary prompt.
- **A `Cancel queued reply` menu entry.** A dedicated menu item shown when a
  reply is pending would allow one-keystroke withdrawal.
  This is deferred unless clearing and submitting the buffer proves too
  cumbersome.
- **Bounding the number of chained queued turns.** Every queued turn requires an
  explicit interrupt and compose step, so the loop cannot run away on its own.

## Risks and Open Questions

- **Draft file data loss.** Moving `QUERY_MESSAGE.md` cleanup to the turn loop
  and preserving the config preamble on write are both paths where a bug could
  silently delete user-authored text or config edits.
  Both paths require integration tests verifying that the draft survives every
  abnormal exit.
- **Behavior after submitting an empty buffer.** Submitting an empty buffer
  clears the queued reply and returns to the interrupt menu, matching the
  existing `Empty | Cancelled if menu` branch.
  Pressing `Ctrl+C` also returns to the interrupt menu without modifying the
  slot.
- **Countdown legibility.** Rendering a `\r`-based countdown line directly
  between the assistant's final output and the next turn's header needs manual
  verification in a real terminal.
- **Interaction with [RFD 093].** [RFD 093] reshapes query composition and the
  `QUERY_MESSAGE.md` lifecycle.
  Because queued replies write the same file that [RFD 093] reads, both designs
  must agree on preamble handling.
  This RFD assumes [RFD 093]'s rules: the submitted buffer replaces the query
  body, and reopening the draft reconciles the config preamble against its
  `CONFIG_SEED` comment, which requires queued-reply writes to preserve both the
  preamble and its seed.

## Implementation Plan

### Phase 1: Turn Loop

Rename `run_turn_loop` to `run_turn_phases` and `turn_loop.rs` to
`turn_phases.rs` so "turn loop" refers to the outer driver (this rename can be
dropped without affecting the rest of the phase).

Return `TurnEnd` from `run_turn_phases`, add the outer turn loop in
`handle_turn` (initially driving a single turn), and move `QUERY_MESSAGE.md`
cleanup from per-turn completion to post-loop exit.
This phase has no user-visible behavior change and can merge on its own.

### Phase 2: Run Scope

Rename `PersistLevel::Turn` to `Run`, move
`TurnState::persisted_inquiry_responses` to the turn-loop driver, and update the
two inquiry prompt labels and their tests.
This preserves existing behavior while a run executes a single turn.
Depends on Phase 1.

### Phase 3: Queueing

Implement the queue slot, the `QUERY_MESSAGE.md` persistence rules, `[q]` in
both mid-turn interrupt menus, seeded composition, the `queue_reply` variant on
both interrupt action enums, and `query.loop` with `never` and `if_pending`.
Queued replies are sent immediately at turn end without a countdown.
Depends on Phases 1 and 2.

### Phase 4: Send Countdown

Add `interrupt.loop_countdown` (`duration_secs`, `action`, and
`compose_in_editor`), the countdown timer, and the countdown interrupt menu
(`[n]`, `[e]`, `[k]`, `[d]`) with its escalation rule.
Selecting `[e]` from the menu cancels the timer and returns to the blocking menu
after editing, whereas `action = "edit"` restarts the countdown after editing
because no menu was open.
Depends on Phase 3.

### Phase 5: Documentation

Update `docs/configuration.md` and the interrupt documentation, and add **Queued
Reply** and **Run** entries to the ubiquitous-language glossary.
The **Run** entry defines a run as a single `jp` invocation executing one or
more turns, and distinguishes the outer turn loop (which runs turns within a
run) from `run_turn_phases` (which runs phases within a turn).

## References

- [RFD 045]: Layered interrupt handler stack, including the escalation rule used
  by the countdown menu.
- [RFD 088]: Inline editor widget and the `compose_in_editor` setting reused by
  the queued-reply composer.
- [RFD 093]: Query composition and the `QUERY_MESSAGE.md` draft lifecycle.
- [RFD 076]: Tool access grants for narrowing tool permissions beyond run-scoped
  approvals.

[RFD 011]: ../011-system-notification-queue.md
[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 083]: ../083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 088]: ../088-unified-editor-service-and-inline-reply-widget.md
[RFD 091]: ../091-printer-owned-status-region.md
[RFD 093]: ../093-inline-first-query-composition.md
[RFD 094]: ../094-built-in-tell_user-tool-for-mid-turn-user-addressed-messages.md
