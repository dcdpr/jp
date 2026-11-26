# Run `just rfd-review` through the review engine

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-24
- **Label**: domain=plugins
- **Label**: domain=tooling
- **Label**: relevant-rfd=136
- **Label**: type=feature

`just rfd-review NNN` runs one `jp query` under `personas/rfd-reviewer` and gets
back one long answer, sorted into four buckets.
`just rfd-triage NNN` attaches that answer to a second conversation, under
`personas/rfd-triager`, which gives each point a verdict.
Neither is tied to the text: a finding names its section in prose, and the
author reads two transcripts beside the RFD.

The review engine ([RFD 136]) and its browser UI ([RFD 140]) hold the same
exchange as comments on ranges of the file, each with a thread and a verdict.
This ticket moves the RFD review onto them:

1. The reviewing model files each finding as a comment on the lines it concerns.
2. The author reads them in the browser, and edits or withdraws the ones that
   are wrong.
3. Send delivers what is left to the authoring assistant, whose verdicts
   (accept, amend, decline, defer, ask) take the place of the triage step.

## What the engine lacks

- **A pass that writes for the reviewer.** Every pass is sent to the assistant,
  and what its turn writes is the assistant's: `review_comment` stores an open
  comment with `Role::Assistant` (`src/tool.rs`), which the author can answer
  and resolve, but not edit or withdraw.
  A reviewing model's findings have to land as the reviewer's drafts, which can
  be edited, withdrawn, and sent with the next pass.
  A pass has to record which side its turn writes for, and the tools have to
  read it.
- **What such a pass sends.** `Review::submit` sends every comment that waits on
  the assistant, and the drafts among them stop being drafts (`src/pass.rs`).
  A comment's part of the message is what the reviewer wrote since the assistant
  last did (`Comment::unanswered`).
  A reviewer's pass has to leave the author's drafts as they are, and from its
  second round on it is sent the threads the assistant answered, which is what
  the attached triage turn carries in the recipes.
- **A conversation per side.** A review runs every pass in one conversation:
  `Passes::conversation_for` refuses a pass that names another
  (`Refusal::Bound`).
  The reviewing model and the authoring assistant need one each, so that neither
  reads the other's reasoning, and each keeps its own model and persona.
- **A configuration per side.** `jp review submit --cfg` adds configurations to
  one pass, which is enough for a recipe.
  A pass started in the browser runs under `plugins.command.review.options.cfg`
  alone (`api::pass_request`), so the browser can only send to the author unless
  the options name a reviewer's configuration too.
- **Length.** What the tools write is capped at `max_chars`, 600 characters
  unless configured.
  A finding with its explanation and a proposed fix is longer.

## The persona

`personas/rfd-reviewer` asks for an answer under numbered headings.
A variant of it asks for one `review_comment` per finding, on the lines the
finding concerns, naming its bucket (architecture, interface, code shape,
other), and for a short closing summary.
`config/review.toml`, which tells an assistant to change the files, must not
apply to it: the reviewer reads and comments.

## The recipes

`just rfd-review NNN` becomes:

1. Find the open review titled `rfd-review:NNN`, or start one of the RFD's file
   (`jp review start`).
2. Run a reviewer's pass on it, with the review prompt as its message.
3. Open it in the browser (`jp review open`).

`just rfd-triage` is removed: sending the review to the authoring assistant is
the Send button, or `jp review submit`.
The Bear note lookups of both recipes are removed with them.
`_bear-note` itself stays, for `rfd/<id>/implement`.

## Steps

- [ ] Let a pass write for the reviewer: the pass records its side,
      `review_comment` under it stores a reviewer's draft, and `review.json` and
      RFD 136 describe it.
- [ ] Keep a reviewer's pass from sending the drafts that wait on the assistant,
      and send it the threads the assistant answered.
- [ ] Bind a review to one conversation per side.
- [ ] Let a reviewer's pass name its configuration, from `jp review submit` and
      from the browser.
- [ ] Let a reviewer's findings run longer than `max_chars` allows a reply.
- [ ] Add the persona that files its findings as comments.
- [ ] Rewrite `just rfd-review` on `jp review`, and remove `just rfd-triage`.

[RFD 136]: ../rfd/136-review-engine-for-workspace-files.md
[RFD 140]: ../rfd/140-browser-frontend-for-the-review-engine.md
