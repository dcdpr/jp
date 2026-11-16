# RFD 131: Queued Replies

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Extends**: [RFD 045]
- **Requires**: [RFD 104]
- **Extended by**: [RFD 132]
- **Summary**: Adds a `[q] Queue reply` interrupt action and a turn loop that
  sends the queued text as the next turn.

## Summary

A new `[q] Queue reply` interrupt action lets the user compose a message without
stopping the assistant.
The current turn runs to completion, a countdown gives a chance to cancel or
edit, and the queued text then starts a new turn in the same `jp` invocation.
Because the queued text is stored in the conversation's `QUERY_MESSAGE.md`
draft, a bare `jp q` recovers it if the run fails.
A run of several turns resolves its configuration once per turn, and a query
sent over an unsent draft asks before it touches the draft.

## Motivation

Two interrupt actions let the user send a message mid-turn: `[r] Reply` while
streaming, and `[r] Stop & respond` while tools run.
Both stop the assistant to send that message.
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
[s] Stop (save & exit)
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

Every other exit path (fatal provider error, `[s] Stop`, interrupt escalation,
or `query.loop = "never"`) leaves `QUERY_MESSAGE.md` on disk.
Abnormal exits never discard text the user typed, and a subsequent bare `jp q`
automatically seeds from the surviving draft.

**A query sent over an unsent draft.** A query that bypasses composition (a
positional query, piped stdin, or `--no-edit`) writes its own text over
`QUERY_MESSAGE.md` before the turn starts (the recovery copy
`preserve_query_message_file` keeps for a pre-turn interrupt) and removes the
file after a successful turn.
An unsent draft is lost either way: replaced if the turn fails, deleted if it
succeeds.
That breaks the deletion rule above, so such a query no longer writes over an
unsent draft.
On an interactive terminal it asks first, interactive meaning [RFD 104]'s
promptability (a controlling terminal, whatever stdout is), so `jp q foo >
out.md` asks too:

```
QUERY_MESSAGE.md holds an unsent draft.
[s] Send, discard draft  [k] Send, keep draft  [e] Edit draft with query appended  [x] Cancel
```

- `s` sends the query and deletes the draft: the explicit discard.
- `k` sends the query and leaves the draft for a later `jp q`.
- `e` opens the composer `jp q --edit` opens, seeded with the draft, a blank
  line, and the query; submitting sends and consumes the draft like any composed
  query.
- `x` (or Esc) changes nothing and ends the invocation.

Without an interactive terminal nobody can answer, so the query is sent and the
draft is kept: the one choice that cannot lose text.
In every case the query skips its recovery copy while an unsent draft exists.
It came from the command line or a pipe, so the user still has it; the draft is
the text that exists nowhere else.

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
Supporting queued replies requires an outer loop that can run multiple turns in
one invocation.

The turn loop goes above the phase machine, in `Query::run`, around the
`TurnInputs::collect` / `TurnInputs::run` pair that runs one turn today.
Because `run_turn_loop` only drives the phases of a single turn (Idle →
Streaming → Executing → Complete), it is renamed to `run_turn_phases` (and
`turn_loop.rs` to `turn_phases.rs`).
"Turn loop" then refers to the outer loop, matching the `query.loop` config key.

Each iteration of the turn loop collects a fresh `TurnInputs` and rebuilds the
per-turn state inside the phase machine (turn coordinator, tool coordinator,
renderer, and retry budget), preserving the state machine's turn boundary.
`TurnInputs` already describes its config as "the effective configuration for
this turn"; [Each Turn Is a Config Epoch](#each-turn-is-a-config-epoch) makes
that literal.
The queued reply becomes the next turn's `ChatRequest`, stamped with the user
author for transcript attribution just like an interrupt reply.

`run_turn_phases` returns `TurnEnd { Completed, Stopped }` instead of `()`.
Returning `()` hides whether a turn completed normally or ended via `[s] Stop`,
and the turn loop needs that distinction in one place rather than split across
two interrupt handlers.

**A new turn starts only on `TurnEnd::Completed`.** `[s] Stop`, interrupt
escalation, between-phase interrupts, and fatal errors all end the run and leave
the draft file intact.

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

The permissions and answers `TurnState` remembers today (`remember_permission`,
`remembered_answer`) move to the turn-loop driver, and the two inquiry prompt
labels change to "remember for this run".
A tool permission approved with `[Y]` during the first turn carries over to
subsequent queued turns, so an unattended run does not stall on a repeated
permission prompt.
Keeping `Turn` alongside `Run` was rejected because exposing both would require
a new prompt option forcing users to choose between two scopes.

### Each Turn Is a Config Epoch

The resolved `AppConfig` stays immutable; that invariant does not change.
What changes is how long it lives: a run of several turns resolves it once per
turn rather than once per process.

At each turn start the loop builds that turn's config through the existing
pipeline (the cached base layers, a fresh conversation layer from
`ConfigPipeline::partial_with_conversation`, the cached `--cfg` arguments, and
the invocation's `apply_cli_config`) and hands it to `TurnInputs::collect`.
Within the turn nothing re-resolves and nothing mutates.
A config delta recorded during or between turns (a tool's config mutation, a
draft preamble) applies from the next turn, exactly as it would to the next `jp
q` today.

Services the run builds once are valid for a turn only while the config they
were built from is unchanged, and are rebuilt at turn start otherwise:

- Provider clients are cached by provider id **and** that provider's resolved
  config, so a delta that changes its endpoint or credential source under the
  same id rebuilds the client.
- MCP servers are reconciled against each turn's config:
  `configure_active_mcp_servers` runs per turn and yields that turn's
  `StartupSet`.
- `TurnInputs::collect` already admits command plugins and loads attachments
  from the config it is given, so per-turn collection keeps both current.

Settings that cannot change mid-process are named rather than implied.
There is one: `interrupt.escalation_cooldown_secs`, which the `SignalRouter`
captures when it is built.

### Background Tasks

The run holds the conversation lock across all of its turns, as the single-turn
command holds it today.
Two background tasks write to the conversation: the title generator and turn
summaries ([RFD 126]).
Both take the lock themselves when they sync and skip their update on
`AlreadyLocked`, because today they sync after the command returns and the
query's lock is gone.
Under a lock held across turns, both would skip every time.

So the turn loop gives them a per-turn boundary.
At each turn end, before the next turn starts, completed tasks are drained, and
a task targeting the run's conversation syncs through the lock the run already
holds instead of taking its own.
Still-running tasks are left alone between turns; the run's exit keeps today's
drain, cancellation, and grace period.

### Argument Lifetime

A run of several turns has to say what each `jp q` argument applies to:

| Scope           | Arguments                                                                                                                                                                                                                                                                                   |
| --------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Run setup, once | `--new`, `--fork`, `--id`, `--local` / `--no-local`, `--title` / `--no-title`, `--tmp`, pre-query compaction, and `--mount`'s symlinks and approvals                                                                                                                                        |
| First turn only | the positional query, piped stdin, `--replay`, `--quote`, `--template`, `--edit` / `--no-edit`                                                                                                                                                                                              |
| Every turn      | `--schema`, and everything `apply_cli_config` maps to config: `--model`, `--auth`, `--param`, `--reasoning` / `--no-reasoning`, `--tier` / `--no-tier`, `--hide-reasoning`, `--hide-tool-calls`, `-t` / `-T`, `--tool-use` / `--no-tool-use`, `--attachment`, and `--mount`'s access grants |

`--schema` is not config (`apply_cli_config` ignores it): the loop stamps it on
each turn's request, which is what repeated invocations would do.
`--mount` appears twice on purpose: its filesystem side effects run once, and
its access grants take part in every turn's config.
`--edit` and `--no-edit` decide how the first query is composed; later turns are
composed by whatever produced them.

### Send Countdown

Before a queued reply is sent, a countdown runs on the chrome channel:

```
⏳ Sending queued reply in 7s… (^C to review)
```

A status region ([RFD 091]) renders the countdown: it is claimed with `delay =
0`, a short tick interval, and a format closure that renders `duration_secs -
elapsed`, the countdown shape the lock-wait region already uses.
The region ticks itself, so the countdown only has to `select!` over the
completion timer and an interrupt notification from its own handler scope, like
the streaming loop.
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

When the run is not interactive ([RFD 104]), the countdown is skipped and the
queued reply is sent immediately.

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
# - `if_pending`: keep the run going while something is waiting to be sent,
#   such as a queued reply (default).
loop = "if_pending"
```

"Pending" means waiting to be sent, and nothing else.
A queued reply is the one thing this RFD can leave pending, and keeping the run
going for it means sending it as the next turn.
A later source of unsent text extends what `if_pending` waits for without
changing the rule.

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
config key: a workspace can set a default, and `--cfg query.loop=never` is
recorded on the conversation like any other `--cfg`, so it holds for later runs
too.
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
- **Each turn re-collects its inputs.** Attachments reload, plugins are
  re-admitted, and MCP servers are reconciled every turn, as they would be for
  repeated invocations; an attachment that fetches over the network pays that
  cost once per turn.

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

- **Type-ahead.** Typing while the assistant streams, with no interrupt at all.
  A follow-up RFD adds it as an opt-in setting, with an input field kept on
  screen during turns; text typed there is one more thing `if_pending` waits
  for.
  It stays opt-in because, on by default, it would put every `jp q` in raw mode
  and take keystrokes the shell would otherwise receive.
  This RFD adds only the interrupt-menu writer.
- **Composing without pausing output.** Rendering assistant output while taking
  line input needs an input field drawn in the printer's status region ([RFD
  091]), which is the REPL follow-up's subject.
  Pausing terminal output during composition is the intended behavior here.
- **Assistant-initiated messages.** [RFD 083] and [RFD 094] cover tools that let
  the assistant address the user mid-turn, which is a separate mechanism.
- **System notification delivery.** The notification queue in [RFD 011] injects
  system messages into an active turn's thread, whereas a queued reply is a
  user-authored `ChatRequest` that starts a new turn.
- **`always`.** A run that waits for the user's next query at every turn end is
  REPL mode.
  It needs an input field to wait in, so it is a new *value* on this axis, added
  with that field by a follow-up RFD, and it brings the only prompting path with
  it.
- **A `Cancel queued reply` menu entry.** A dedicated menu item shown when a
  reply is pending would allow one-keystroke withdrawal.
  This is deferred unless clearing and submitting the buffer proves too
  cumbersome.
- **Bounding the number of chained queued turns.** Every queued turn requires an
  explicit interrupt and compose step, so the loop cannot run away on its own.

> [!TIP]
> [RFD 132] adds type-ahead as `query.live_input`, an input field kept on screen
> while turns run, and `always` with the field it waits in; `--repl` sets both.

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
- **Countdown legibility.** Rendering a countdown row directly between the
  assistant's final output and the next turn's header needs manual verification
  in a real terminal.
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

Return `TurnEnd` from `run_turn_phases` and add the outer turn loop in
`Query::run`, initially driving a single turn: each iteration builds its config
epoch and collects its `TurnInputs`, and background tasks drain at the turn
boundary, syncing through the run's lock.
Move `QUERY_MESSAGE.md` cleanup from per-turn completion to post-loop exit.
This phase has no user-visible behavior change and can merge on its own.

### Phase 2: Run Scope

Rename `PersistLevel::Turn` to `Run`, move the permissions and answers
`TurnState` remembers to the turn-loop driver, and update the two inquiry prompt
labels and their tests.
This preserves existing behavior while a run executes a single turn.
Depends on Phase 1.

### Phase 3: Queueing

Implement the queue slot, the `QUERY_MESSAGE.md` persistence rules, `[q]` in
both mid-turn interrupt menus, seeded composition, the `queue_reply` variant on
both interrupt action enums, and `query.loop` with `never` and `if_pending`.
Apply the argument-lifetime rules, which first matter once a run holds a second
turn.
Queued replies are sent immediately at turn end without a countdown.
Depends on Phases 1 and 2.

### Phase 4: Send Countdown

Add `interrupt.loop_countdown` (`duration_secs`, `action`, and
`compose_in_editor`), the countdown region, and the countdown interrupt menu
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

### Phase 6: Unsent Drafts on Direct Sends

A query sent without composing no longer writes over an unsent draft: the prompt
on an interactive terminal, keep-the-draft without one, and no recovery copy
while an unsent draft exists.
Independent of every other phase, since it fixes a loss that exists today, so it
can land first.

## References

- [RFD 045]: Layered interrupt handler stack, including the escalation rule used
  by the countdown menu.
- [RFD 088]: Inline editor widget and the `compose_in_editor` setting reused by
  the queued-reply composer.
- [RFD 093]: Query composition and the `QUERY_MESSAGE.md` draft lifecycle.
- [RFD 076]: Tool access grants for narrowing tool permissions beyond run-scoped
  approvals.
- [RFD 091]: Printer status region that renders the countdown.
- [RFD 104]: Promptability, which decides whether the unsent-draft prompt and
  the send countdown are shown.
- [RFD 126]: Turn summaries, one of the two background tasks drained at each
  turn boundary.

[RFD 011]: 011-system-notification-queue.md
[RFD 045]: 045-layered-interrupt-handler-stack.md
[RFD 049]: 049-non-interactive-mode-and-detached-prompt-policy.md
[RFD 070]: 070-negative-config-deltas.md
[RFD 076]: 076-tool-access-grants.md
[RFD 083]: 083-built-in-ask_user-tool-for-assistant-initiated-inquiries.md
[RFD 088]: 088-unified-editor-service-and-inline-reply-widget.md
[RFD 091]: 091-printer-owned-status-region.md
[RFD 093]: 093-inline-first-query-composition.md
[RFD 094]: 094-built-in-tell_user-tool-for-mid-turn-user-addressed-messages.md
[RFD 104]: 104-terminal-capability-and-promptability-signals.md
[RFD 126]: 126-turn-summaries.md
[RFD 132]: 132-repl-mode-with-live-streaming-input.md
