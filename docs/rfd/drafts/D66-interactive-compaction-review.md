# RFD D66: Interactive Compaction Review

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-02
- **Extends**: [RFD 064]
- **Summary**: Adds `jp conversation compact --interactive` to review and revise
  proposed compactions in a forked conversation before applying.

## Summary

This RFD adds an interactive mode to `jp conversation compact`.
The user reviews and revises a proposed compaction with the model in a separate
compaction conversation, then applies the result to the source conversation.
Every field of the compaction is negotiable: the turn range, the summary text,
and the mechanical policies.

## Motivation

[RFD 064] generates a compaction summary in a single non-interactive model call
and applies it immediately.
That one-shot flow falls short in two cases:

- Complex compactions need review and revision before they replace turns in the
  conversation's context.
- Conversations that exceed the context window must be compacted in subranges,
  building new summaries from prior compactions, which requires higher-level
  coordination than a single prompt provides.

Running compaction review as an ordinary conversation supports both human review
and subagent-driven compaction, instead of leaving summarization as a one-shot
operation that cannot handle conversations larger than the context window.

## Design

### Behavior

Interactive compaction follows a `git rebase` workflow: start the review,
discuss changes in ordinary chat, then `--continue` or `--abort`.

```sh
# 1. Start a review.
jp conversation compact --interactive

# 2. Discuss. Repeat as needed.
jp q

# 3. Apply the agreed compaction to the source conversation.
jp conversation compact --continue

# Or: stop without applying anything.
jp conversation compact --abort
```

**Step 1** (`--interactive`) forks the source conversation into a **compaction
conversation**, makes it active, runs the existing summarizer once to seed an
opening summary proposal, records that draft in the conversation, and prints
instructions for proceeding.
The opening proposal is always a summary compaction.
Its turn range comes from `compact`'s range flags (`--keep-first`,
`--keep-last`, `--from`, `--to`) if passed, otherwise from the first configured
rule with a `summary` section, or the built-in default range.
`--reasoning`, `--tools`, `--dry-run`, and `--reset` conflict with
`--interactive`, though the mechanical policies remain negotiable during the
review.
Starting a second review for a source conversation that already has one open
fails (see [One review per source](#one-review-per-source)).

The fork copies the entire source conversation, including the most recent turns,
so the model can inspect turns that a range change might add to or exclude from
the compaction.
Whether the fork retains the source's prior compactions depends on whether the
raw events fit in the context window (see [Raw events and prior
compactions](#raw-events-and-prior-compactions)).

**Step 2** is ordinary chat in the persisted compaction conversation.
It introduces no special interaction mode, prompt surface, or long-lived
process, so the user can pause the review and resume it later.

**Step 3** (`--continue`) reads the latest proposal from the compaction
conversation, prints it, and prompts for confirmation.
If confirmed, it appends the proposal to the source conversation as a
`Compaction` event, archives the compaction conversation ([RFD 071]), and
reactivates the source conversation.
If declined, nothing changes and the review stays open.

`--abort` ends the review without modifying the source conversation, archives
the compaction conversation, and reactivates the source.

`--continue` and `--abort` reject conversation IDs and range flags.
Both operate on the active conversation and fail if it is not a compaction
conversation; a user who switched away must run `jp conversation use` to return
to the review first.
Both commands archive the compaction conversation rather than deleting it,
preserving the review transcript unless the user deletes it explicitly.

### The compaction conversation

When `--interactive` forks the source conversation, it configures the new
conversation for review:

- **Turn markers.** Each copied turn has its 1-based source turn number
  prepended to the user message (for example, `[turn 17]`), matching JP's
  user-facing turn numbering ([Indexing and Counting Conventions]).
  Because markers are ordinary event content, both the model and `jp
  conversation print` see the same turn numbers without counting messages.
  Turns added during the review do not carry markers.

- **Tools.** A fork-time configuration delta disables all inherited tools and
  enables only `propose_compaction`.
  The source conversation is already in context, so the model needs no file
  access, cannot modify files during a review discussion, and avoids resending
  inherited tool definitions on every turn.

- **Instructions.** The fork-time configuration sets the review instructions,
  directing the model to discuss the compaction and record each agreed revision
  by calling `propose_compaction`.

- **Model.** The fork-time configuration sets the review model (see
  [Configuration](#configuration)).

- **Source ID.** The compaction conversation stores the source conversation's ID
  in a field on the conversation record (alongside `pinned_at` and
  `archived_at`, rather than in event metadata).
  This field identifies the conversation as a compaction conversation and
  persists across archiving, so an archived review still records which
  conversation it reviewed.

### One review per source

`--interactive` refuses to start when an unarchived compaction conversation
already references the source conversation:

```
error: a review of this conversation is already open: 01xyz
switch to it with `jp conversation use 01xyz`, then discuss, `--continue`, or `--abort`
```

Allowing two concurrent reviews would duplicate the source conversation twice
and let the second review to finalize append a competing compaction on top of
the first, leaving [RFD 064]'s overlap rules to resolve the conflict silently.
Archived reviews do not block new ones, because both `--continue` and `--abort`
archive the compaction conversation when finished.
Unarchiving an old review reopens it, blocking new reviews and source
compactions again; if the user runs `--continue` on it, re-validation and the
confirmation prompt guard against applying a stale proposal.

### Compaction of the source is blocked

An open review freezes the source conversation's compaction state, just as `git
rebase` blocks conflicting operations while in progress.
While an unarchived compaction conversation references the source, JP rejects
any command that appends or removes `Compaction` events on the source stream,
including `jp conversation compact` (and `--reset`) and `jp query --compact`:

```
error: a compaction review of this conversation is open: 01xyz
finish it first: `jp conversation use 01xyz`, then discuss, `--continue`, or `--abort`
```

Without this block, running `--reset` mid-review would invalidate the compaction
events copied into the fork, and adding a new summary compaction would trigger
[RFD 064]'s overlap auto-extension when the review is applied, silently widening
the negotiated range.

Only compaction writes on the source are blocked, using the same open-review
scan as `--interactive`.
`--dry-run` remains available because it writes nothing, `jp conversation fork
--compact` remains available because it compacts only the new fork, and ordinary
chat in the source remains available because the proposal's absolute turn range
tolerates new turns (see [Range bounds are
absolute](#range-bounds-are-absolute)).

### What is negotiated

A review negotiates a single **compaction proposal**, a candidate `Compaction`
event for the source conversation using the fields defined in [RFD 064]:

| Field        | Negotiable | Example change                                     |
| ------------ | ---------- | -------------------------------------------------- |
| `from_turn`  | yes        | "start at 4, turns 1 to 3 set up the whole task"   |
| `to_turn`    | yes        | "stop at 17, turn 18 opens a new topic"            |
| `summary`    | yes        | "keep the retry decision, drop the file listing"   |
| `reasoning`  | yes        | "do not strip reasoning, it carries the rationale" |
| `tool_calls` | yes        | "strip responses but keep the arguments"           |

Turn numbers refer to the **source** conversation, matching the injected turn
markers rather than the turns of the compaction conversation.

The fields follow [RFD 064]'s precedence rules: when `summary` is set,
`reasoning` and `tool_calls` are ignored.
Validation rejects a proposal that sets `summary` alongside either policy field,
preventing the user and model from negotiating policies that would have no
effect.

### Recording a proposal

The model records a proposal by calling a built-in tool (implemented inside JP
rather than as an external program):

```
propose_compaction(from_turn, to_turn, summary, reasoning, tool_calls)
```

`propose_compaction` is disabled by default and enabled only by the fork-time
configuration in a compaction conversation.

`from_turn` and `to_turn` are 1-based source turn positions matching the turn
markers.
Markers, tool arguments, validation errors, and rendered proposals all use
1-based numbering; JP converts them to 0-based `Compaction` indices only when
`--continue` constructs the event, matching the CLI range-flag boundary
([Indexing and Counting Conventions]).

Each tool call records a complete proposal as an ordinary tool-call event in the
compaction conversation, and `--continue` reads the latest call.
Using a tool call rather than message text has three advantages:

- Follow-up chat does not overwrite or obscure the latest proposal.
- JP renders each revision in the terminal through its existing tool-call UI.
- The turn range and summary are updated atomically in a single call, so they
  cannot drift out of sync.

JP validates each call against the current state of the source conversation: the
current conversation must reference a source ID, `from_turn` and `to_turn` must
fall within the source's turn count with `from_turn <= to_turn`, and the policy
values must parse.
Validation also applies [RFD 064]'s summary-overlap rule immediately: if a
summary range partially overlaps an existing summary compaction, JP expands the
range to subsume it so the tool result, confirmation prompt, and stored
`Compaction` event all carry the same bounds.
An invalid call returns a tool-result error ("the conversation has 20 turns,
`to_turn` was 25") that the model can correct in the same turn.
A valid call returns the formatted proposal, confirming the recorded fields for
both the user and the model.

During the review, validation reads a lock-free snapshot of the source
conversation.
A snapshot is sufficient because an open review blocks compaction writes on the
source and new turns only increase the valid turn count.
When `--continue` runs, it fails with a clear error if the compaction
conversation contains no `propose_compaction` call and re-validates the latest
call while holding the source conversation's lock.

### The opening proposal

`--interactive` records the initial summary draft by synthesizing an assistant
`propose_compaction` tool call and its matching tool result directly in the
compaction conversation's stream, using the seeded range and the summarizer's
output as arguments.
The review model sees this synthetic call in its conversation history and treats
it as its own prior call.
Writing the events directly keeps the opening state deterministic and preserves
the summarizer's output byte-for-byte; asking the model to emit the opening tool
call would cost an extra model turn and risk the model rewording the summary or
failing to call the tool.

### Range bounds are absolute

Users often request ranges relative to the end of the conversation, such as
"keep the last three turns".
`propose_compaction` resolves these requests against the source conversation's
turn count when the proposal is made and records absolute source turn numbers.
For a 20-turn conversation, "keep the last three turns" is recorded and
displayed as turns 1 to 17:

```
Proposed: compact turns 1-17 of 20 (summarize)
```

This matches [RFD 064], which resolves all range bounds to absolute turn indices
at creation time.
Because bounds are absolute, a proposal's range does not shift if the source
conversation grows while the review is open: if the source grows to 22 turns,
`--continue` still compacts turns 1 to 17 and leaves turns 18 to 22 uncompacted
for a later compaction.
Storing a relative bound would silently widen the range to cover turns that were
never reviewed.

### Applying and aborting

`--continue` resolves the source conversation from the active compaction
conversation's source ID field and locks both conversations.
Before modifying either conversation, it prints the latest proposal and prompts
for confirmation:

```
Proposed: compact turns 1-17 (summarize)
The conversation now has 24 turns; 7 turns stay uncompacted.

<summary text>

Apply? [y/n]
```

Printing the proposal and current turn count surfaces both the summary from
earlier in the discussion and any turns added to the source since the proposal
was recorded.
Declining leaves both conversations unchanged with the review still open.
The standard `--confirm`, `--no-confirm`, and `--yes` flags apply.

`--abort` does not prompt.
Nothing is written to the source conversation until `--continue` is confirmed,
and `--abort` archives rather than deletes the compaction conversation, so
aborting by mistake loses no work and leaves the source conversation untouched.

Archiving or deleting the source conversation while a review is open is allowed.
`--abort` always succeeds because it modifies only the compaction conversation.
`--continue` applies the compaction normally if the source was archived, and
fails with instructions to close the review with `--abort` if the source was
deleted.

`--continue` is idempotent.
It appends the `Compaction` to the source before archiving the compaction
conversation, so a failure between the two steps leaves the compaction applied
and the review still open.
Because an open review blocks all other compaction writes on the source (see
[Compaction of the source is blocked](#compaction-of-the-source-is-blocked)), a
rerun detects that the source's latest `Compaction` already matches the
proposal, skips the confirmation prompt and write, and finishes archiving the
review and reactivating the source.

### Raw events and prior compactions

At fork time, JP estimates whether the source conversation's raw events fit
within the review model's context window.
When they fit, the fork strips any copied `Compaction` events so the review
model sees every original turn rather than earlier summaries.
When the raw events exceed the context window, the fork retains the source's
prior `Compaction` events; the review model sees earlier compacted turns through
their stored summaries, and any new summary covering those turns is synthesized
from those summaries.

The initial summarizer run in `--interactive` follows the same rule, reading the
raw events in its target range when they fit the summary model's context window
and falling back to the projected view when they do not.

### Configuration

```toml
# Defaults to assistant.model.
[conversation.compaction.review.model]
id = "anthropic/claude-sonnet-4-5"
```

`model` is a table with the same schema as `assistant.model` and a compaction
rule's `summary.model`.
Because interactive review is a multi-turn discussion with tool calls, a
lightweight model configured for one-shot `summary.model` generation is a poor
fit, so `review.model` defaults to `assistant.model` instead.
The setting resolves against the configuration inherited from the source
conversation and is applied through the fork-time configuration delta; `--model`
overrides it.

## Drawbacks

- **Three commands instead of one.** Non-interactive `jp conversation compact`
  completes in a single invocation, whereas interactive review requires starting
  a compaction conversation, chatting, and running `--continue` or `--abort`.

- **Archive growth.** Every review leaves an archived conversation containing a
  full copy of the source conversation's events.
  Archiving hides finished reviews from default listings, but storage grows with
  each review unless the user deletes them manually, because the compaction
  conversation is the record of how the summary was produced.

- **Token cost.** Every turn in the compaction conversation resends the copied
  source conversation.
  Prompt caching absorbs most of this, but a multi-turn review of a large
  conversation remains expensive.

- **A new built-in tool.** JP has only one built-in tool today.
  Adding `propose_compaction` expands that surface, and the model can still
  forget to call it.

- **Synthetic opening tool call.** The review opens with a `propose_compaction`
  call injected by JP rather than emitted by the model.
  Models handle synthetic tool history reliably, but the transcript records a
  call the model did not make.

- **Self-review bias.** The model advises on a draft presented as its own prior
  output, which gives it an incentive to defend the initial summary.

## Alternatives

### Type the range, read the summary from the last message

Have `--continue --keep-last 3` take the turn range from CLI flags and read the
summary text from the model's final message.
Rejected because the final message in a free-form chat is usually an
acknowledgement rather than the summary, and requiring the user to re-enter the
range by hand provides no check that the flags match the turns the summary was
written for.

### Ask the model to restate the proposal at finalize time

Have `--continue` send a final structured-output prompt asking the model to emit
the agreed proposal, then print it for confirmation.
Rejected, narrowly: it avoids adding a new tool and the confirmation step
catches drift, but it costs an extra model call on every `--continue` and leaves
no distinct proposal event during the chat, so revisions cannot be rendered as
they happen.

### Review inside the source conversation

Hold the discussion in the source conversation and extend the compaction range
to cover the review turns so future prompts hide them.
Rejected on four counts: `compact --reset` would restore the raw events and
resurrect the review discussion; the extra turns would shift turn counts for
`--keep-last 3`, `--last N`, and [RFD 064]'s range arithmetic; a later summary
reading raw events would summarize the review discussion as task work; and
`conversation print` and `conversation grep` would surface the review transcript
permanently.

### An ephemeral review session inside one invocation

Run the review as an inquiry loop inside a single invocation against the
temporary stream `generate_summary` already builds.
Rejected because pausing and resuming a review would require a separate durable
draft artifact and rules for re-resolving relative ranges when the source
conversation grows.
A compaction conversation is durable using existing conversation storage.

### A conversation attachment scheme

Give the compaction conversation read access to the source through a new
`conversation:` attachment handler instead of copying the events.
Rejected because forking already places the source content in context.
The attachment scheme may be worth adding for other use cases, but interactive
compaction does not need it.

## Non-Goals

- **Reviewing mechanical compaction on its own.** Stripping reasoning and tool
  calls is deterministic and can already be previewed with `--dry-run`.
  Those policies are negotiable during a review because they are fields of the
  proposal, but `--interactive` is not meant for mechanical policies alone.

- **Multiple proposals per review.** One review produces a single `Compaction`.
  Multi-rule configurations are served by non-interactive `compact`.

- **Review in `jp query --compact`.** `jp query --compact` compacts
  non-interactively before sending a query; pausing for an interactive review
  conflicts with that workflow.

- **Automatic compaction review.** Automatic compaction (deferred by [RFD 064])
  runs without user intervention; pausing to ask the user is not automatic.

- **Editing the stored event stream.** Compaction remains a non-destructive
  overlay.

## Risks and Open Questions

- **Does the model reliably call `propose_compaction`?** The review instructions
  must prompt the model to record agreed revisions reliably without calling the
  tool after every exploratory message.
  This prompt will need tuning during implementation.

## Implementation Plan

### Phase 1: The proposal tool

Add the source conversation ID field to the conversation record and implement
`propose_compaction` as a built-in tool, disabled by default, with arguments
matching the `Compaction` fields defined in [RFD 064].
Validate every call against the source conversation referenced by the
conversation's source field: require a source ID on the current conversation,
verify that the 1-based `from_turn` and `to_turn` positions fall within the
source's turn bounds with `from_turn <= to_turn`, validate the policy values,
and apply summary-overlap expansion immediately so the recorded range is final.
Keep tool arguments 1-based; translation to 0-based `Compaction` indices happens
in Phase 3 when `--continue` builds the event.
Return either the validation error or the formatted proposal as the tool result.

This phase makes no CLI changes and leaves the tool disabled everywhere, so it
can merge independently.

### Phase 2: `--interactive`

Add `--interactive` to `jp conversation compact` and reject `--reasoning`,
`--tools`, `--dry-run`, and `--reset` alongside it.
Refuse to start when an unarchived compaction conversation already references
the source, and reuse that open-review scan to block non-interactive `jp
conversation compact` (including `--reset`) and `jp query --compact` on the
source while a review is open.

Fork the source conversation, prepend a 1-based turn marker to each copied turn,
and record the source ID in the field added in Phase 1.
Strip copied `Compaction` events when the source's raw events fit the review
model's context window; otherwise keep them and pass the projected view to the
initial summarizer run.
Write the fork-time configuration delta to disable inherited tools, enable
`propose_compaction`, and set the review instructions and review model.
Run the existing summarizer once over the range from the CLI flags, the first
configured rule with a `summary` section, or the built-in default; record its
draft as a synthetic opening `propose_compaction` call and result, activate the
compaction conversation, and print instructions.

Depends on Phase 1.

### Phase 3: `--continue` and `--abort`

Add `--continue` and `--abort` to `jp conversation compact`.
Neither flag accepts a conversation ID or range flags, and both require the
active conversation to be a compaction conversation.
`--continue` resolves the source conversation from the active compaction
conversation's source field, locks both conversations, reads and re-validates
the latest `propose_compaction` call, prints the proposal, and prompts for
confirmation (honoring `--confirm`, `--no-confirm`, and `--yes`).
If confirmed, it builds and appends the `Compaction` event, archives the
compaction conversation, and reactivates the source.
`--abort` archives the compaction conversation and reactivates the source
without modifying the source's event stream or prompting.

Append the `Compaction` before archiving the compaction conversation so an
archive failure never leaves the compaction unapplied.
If `--continue` is rerun after such a failure and finds the proposal already
recorded as the source's latest `Compaction`, skip the confirmation and write
and finish archiving and reactivating.
Allow `--continue` when the source is archived, and fail with a message pointing
to `--abort` when the source has been deleted.

Depends on Phase 2.

### Phase 4: Configuration

Add `conversation.compaction.review.model`, defaulting to `assistant.model` and
applied through the fork-time configuration delta, and make `--model` override
it the same way it overrides `summary.model` on compaction rules.

Depends on Phase 2 and can merge independently of Phase 3.

## References

- [RFD 064], Non-Destructive Conversation Compaction
- [RFD 071], Conversation Archiving

[Indexing and Counting Conventions]: ../../wiki/00dxyba-indexing-and-counting-conventions.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 071]: ../071-conversation-archiving.md
