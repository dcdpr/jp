# RFD D46: Queued Replies

- **Status**: Draft
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Extends**: [RFD 045]

## Summary

A new interrupt action, `[q] Queue reply`, composes a message without stopping
the assistant.
The turn runs to completion, a countdown gives one last chance to cancel, and
the queued text then starts a new turn in the same `jp` invocation.
The queued text lives in the conversation's `QUERY_MESSAGE.md` draft, so a
failed run leaves it recoverable with a bare `jp q`.

## Motivation

Two interrupt actions exist for "I want to say something": `[r] Reply` while
streaming, and `[r] Stop & respond` while tools run.
Both stop the assistant to do it.
There is no way to say "keep going, and here is what to do next."

The use case that motivates this is stepping away from the keyboard.
The assistant is working through a long turn, the direction is already clear,
and the user wants the next instruction queued so the run continues without
them.
Today that requires waiting for the turn to end and typing `jp q` — which means
being present at exactly the moment the turn finishes.

Doing nothing keeps every mid-turn message an interruption in the literal sense:
the only way to add context is to discard the rest of the response.

## Design

### Behavior

Both interrupt menus gain one entry:

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

`[q]` opens the same compose surface as `[r]` — inline widget or external
editor, following that context's existing `compose_in_editor` setting — and
then returns to exactly what was happening before.
Streaming resumes, running tools keep running.
Nothing about the turn's trajectory changes.

`queue_reply` is the one action whose meaning is identical in both contexts.
Its neighbours differ deliberately (`reply` stops the stream, `respond` cancels
tools), which is why the same identifier is correct in two places where the
surrounding vocabulary diverges.

The terminal stays silent while composing, as it does for `[r]` today; assistant
output resumes after submitting.
The provider stream is not polled during that window, so a long compose can lose
the connection — that routes through the existing retry path (transient error
→ retry, or continue from the partial response) and needs no special handling.

### One slot, seeded

There is one queued reply, not a list.
A second `[q]` seeds the widget with the text already queued.

| Input                  | Effect                                         |
| ---------------------- | ---------------------------------------------- |
| Submit, unchanged      | Slot keeps the same text.                      |
| Submit, edited         | Slot replaced.                                 |
| Submit, buffer emptied | Slot cleared, draft removed. Back to the menu. |
| `Ctrl+C`               | Slot unchanged. Back to the menu.              |

`Ctrl+C` means "back up a level, change nothing", exactly as it does on the
reply path.
Withdrawing a queued reply is the emptied-submit row: clear the buffer, press
Enter.
`Ctrl+C` is deliberately not the withdraw path — it would be the only place in
the interrupt system where `Ctrl+C` destroys text the user typed.

### The draft file is the slot

The queued text is written to the conversation's `QUERY_MESSAGE.md`, the same
file `preserve_query_message_file` already manages.
The in-memory slot is a cache over it.
Two rules govern its lifetime:

- The file is written when a reply is queued.
- The file is deleted only when the queued reply is **sent** as a new turn, or
  when the user **explicitly discards** it.

Every other path — fatal provider error, `[s] Stop`, `[a] Abort`, escalation,
`query.loop = "never"` — leaves the file alone.
Nothing on an abnormal path destroys text the user typed, and manual resumption
comes for free: after any failure, a bare `jp q` seeds from the draft.

Two existing behaviors need care here, and they are where a data-loss bug would
come from:

- `cleanup_query_message_file` runs when a turn succeeds and would delete a
  queued reply.
  Ownership of the file's lifecycle moves to the turn loop: clean up after the
  loop, only when the slot is empty.
- The file is a `QueryDocument` (config preamble plus query text), while
  `preserve_query_message_file` writes raw content.
  Writing a queued reply must preserve an existing preamble, which is the same
  problem [RFD 093] is solving in this file.

Recovery is conditional: `preserve_query_message_file` only writes when
user-local storage is configured, because scratch must never land in the
committed workspace directory.
Without a user-local store the feature still works and only crash recovery
degrades — a gap today's failed-turn recovery already has.

### The turn loop and the Run scope

`jp query` runs exactly one turn per invocation: `run_turn_loop` returns when
the phase reaches `Complete`, and nothing above it loops.
A queued reply needs a driver that runs one *or more* turns, which is the
structural part of this proposal.

The turn loop goes above the phase machine, in `handle_turn`.
That makes `run_turn_loop` a misnomer — it drives the phases within one turn
(Idle → Streaming → Executing → Complete), not a loop over turns — so it
becomes `run_turn_phases` and `turn_loop.rs` becomes `turn_phases.rs`.
"Turn loop" then names the outer driver, matching the `query.loop` config key.

Each iteration rebuilds the per-turn pieces that are already constructed there
or inside the phase machine (turn coordinator, tool coordinator, renderer, retry
budget), so the turn boundary the state machine is built around stays intact.
The queued reply becomes the next turn's `ChatRequest`, author-stamped for
transcript attribution the way an interrupt reply is today.

`run_turn_phases` gains a `TurnEnd { Completed, Stopped, Aborted }` return.
Today its caller cannot tell a natural completion from a user Stop or Abort —
all three are `Ok(())` — and the consumption policy needs that distinction in
one place rather than scattered across two interrupt handlers.

**A new turn starts only when the previous turn ended on its own.** Stop, Abort,
escalation, a between-phase interrupt, and fatal errors end the run with the
draft intact.

A run containing several turns forces a scope that did not previously exist.
`PersistLevel` today is:

```rust
pub enum PersistLevel { None, Turn }
```

`Turn` and a hypothetical `Run` are indistinguishable while a run holds one
turn, so no user has an expectation about the difference.
This RFD makes them differ, and resolves it by renaming rather than adding:

```rust
pub enum PersistLevel { None, Run }
```

`TurnState::persisted_inquiry_responses` moves to the turn-loop driver, and the
two prompt labels become "remember for this run".
A permission approved with `[Y]` in the first turn therefore holds for the
queued turn, which is the point: an unattended run that stops at "are you sure?"
defeats the feature.

Adding `Run` alongside `Turn` was rejected — nothing would emit it without also
adding a prompt option asking users to distinguish two scopes they have never
been able to distinguish.

### The send countdown

Before a queued reply is sent, a countdown runs on the chrome channel:

```
⏳ Sending queued reply in 7s… (^C to review)
```

`spawn_line_timer` drives it: `delay = 0`, a short interval, and a format
closure rendering `duration_secs - elapsed`.
The window is a `select!` over the completion sleep, the timer ticks, and an
interrupt notification from its own `signals.push_handler()` scope, matching the
shape of the streaming loop.
It runs between turns, after the previous turn is flushed, so an interrupt there
cannot lose committed work.

Ctrl-C during the countdown opens a third interrupt menu:

```
Queued reply
[n] Send now  [e] Edit draft  [k] Keep draft (don't send)  [d] Discard draft
```

`s` and `c` are deliberately not used: they mean Stop and Continue in the two
menus that appear seconds earlier in the same flow, and rebinding them here
would invert them.
`[d]` is the only destructive choice in the whole feature.

`[e]` opens the compose surface seeded with the queued text, following this
menu's own `interrupt.loop_countdown.compose_in_editor`.
It cancels the countdown on the way in: a timer that fires mid-compose would
send the text the user is in the middle of replacing.
Saving returns to this menu with no countdown running — the user pressed
Ctrl-C, so they are at the keyboard, and a blocking menu matches the two
interrupt menus that came before it.
`Ctrl+C` in the composer leaves the queued text unchanged and returns here.
An emptied submit clears the slot, which leaves nothing to send: that is `[d]`
by another route, and the invocation ends.

This is the only place a queued reply can be revised after the turn ends, which
is exactly when the full response is finally visible.

Ctrl-C on this menu escalates ([RFD 045]): keep the draft and begin a graceful
shutdown.
Escalation never takes the destructive branch.

Without a tty the countdown is skipped and the reply is sent immediately,
matching how `spawn_waiting_indicator` behaves.

### Configuration

Two new values on the existing action enums:

```toml
[interrupt.streaming]
action = "queue_reply"

[interrupt.tool_call]
action = "queue_reply"
```

What happens when a turn ends is a `[query]` key, because it is a property of
the invocation rather than of Ctrl-C:

```toml
[query]
# When to run another turn after one ends.
#
# - `never`: end the invocation. A queued reply stays as a draft for the next
#   `jp q`.
# - `if_pending`: run another turn when a reply is queued (default).
loop = "if_pending"
```

The values answer one question — when do you loop? — so they order on a single
axis, and `never` reads unambiguously where a key named `run` with a value
`none` would not.

`loop = "never"` turns the feature into a mid-turn scratchpad: compose notes
while the assistant works, keep them as the next query's draft, never send them
automatically.
A chrome line at turn end says so, otherwise there is no signal the notes
survived.

Nothing writes `query.loop` at runtime.
It is a preference read at each turn boundary and layered like any other config
key, so a workspace can default it and `--cfg query.loop=never` can override one
invocation.
Per-instance control lives in the countdown menu instead: `[k] Keep draft`
declines one specific send, the way `y` approves one specific tool call where
`Y` remembers.

**`query.loop` is a turn-end policy, not a startup policy.** A draft left behind
by a failed run does not fire on the next `jp q`; it seeds the composer per [RFD
093]'s draft rules and the user submits it.
Without this rule, `if_pending` would silently send stale text at invocation
start.

The countdown keeps its own interrupt context:

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

The compose rule is uniform across all three contexts: **each menu's compose
actions use that menu's `compose_in_editor`.** `[q]` reads the setting of the
menu it was pressed in, and `[e]` reads the countdown menu's own.
Nothing has to remember where a queued reply came from, and a second `[q]` from
a different menu than the first is not a special case.

Reachability, documented because dead keys are worse than absent ones:
`query.loop = "never"` makes all three keys unreachable, and `duration_secs = 0`
makes `action` and `compose_in_editor` unreachable — with no countdown there is
no Ctrl-C window and therefore no menu.

The section is named for the window rather than for the queued reply because the
window is what has a duration and an interrupt.
The two are co-extensive today: no queued reply, no countdown.
If a second interruptible turn-end subject appears — automatic compaction has
the same shape, something is about to happen and the user may want to stop it —
it slots in under this section, which already owns the timer, and the flat menu
becomes `[q] Queued reply →` with the shared outcomes staying at the top level.
Naming a sibling section instead would give each subject its own countdown, with
no defined behavior when two are pending at one turn boundary.

`duration_secs` is a timing key in a section otherwise about Ctrl-C behavior,
which `InterruptConfig::escalation_cooldown_secs` already establishes as
acceptable.

[RFD 093] also introduces `[query]`.
Whichever lands first creates the section; `compose_in_editor` and `loop` are
independent keys within it.

## Drawbacks

- **Crash recovery is conditional on user-local storage.** Without it there is
  no safe place for a scratch file, so a queued reply lost to a crash is not
  recoverable.
  The feature itself still works.
- **Renaming `PersistLevel::Turn` changes a `jp_tool` public type** and two
  user-visible prompt labels.
  Small, but it is a contract.
- **Two renames land alongside the feature.** `PersistLevel::Turn` → `Run` and
  `run_turn_loop` → `run_turn_phases`, the second taking a module file with it.
  Both are corrections the new turn boundary forces, but they are churn in a
  diff that is already structural.
- **Run-scoped approvals widen the unattended window.** A decision keyed by tool
  name now covers a later turn's calls with different arguments.
  Bounded by the invocation, not durable, and narrowing it is the job of access
  grants ([RFD 076]) and argument-conditional policy, not this axis.
- **A third interrupt menu** to learn, seconds after the first two.
- **The turn loop reuses the resolved `AppConfig`.** A tool that mutated
  conversation config mid-turn does not affect the queued turn.
  Consistent with how `[r] Reply` behaves within a turn today, but it is a known
  gap.

## Alternatives

- **Transition `Complete → Idle` inside the phase machine.** Turn-scoped state
  (coordinator, retry budget, pending tools, replay trim) would become
  run-scoped and need manual resetting, collapsing the boundary the state
  machine is built around.
- **Thread the queued text through return types** (`InterruptAction` →
  `ToolInterruptResult` → `ExecutionResult` → turn loop).
  Puts a payload on `ExecutionOutcome` that has nothing to do with tool
  execution, entangling two independent axes.
  A single-slot sink owned by the driver keeps `InterruptHandler` pure and
  touches neither result type.
- **Persist the queued reply, exit, and let the next `jp q` pick it up.** No
  turn loop, but it is not the feature: a stale message silently prepended to a
  later query is a surprise.
  This is what `query.loop = "never"` opts into deliberately.
- **A `persist_permissions` config key.** Redundant with the per-press choice
  the prompt already offers: `y` is "just this once", `Y` is "remember".
- **A top-level `[queued_reply]` section.** A new top-level section is a cost
  the project is deliberately avoiding, and both `[query]` and `[interrupt]`
  already give each key an unambiguous home.
- **`interrupt.loop_countdown.auto_send` as a boolean.** Keeps every key for the
  feature in one section, but the turn-end policy is not Ctrl-C behavior, and a
  boolean cannot express the `always` case that turns the loop into a REPL.
- **`query.message` as the medium.** Storing the queued text as a config value
  costs two `config_delta` events per queued reply — one setting it, one
  clearing it — and leaves the message stored twice after consumption: in the
  delta and in the `ChatRequest` that was actually sent.
  Config deltas are never removed, so the text stays in the conversation's
  config history for its lifetime, and clearing the field cleanly needs [RFD
  070] rather than an empty-string sentinel.
- **A `QueuedRequest` event kind.** Needs a new non-provider-visible
  `EventKind`, because a plain `ChatRequest` is on the `is_provider_visible`
  allowlist and would be sent to the provider mid-turn as if it were an
  immediate reply.
  Consuming it means popping it and re-appending it as a `ChatRequest`: three
  mutations of an append-only log, two of which are not appends.
  A queued reply is not a conversation event yet, and staging one in the event
  log makes that log temporary storage.
- **The pending request in conversation metadata.** Mutable with no delta trail,
  but the metadata record is the small document read for every `jp conversation
  ls`, and a prose body inflates every listing read.
- **`query.loop` in conversation metadata.** Metadata's only advantage over
  config is runtime mutation without a delta trail, and nothing mutates
  `query.loop` at runtime.
  Making it runtime state would let mechanism overwrite intent — a queued reply
  consumed under `always` would reset the user's own preference — and would
  need a second field for "is something pending", which the draft's existence
  already answers.
  Config additionally gives layering, `--cfg`, and the generated-`config.toml`
  and JSON-schema documentation channel that a metadata field has none of.
- **Naming it "follow-up" or "deferred reply".** `Action::SendFollowUp` already
  means the intra-turn provider cycle, and `defer` is the detached prompt policy
  ([RFD 049]).
  "Queued Reply" composes with the existing `Reply`: same noun, different
  delivery timing.

## Non-Goals

- **Type-ahead.** Typing while the assistant streams, with no interrupt at all,
  is the natural end state.
  This RFD builds the slot and the turn loop it would need, but adds only the
  interrupt-menu writer.
- **Composing without pausing output.** Rendering assistant output while taking
  line input needs a printer-owned status line ([RFD 091]).
  Silence during compose is the intended behavior here, not a limitation to fix.
- **Assistant-initiated messages.** [RFD 083] and [RFD 094] cover the assistant
  addressing the user mid-turn; unrelated mechanism.
- **System notification delivery.** [RFD 011]'s queue injects system messages
  into an existing turn's thread.
  A queued reply is a user-authored `ChatRequest` that starts a new turn.
- **`query.loop = "always"`.** The third value on the axis — always run another
  turn, prompting when nothing is pending — is REPL mode.
  It is a new *value* on the axis this RFD introduces, not a new axis, and it
  brings the only prompting path with it.
- **A `Cancel queued reply` entry in the interrupt menus.** Shown only while a
  reply is pending, it would be a one-keystroke withdraw.
  Deferred until emptying the buffer proves to be a common gesture.
- **Bounding the number of chained queued turns.** Each one requires an explicit
  interrupt and compose, so there is no runaway.

## Risks and Open Questions

- **The draft-file lifecycle is the sharp edge.** Moving `QUERY_MESSAGE.md`
  ownership to the turn loop, and preserving a config preamble on write, are
  both places where a mistake silently deletes text the user typed.
  Both need tests that assert the file survives each abnormal path.
- **Re-showing the menu after an emptied submit.** Proposed: return to the
  interrupt menu, matching how `Empty | Cancelled if menu` behaves today.
  `Ctrl+C` returns to the same place without touching the slot.
- **Countdown legibility.** A `\r`-based countdown line immediately after the
  assistant's final output, and immediately before a new turn's header, needs a
  look on a real terminal.
- **Interaction with [RFD 093].** Active work there reshapes query composition
  and the draft's role.
  The queued reply writes the same file 093 reads, so the two need to agree on
  preamble handling; this RFD assumes 093's rule that the buffer wins for query
  text and the preamble is preserved untouched.

## Implementation Plan

### Phase 1: the turn loop

Rename `run_turn_loop` to `run_turn_phases` and `turn_loop.rs` to
`turn_phases.rs`, freeing "turn loop" for the outer driver.
Droppable without affecting the rest of the phase.

Add `TurnEnd` to `run_turn_phases`'s return.
Add the turn loop in `handle_turn`, driving exactly one turn.
Move `QUERY_MESSAGE.md` cleanup from per-turn to post-loop.
No user-visible change; mergeable alone.

### Phase 2: the Run scope

Rename `PersistLevel::Turn` to `Run`, move
`TurnState::persisted_inquiry_responses` to the turn-loop driver, update the two
prompt labels and their tests.
Behavior-preserving while a run holds one turn.
Depends on phase 1.

### Phase 3: queueing

The slot, the draft-file rules, `[q]` in both menus, seeded compose, the
`queue_reply` value on both action enums, and `query.loop` with `never` and
`if_pending`.
Sends immediately at turn end (no countdown yet).
Depends on phases 1–2.

### Phase 4: the countdown

`interrupt.loop_countdown` with all three keys, the countdown timer, and the
third interrupt menu (`[n]`, `[e]`, `[k]`, `[d]`) with its escalation rule.
`[e]` cancels the timer on entry and returns to a menu with no countdown
running; the configured `edit` action restarts it instead, having no menu to
return to.
Depends on phase 3.

### Phase 5: documentation

`docs/configuration.md`, the interrupt documentation, and a **Queued Reply**
entry in the ubiquitous-language glossary.
A **Run** entry too: one `jp` invocation, containing one or more Turns — a term
this RFD forces into existence.
The entry names which loop is which: the turn loop runs Turns within a Run,
`run_turn_phases` runs phases within a Turn.

## References

- [RFD 045] — the layered interrupt handler stack this builds on, including the
  escalation rule the countdown menu follows.
- [RFD 088] — the inline editor widget and the `compose_in_editor` spectrum the
  compose surface reuses.
- [RFD 093] — query composition and the `QUERY_MESSAGE.md` draft lifecycle this
  RFD writes into.
- [RFD 076] — tool access grants, the axis for narrowing what run-scoped
  approvals cover.

[RFD 011]: ../011-system-notification-queue.md
[RFD 045]: ../045-layered-interrupt-handler-stack.md
[RFD 049]: ../049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 070]: ../070-negative-config-deltas.md
[RFD 076]: ../076-tool-access-grants.md
[RFD 083]: ../083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 088]: ../088-unified-editor-service-and-inline-reply-widget.md
[RFD 091]: ../091-printer-owned-status-line.md
[RFD 093]: ../093-inline-first-query-composition.md
[RFD 094]: ../094-built-in-tell_user-tool-for-mid-turn-user-addressed-messages.md
