# RFD D46: Interactive Compaction Review

- **Status**: Draft
- **Category**: Design
- **Authors**: rgrant <rgrant@contract.design>
- **Date**: 2026-08-02
- **Extends**: [RFD 064]

## Summary

This RFD adds an interactive mode to `jp conversation compact`.
The user reviews and revises a proposed compaction by talking to the model in a
separate compaction conversation, then applies the result to the source
conversation.
Every field of the compaction is negotiable: the turn range, the summary text,
and the mechanical policies.

## Motivation

[RFD 064] generates a compaction summary in one non-interactive model
call and applies it immediately.  But:

  - some conversation compactions are intricate and need review; and

  - conversations that can't fit in context cannot be compacted except
    by operating over subranges, which means summarizing from prior
    compactions, and that needs higher-level tooling to do well.

Interactive review of compaction summaries in a flexible conversation
model allows both subagent use in compaction as well as human review.

Doing nothing leaves summarization as a one-shot bet that cannot
operate over long converations.

## Design

### Behavior

Three commands.

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

The shape follows `git rebase`: start, then discuss, then `--continue` or
`--abort`.

**Step 1** forks the source conversation into a **compaction conversation**,
makes it active, runs the existing summarizer once so the review opens on a real
draft rather than a blank page, records that draft as the opening proposal, and
prints how to proceed.
The opening proposal is always a summary proposal.
Its range comes from the range flags of `compact` (`--keep-first`,
`--keep-last`, `--from`, `--to`) when given; otherwise from the first
configured rule with a `summary` section; otherwise from the built-in default
range.
`--reasoning`, `--tools`, `--dry-run`, and `--reset` conflict with
`--interactive`; the policies stay negotiable inside the review.
Starting a second review for a source that already has one open is refused (see
[One review per source](#one-review-per-source)).

The fork copies the whole source conversation, including the most recent turns.
The range is under negotiation, so the model has to see the turns that a range
change would move across.
Whether the source's earlier compactions come along depends on size (see
[Raw events and prior compactions](#raw-events-and-prior-compactions)).

**Step 2** is ordinary chat.
No new interaction mode, no prompt surface, no long-lived process.
The user can leave and come back; the compaction conversation is durable.

**Step 3** reads the most recent proposal from the compaction conversation,
prints it, and asks for confirmation.
On yes, it applies the proposal to the source conversation as a `Compaction`
event, archives the compaction conversation ([RFD 071]), and switches the
active conversation back to the source.
On no, nothing changes and the review stays open.

`--abort` ends the review without applying anything.
It archives the compaction conversation and switches back to the source.

`--continue` and `--abort` take no conversation ID and no range flags.
They operate on the active conversation, which must be a compaction
conversation; anything else fails with a clear error, so a user who switched
away switches back first with `jp conversation use`.

Both endings archive rather than delete, so the discussion that produced (or
failed to produce) a compaction stays available.
The user can delete a compaction conversation outright if they want it gone.

### The compaction conversation

The fork is not a plain copy.
Step 1 sets the new conversation up for its one job:

- **Turn markers.**
  Each copied turn gets a marker carrying its source turn number, prepended to
  the turn's user message: `[turn 17]`.
  The markers are ordinary event content, so the model sees them in its context
  and the user sees them in `jp conversation print`.
  Both sides read the same numbers instead of counting messages.
  Marker numbers are 1-based, like every user-facing turn position
  ([Indexing and Counting Conventions]).
  Turns added during the review carry no markers; they are not part of the
  negotiation.

- **Tools.**
  A configuration entry written at fork time disables every inherited tool and
  enables exactly one: `propose_compaction`.
  The source's content is already fully in context, so the review needs no file
  access, and a model with edit access during what the user treats as a
  read-only discussion is a surprise nobody wants.
  Dropping the inherited tool descriptions also stops them being resent on
  every review turn.

- **Instructions.**
  The same configuration entry sets the review instructions: the model's job is
  to discuss the compaction and record each agreed revision by calling
  `propose_compaction`.

- **Model.**
  The same configuration entry sets the review model (see
  [Configuration](#configuration)).

- **Source ID.**
  The compaction conversation records the source conversation's ID in a new
  field on the conversation record — a sibling of `pinned_at` and
  `archived_at`, not event metadata.
  Its presence is what marks a conversation as a compaction conversation, and
  it survives archiving, so an archived review still names what it reviewed.

### One review per source

`--interactive` refuses to start when a not-yet-archived compaction
conversation already names the source, and points at it:

```
error: a review of this conversation is already open: 01xyz
switch to it with `jp conversation use 01xyz`, then discuss, `--continue`, or `--abort`
```

Two open reviews would each carry a full copy of the source, and whichever
finalized second would append a second compaction on top of the first, with
[RFD 064]'s overlap rules quietly deciding the outcome.
There is no use case for that.
Archived reviews never block: both endings archive, so a finished review clears
the way, and a forgotten one is exactly what the error reminds the user about.
Unarchiving an old review reopens it: new reviews and compaction of the source
are blocked again, and `--continue` offers its stale proposal.
Re-validation and the confirmation prompt make that safe, if rarely useful.

### Compaction of the source is blocked

An open review freezes the source's compaction state, the way git blocks
conflicting operations mid-rebase.
While a not-yet-archived compaction conversation names the source, JP refuses
every operation that appends or removes `Compaction` events on the source's
stream: `jp conversation compact` in all forms, including `--reset`, and
`jp query --compact` targeting the source.

```
error: a compaction review of this conversation is open: 01xyz
finish it first: `jp conversation use 01xyz`, then discuss, `--continue`, or `--abort`
```

Without the block, a `--reset` mid-review changes what the copied compaction
events in the fork mean, and a new summary compaction triggers [RFD 064]'s
overlap auto-extension at apply time, silently widening the range the review
negotiated.

The block is scoped to compaction writes.
`--dry-run` stays available; it writes nothing.
Ordinary chat in the source stays available; the proposal's absolute range
already tolerates growth (see
[Range bounds are absolute](#range-bounds-are-absolute)).
`jp conversation fork --compact` stays available; it compacts the fork's copy
and leaves the source's stream untouched.
The check is the same open-review scan `--interactive` performs.

### What is negotiated

The unit under discussion is a **compaction proposal**: one candidate
`Compaction` for the source conversation.
Its fields are those [RFD 064] already defines.

| Field        | Negotiable | Example change                                     |
| ------------ | ---------- | -------------------------------------------------- |
| `from_turn`  | yes        | "start at 4, turns 1 to 3 set up the whole task"   |
| `to_turn`    | yes        | "stop at 17, turn 18 opens a new topic"            |
| `summary`    | yes        | "keep the retry decision, drop the file listing"   |
| `reasoning`  | yes        | "do not strip reasoning, it carries the rationale" |
| `tool_calls` | yes        | "strip responses but keep the arguments"           |

Turn numbers refer to the **source** conversation, read from the turn markers.
The compaction conversation is scratch space, not the subject.

The fields follow [RFD 064]'s precedence: when `summary` is set, `reasoning`
and `tool_calls` are ignored.
Validation flags a proposal that sets `summary` alongside either, so the model
drops the dead fields rather than letting the user negotiate policies that
change nothing.

### Recording a proposal

The model records a proposal by calling a built-in tool.

```
propose_compaction(from_turn, to_turn, summary, reasoning, tool_calls)
```

Built-in means implemented inside JP rather than as an external program.
The tool is disabled by default; the configuration entry written at fork time
enables it inside compaction conversations, and nowhere else.
Outside a review, the model never sees it.

`from_turn` and `to_turn` are 1-based source turn positions — the numbers the
model reads from the turn markers.
The model never sees a 0-based index: markers, tool arguments, validation
errors, and the rendered proposal all speak 1-based.
JP translates to the 0-based `Compaction` fields only when `--continue` builds
the event, at the same boundary where CLI range flags are translated
([Indexing and Counting Conventions]).

Each call is one proposal.
Calls accumulate as ordinary tool-call events in the compaction conversation, so
the revision history is the conversation history.
`--continue` reads the most recent call.

Three properties follow from using a tool call rather than message text:

- Chat after a proposal does not invalidate it.
  The proposal is a distinct event, not a position in the message list.
- JP already renders tool calls, so each revision is visible on screen as it is
  made.
- The range and the summary are recorded by the same action, so they cannot
  describe different things.

JP validates every call as it is made, against the source conversation as it
stands: the range falls within the source's turns, `from_turn` does not exceed
`to_turn`, and the policy values parse.
Validation also applies [RFD 064]'s summary-overlap rule at once: a summary
range that partially overlaps an existing summary compaction is grown to
subsume it, and the grown range is the proposal's range from that moment on.
The tool result, the confirmation prompt, and the stored event all carry the
same numbers; nothing widens at apply time.
An invalid call returns an error as the tool result ("the conversation has 20
turns, `to_turn` was 25"), which the model reads and corrects in the same
exchange, while the user watches.
A valid call returns the proposal rendered for display, so the user and the
model see the same confirmed reading of what was just agreed.
A call made in a conversation that records no source is an error.

Validation reads the source without taking its lock.
A snapshot suffices: an open review admits no compaction writes, new turns
only widen the valid range, and `--continue` re-validates under the source's
lock before applying.

`--continue` fails with a clear error when the compaction conversation contains
no `propose_compaction` call, and re-validates the call it finds before
applying it.

### The opening proposal

Step 1 records the summarizer's draft as the first proposal by writing the
tool-call events itself: an assistant `propose_compaction` call whose arguments
are the summarizer's output and the seeded range, plus the matching result.
The model never made this call; it finds it in its history and treats it as its
own, which models handle fine in practice.
Writing the events directly keeps the opening state deterministic and the
recorded arguments byte-for-byte the summarizer's output.
Asking the model to record the draft itself would cost one more model call and
lose the guarantee: the model can reword the summary while copying it, or fail
to call the tool at all.

### Range bounds are absolute

A range agreed in conversation is usually expressed relative to the end of the
conversation: "keep the last three turns".
`propose_compaction` records it as absolute source-conversation turn numbers,
resolved against the conversation as it stood when the proposal was made.
"Keep the last three of twenty" is recorded as turns 1 to 17, and displayed that
way:

```
Proposed: compact turns 1-17 of 20 (summarize)
```

This matches [RFD 064], which already resolves all range bounds to absolute turn
indices at creation time.

The consequence is that a proposal does not move.
If the source conversation grows to twenty-two turns while the review is open,
`--continue` still compacts turns 1 to 17.
Turns 18 to 22 stay uncompacted and remain available to a later compaction.
A relative bound would silently widen the range to cover turns the review never
looked at.

### Applying and aborting

`--continue` resolves the source from the active compaction conversation's
source field, so the user passes no IDs.
It reads the compaction conversation and writes to the source, so it locks
both.

Before touching anything, it prints the proposal it found and asks:

```
Proposed: compact turns 1-17 (summarize)
The conversation now has 24 turns; 7 turns stay uncompacted.

<summary text>

Apply? [y/n]
```

The most recent proposal can sit several messages back in a discussion the user
last touched yesterday, and the source may have grown since it was recorded;
the confirmation shows both.
Declining leaves everything as it was, review still open.
The standard `--confirm` / `--no-confirm` / `--yes` flags apply.

`--abort` does not prompt.
It writes nothing to the source and archives rather than deletes, so an
accidental abort loses nothing that matters.

Deleting or archiving the source while a review is open is not blocked.
`--abort` always works; it touches only the compaction conversation.
`--continue` against an archived source applies normally.
`--continue` against a deleted source fails: the source conversation no longer
exists, and the error says to close the review with `--abort`.

Nothing is written to the source conversation until a confirmed `--continue`.
Abandoning a review costs the compaction conversation and the tokens spent in
it, and leaves the source conversation untouched.

`--continue` is idempotent.
Apply comes before archive, so a failure between the two leaves the compaction
applied and the review still open.
A rerun detects that state — the source's most recent `Compaction` equals the
proposal, and an open review means nothing else can have written it (see
[Compaction of the source is blocked](#compaction-of-the-source-is-blocked))
— and skips the prompt and the apply, finishing only the archive and the
switch back.

### Raw events and prior compactions

Raw events are preferred, and size decides.
At fork time, JP estimates whether the source's raw events fit the review
model's context window.
When they fit, the fork drops the copied compaction events: the review model
sees every original turn, and earlier summaries play no part in the review.
When they do not fit, the fork keeps them: the review model sees earlier turns
as their stored summaries, and a summary negotiated over such turns is built
from those summaries, because the original messages cannot be loaded.

The step 1 summarizer follows the same rule: it reads the raw events of its
range when they fit the summary model's window, and the projected view when
they do not.
A source with no prior compactions has nothing to drop or keep; both readings
are the same.

### Configuration

```toml
# Defaults to assistant.model.
[conversation.compaction.review.model]
id = "anthropic/claude-sonnet-4-5"
```

`model` is a table mirroring `assistant.model`, the shape the rules' `summary.model`
already uses.

Review is a multi-turn discussion with tool calls.
The cheap one-shot model a rule may name under `summary.model` is a poor fit, so
the review model defaults to `assistant.model` rather than to the rule's summary
model.
The setting resolves against the compaction conversation's own configuration,
which the fork carries over from the source, and is applied through the
fork-time configuration entry; `--model` overrides it.

## Drawbacks

- **Three commands instead of one.**
  Non-interactive `jp conversation compact` remains a single invocation.
  Review costs a compaction conversation, a chat, and a finalize.

- **Archive growth.**
  Every review leaves an archived conversation carrying a full copy of the
  source conversation's events.
  Archiving keeps them out of the default listing, but the storage cost is real
  and grows with each review.
  Deleting is the user's call, because the compaction conversation is the record
  of why the summary says what it says.

- **Token cost.**
  The compaction conversation carries the whole source conversation, and every
  review turn resends it.
  Prompt caching absorbs most of this, but a long review of a long conversation
  is not cheap.

- **A new built-in tool.**
  JP has one today.
  Adding a second sets a precedent, and the model can forget to call it.

- **The opening proposal is fabricated.**
  The review opens with a tool call the model never made.
  Models handle found history fine in practice, but the compaction conversation
  is not a faithful transcript of what the model did.

- **The model advises on its own summary.**
  It has an incentive to defend the draft it wrote.
  The user decides, but the advice is not neutral.

## Alternatives

### Type the range, read the summary from the last message

`--continue --keep-last 3` takes the range from flags and the summary from the
model's final message.
Rejected: after free chat the last message is usually acknowledgement, not the
summary.
The user also transcribes a range by hand, and nothing checks that it matches
the range the summary was written for.

### Ask the model to restate the proposal at finalize time

`--continue` sends one more turn asking for the proposal in a fixed shape, using
the structured output JP already has, and prints it for confirmation.
Rejected, narrowly.
It needs no new tool and the confirmation step catches drift, but it spends a
model call on every finalize, and no proposal exists as a distinct artifact
during the chat, so revisions cannot be shown as they happen.

### Review inside the source conversation

Hold the discussion in the source conversation, then extend the compaction range
to cover the review turns so the model never sees them.
Rejected on four counts.
`compact --reset` restores the raw events and resurrects the discussion.
Turn counts shift, silently changing what `--keep-last 3`, `--last N`, and
[RFD 064]'s range arithmetic mean.
A later summary reads raw events and would summarize the meta-discussion as
work.
`conversation print` and `conversation grep` surface it forever.

### An ephemeral review session inside one invocation

Run the review as an inquiry loop against the throwaway stream
`generate_summary` already builds.
Rejected: it needs a durable draft artifact to survive the process, and with it
a rule for what happens when the conversation grows and the range resolves
differently.
A compaction conversation is durable for free.

### A conversation attachment scheme

Give the compaction conversation read access to the source through a new
`conversation:` attachment handler.
Rejected: forking already copies the content.
The attachment scheme may be worth having for other reasons, but this feature
does not need it.

## Non-Goals

- **Reviewing mechanical compaction on its own.**
  Stripping reasoning and tool calls is deterministic and already previewable
  with `--dry-run`.
  The policies are negotiable inside a review because they are fields of the
  proposal, but a review is not worth starting for them alone.

- **Multiple proposals per review.**
  One review produces one `Compaction`.
  Multi-rule configurations are served by non-interactive `compact`.

- **Review in `jp query --compact`.**
  That path compacts silently before querying.
  Interactive review there is a mode collision.

- **Automatic compaction review.**
  Whatever [RFD 064]'s follow-up on automatic compaction settles, an automatic
  trigger that stops to ask is not automatic.

- **Editing the stored event stream.**
  Compaction remains an overlay.

## Risks and Open Questions

- **Does the model reliably call `propose_compaction`?**
  The review instructions have to make recording a revision habitual without
  the model spamming a call after every message.
  Needs tuning during implementation.

## Implementation Plan

### Phase 1: The proposal tool

Add the source field on the conversation record, and `propose_compaction` as a
built-in tool, disabled by default.
Its arguments mirror the `Compaction` fields [RFD 064] defines.
Every call is validated against the source conversation named by the
conversation's source field: range within bounds, `from_turn` at most `to_turn`,
policy values recognized, and the summary-overlap growth applied at once, so
the recorded range is final.
Arguments are 1-based positions, validated as such; the translation to the
stored 0-based indices happens when `--continue` builds the `Compaction`
(Phase 3), not in the tool.
The tool result carries the validation error or the proposal rendered for
display; a conversation that names no source is itself a validation error.

No command changes, and nothing enables the tool yet.
Mergeable independently.

### Phase 2: `--interactive`

Add the flag to `jp conversation compact`.
Refuse when an open review already names the source.
Fork the source conversation, inserting a turn marker at the start of each
copied turn and recording the source ID in the field added in Phase 1.
Drop the copied compaction events when the source's raw events fit the review
model's context window; keep them when they do not, and feed the step 1
summarizer the projected view in that case.
Reject `--reasoning`, `--tools`, `--dry-run`, and `--reset` alongside
`--interactive`.
Write the fork-time configuration entry: inherited tools off,
`propose_compaction` on, review instructions, review model.
Run the existing summarizer once, its range from the flags when given,
otherwise the first configured rule with a `summary` section, otherwise the
built-in default; write its draft as the opening proposal, activate the
compaction conversation, and print instructions.
Block compaction writes to the source while the review is open: non-interactive
`jp conversation compact` (including `--reset`) and `jp query --compact` refuse
when an open review names the conversation, reusing the open-review scan.

Depends on Phase 1.

### Phase 3: `--continue` and `--abort`

Add both flags.
Neither accepts a conversation ID or range flags; both require the active
conversation to be a compaction conversation.
`--continue` resolves the source conversation from the active compaction
conversation's source field, locks both, reads the most recent `propose_compaction`
call, re-validates it, prints it, and asks for confirmation (the shared
`--confirm` / `--no-confirm` / `--yes` flags apply).
On yes it builds and appends the `Compaction`, archives the compaction
conversation, and reactivates the source.
`--abort` archives the compaction conversation and reactivates the source
without touching the source's event stream, and without prompting.

Archiving is the last step of `--continue`: a failure to archive must not leave
the compaction unapplied.
A rerun after such a failure finds the proposal already applied as the source's
most recent `Compaction`, skips the confirmation and the apply, and completes
the archive and reactivation.
A deleted source makes `--continue` fail with a pointer to `--abort`; an
archived source is applied to normally.

Depends on Phase 2.

### Phase 4: Configuration

Add `conversation.compaction.review.model`, defaulting to `assistant.model`,
applied through the fork-time configuration entry, and make `--model` override
it the way it overrides rule summary models today.

Depends on Phase 2.
Mergeable separately from Phase 3.

## References

- [RFD 064], Non-Destructive Conversation Compaction
- [RFD 071], Conversation Archiving

[Indexing and Counting Conventions]: ../../architecture/indexing-conventions.md
[RFD 064]: ../064-non-destructive-conversation-compaction.md
[RFD 071]: ../071-conversation-archiving.md
