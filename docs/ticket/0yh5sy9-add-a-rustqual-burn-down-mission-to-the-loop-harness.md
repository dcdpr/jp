# Add a rustqual burn-down mission to the loop harness

- **Status**: Todo
- **Kind**: Feature
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

## One run, one rule

A run targets a single rule, passed to the mission as `--rule <ID>`, and works
on its own branch named after that rule (for example `qual/cx-004`).
Running tier by tier means running the harness once per rule, in the tier order
below.
This keeps every branch uniform: one rule, reviewed once.

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

Out of scope: SRP struct and module splits, coupling, and anything tier 3.
Those are human-led and get their own tickets, one per file, once the burn-down
shows what remains.

## Output and ratchet

- Each run's branch holds commits for one rule.
  Open PRs from it grouped by crate, so a single review covers one rule in one
  crate.
- Once a rule reaches zero on `main`, add it to the `enforced` list in a small
  follow-up PR.
