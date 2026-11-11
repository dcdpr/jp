# Add a rustqual burn-down mission to the loop harness

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

A second mission for the loop harness from T-0yh5hqj, which fixes rustqual
findings unattended, one at a time.
It depends on:

- T-0yh5frr, for `qual report` and `qual diff`;
- T-0yh5m6r, the rule policy, which must have landed so the loop never works on
  a rule JP exempts;
- T-0yh8q2s, the per-rule guidance files the prompt includes.

`qual report` and `qual diff` become rustqual commands once T-1dhk659's
follow-up folds `qual` into rustqual.
If this mission is built before then, call them through one place in the
mission's hooks, so the switch is a one-line change.

## One run, one rule

A run targets a single rule, passed to the mission as `--rule <ID>`, and works
on its own branch named after that rule (for example `qual/cx-004`).
Running tier by tier means running the harness once per rule, in the tier order
below.
This keeps every branch uniform: one rule, reviewed once.

## Before a run

A rule's guidance file is checked against that rule's findings when its run
starts, not for every rule up front (T-0yh8q2s wrote the files; nobody has read
most of them next to a real finding).

The mission refuses to start when:

- the rule is in `exempt` in `.config/rustqual/qual.toml`;
- `.config/rustqual/guidance/<RULE>.md` doesn't exist.

Whoever starts the run then does what the harness can't:

- Render five or so of the rule's findings with their guidance (`qual report
  --format ai`), spread over crates, and read each as the model will: does it
  say what to change without looking anything else up?
- Fix the guidance file where it doesn't, in a commit of its own before the
  run.
- Where rustqual's own message is what falls short, add the case to T-0yh5j3r.
- Where most of the sample is findings the guidance says to leave alone, don't
  run: the rule goes back to the policy for a decision.

After the run, the reasons in `rejected.jsonl` are the second check: a reason
that recurs becomes a line in the guidance file.

## `next-target`

Read `qual report` and print the next finding of the run's rule, one file at a
time.
Print nothing when none is left, which ends the run.
Skip findings already recorded as attempted or rejected.
Insert the rendered finding and its guidance file into the prompt, so the model
fixes exactly that finding and doesn't wander.

## `gate`

The harness measures all of this; it doesn't take the model's word for it:

- the target finding is gone;
- `qual diff --base <iteration start>` is empty;
- no change under `.config/rustqual/` and no new `qual:allow` marker;
- `cargo check` passes, and tests pass for the affected crate;
- one commit, with a subject that follows the repository's convention.

On failure the harness discards the iteration (mission setting on) and records
the finding as attempted.

## Rejections

The model may append `{key, reason}` to `rejected.jsonl` instead of fixing a
finding, when it judges the finding shouldn't be fixed.
A maintainer reviews these and turns each into a reasoned suppression or a
policy change.
The loop never acts on them itself.

## Tier order

| Tier | Rules                                                                                                       | Approx. count |
| ---- | ----------------------------------------------------------------------------------------------------------- | ------------- |
| 1    | dead code and types, wildcard imports, magic numbers, remaining boilerplate                                 | ~800          |
| 2    | function length, cognitive and cyclomatic complexity, nesting, duplicates and fragments, untested functions | ~800          |
| 2    | error handling (`unwrap` → `?`): reviewed as behaviour changes, because the failure mode changes            | 194           |

The boilerplate rules (`BP-*`, 256 of the tier 1 findings) are exempt in
`.config/rustqual/qual.toml` and out of the burn-down until T-1dhk659 brings
them back.
`TQ-003` (untested functions) waits on T-1ha99kp.

Out of scope: SRP struct and module splits, coupling, and anything tier 3.
Those are human-led and get their own tickets, one per file, once the burn-down
shows what remains.

## Output and ratchet

- Each run's branch holds commits for one rule.
  Open PRs from it grouped by crate, so a single review covers one rule in one
  crate.
- Once a rule reaches zero on `main`, add it to the `enforced` list
  (`.config/rustqual/qual.toml` today) in a small follow-up PR.
