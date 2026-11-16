# RFD 131: Queued Replies

- **Status**: Discussion
- **Category**: Design
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-07-31
- **Extends**: [RFD 045]
- **Requires**: [RFD 104]
- **Extended by**: [RFD 132]
- **Summary**: Adds a Queue reply interrupt action and a turn loop that sends
  the queued text as the next turn.

## Summary

A new interrupt action, `[q] Queue reply`, composes a message without stopping
the assistant.
The turn runs to completion, a countdown gives one last chance to cancel, and
the queued text then starts a new turn in the same `jp` invocation.
The queued text lives in the conversation's `QUERY_MESSAGE.md` draft, so a
failed run leaves it recoverable with a bare `jp q`.
A run of several turns resolves its configuration once per turn, and a query
sent over an unsent draft asks before it touches the draft.

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
[s] Stop (save & exit)
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

Every other path (fatal provider error, `[s] Stop`, escalation, `query.loop =
"never"`) leaves the file alone.
Nothing on an abnormal path destroys text the user typed, and manual resumption
comes for free: after any failure, a bare `jp q` seeds from the draft.

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

Two existing behaviors need care here, and they are where a data-loss bug would
come from:

- `cleanup_query_message_file` runs when a turn succeeds and would delete a
  queued reply.
  Ownership of the file's lifecycle moves to the turn loop: clean up after the
  loop, only when the slot is empty.

  A narrower guard already stands in for this: `jp query` fingerprints the draft
  when it composes its request (`DraftRemoval::IfUnchanged`) and a successful
  turn removes the file only while it still matches, so a draft written mid-turn
  through the plugin protocol's `write_draft` survives.
  The slot-based rule replaces it, and the fingerprint plumbing comes out with
  the same change.

- The file is a `QueryDocument` (config preamble plus query text), while
  `preserve_query_message_file` writes raw content.
  Writing a queued reply must preserve an existing preamble together with its
  `<!-- CONFIG_SEED: ... -->` comment, the config the preamble was pre-filled
  with.
  The next open reconciles the preamble against that config seed to tell the
  user's config edits apart from stale pre-filled values ([RFD 093]); a preamble
  written back without it loses those edits.

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

The turn loop goes above the phase machine, in `Query::run`, around the
`TurnInputs::collect` / `TurnInputs::run` pair that runs one turn today.
That makes `run_turn_loop` a misnomer — it drives the phases within one turn
(Idle → Streaming → Executing → Complete), not a loop over turns — so it
becomes `run_turn_phases` and `turn_loop.rs` becomes `turn_phases.rs`.
"Turn loop" then names the outer driver, matching the `query.loop` config key.

Each iteration collects a fresh `TurnInputs` and rebuilds the per-turn pieces
inside the phase machine (turn coordinator, tool coordinator, renderer, retry
budget), so the turn boundary the state machine is built around stays intact.
`TurnInputs` already describes its config as "the effective configuration for
this turn"; [Each turn is a config epoch](#each-turn-is-a-config-epoch) makes
that literal.
The queued reply becomes the next turn's `ChatRequest`, author-stamped for
transcript attribution the way an interrupt reply is today.

`run_turn_phases` gains a `TurnEnd { Completed, Stopped }` return.
Today its caller cannot tell a natural completion from a user Stop (both are
`Ok(())`), and the consumption policy needs that distinction in one place rather
than scattered across two interrupt handlers.

**A new turn starts only when the previous turn ended on its own.** Stop,
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

The permissions and answers `TurnState` remembers today (`remember_permission`,
`remembered_answer`) move to the turn-loop driver, and the two prompt labels
become "remember for this run".
A permission approved with `[Y]` in the first turn therefore holds for the
queued turn, which is the point: an unattended run that stops at "are you sure?"
defeats the feature.

Adding `Run` alongside `Turn` was rejected — nothing would emit it without also
adding a prompt option asking users to distinguish two scopes they have never
been able to distinguish.

### Each turn is a config epoch

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

### Background tasks

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

### Argument lifetime

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

### The send countdown

Before a queued reply is sent, a countdown runs on the chrome channel:

```
⏳ Sending queued reply in 7s… (^C to review)
```

It is a status region ([RFD 091]) claimed with `delay = 0`, a short interval,
and a format closure rendering `duration_secs - elapsed`, the countdown shape
the lock-wait region already uses.
The region ticks itself; the window is a `select!` over the completion sleep and
an interrupt notification from the countdown's own handler scope, matching the
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

Without an interactive terminal ([RFD 104]) the countdown is skipped and the
reply is sent immediately.

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
# - `if_pending`: keep the run going while something is waiting to be sent,
#   such as a queued reply (default).
loop = "if_pending"
```

"Pending" means waiting to be sent, and nothing else.
A queued reply is the one thing this RFD can leave pending, and keeping the run
going for it means sending it as the next turn.
A later source of unsent text extends what `if_pending` waits for without
changing the rule.

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
key: a workspace can default it, and `--cfg query.loop=never` is recorded on the
conversation like any other `--cfg`, so it holds for later runs too.
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
- **Each turn re-collects its inputs.** Attachments reload, plugins are
  re-admitted, and MCP servers are reconciled every turn, as they would be for
  repeated invocations; an attachment that fetches over the network pays that
  cost once per turn.

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
  Silence during compose is the intended behavior here, not a limitation to fix.
- **Assistant-initiated messages.** [RFD 083] and [RFD 094] cover the assistant
  addressing the user mid-turn; unrelated mechanism.
- **System notification delivery.** [RFD 011]'s queue injects system messages
  into an existing turn's thread.
  A queued reply is a user-authored `ChatRequest` that starts a new turn.
- **`always`.** A run that waits for the user's next query at every turn end is
  REPL mode.
  It needs an input field to wait in, so it is a new *value* on this axis, added
  with that field by a follow-up RFD, and it brings the only prompting path with
  it.
- **A `Cancel queued reply` entry in the interrupt menus.** Shown only while a
  reply is pending, it would be a one-keystroke withdraw.
  Deferred until emptying the buffer proves to be a common gesture.
- **Bounding the number of chained queued turns.** Each one requires an explicit
  interrupt and compose, so there is no runaway.

> [!TIP]
> [RFD 132] adds type-ahead as `query.live_input`, an input field kept on screen
> while turns run, and `always` with the field it waits in; `--repl` sets both.

## Risks and Open Questions

- **The draft-file lifecycle is the sharp edge.** Moving `QUERY_MESSAGE.md`
  ownership to the turn loop, and preserving a config preamble on write, are
  both places where a mistake silently deletes text the user typed.
  Both need tests that assert the file survives each abnormal path.
- **Re-showing the menu after an emptied submit.** Proposed: return to the
  interrupt menu, matching how `Empty | Cancelled if menu` behaves today.
  `Ctrl+C` returns to the same place without touching the slot.
- **Countdown legibility.** A countdown row immediately after the assistant's
  final output, and immediately before a new turn's header, needs a look on a
  real terminal.
- **Interaction with [RFD 093].** Active work there reshapes query composition
  and the draft's role.
  The queued reply writes the same file 093 reads, so the two need to agree on
  preamble handling; this RFD assumes 093's rules that the buffer wins for query
  text and that the preamble is reconciled against its config seed when the
  draft is next opened, which requires the queued write to keep both.

## Implementation Plan

### Phase 1: the turn loop

Rename `run_turn_loop` to `run_turn_phases` and `turn_loop.rs` to
`turn_phases.rs`, freeing "turn loop" for the outer driver.
Droppable without affecting the rest of the phase.

Add `TurnEnd` to `run_turn_phases`'s return.
Add the turn loop in `Query::run`, driving exactly one turn: each iteration
builds its config epoch and collects its `TurnInputs`, and background tasks
drain at the turn boundary, syncing through the run's lock.
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
The argument-lifetime rules, which first matter once a run holds a second turn.
Sends immediately at turn end (no countdown yet).
Depends on phases 1–2.

### Phase 4: the countdown

`interrupt.loop_countdown` with all three keys, the countdown region, and the
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

### Phase 6: unsent drafts on direct sends

A query sent without composing no longer writes over an unsent draft: the prompt
on an interactive terminal, keep-the-draft without one, and no recovery copy
while an unsent draft exists.
Independent of every other phase, since it fixes a loss that exists today, so it
can land first.

## References

- [RFD 045] — the layered interrupt handler stack this builds on, including the
  escalation rule the countdown menu follows.
- [RFD 088] — the inline editor widget and the `compose_in_editor` spectrum the
  compose surface reuses.
- [RFD 093] — query composition and the `QUERY_MESSAGE.md` draft lifecycle this
  RFD writes into.
- [RFD 076] — tool access grants, the axis for narrowing what run-scoped
  approvals cover.
- [RFD 091]: the printer status region the countdown renders in.
- [RFD 104]: promptability, which decides whether the unsent-draft prompt and
  the send countdown are shown.
- [RFD 126]: turn summaries, one of the two background tasks the per-turn
  boundary drains.

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
