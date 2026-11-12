# Add a rustqual burn-down mission to the loop harness

- **Status**: In Progress
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

Validate each rule's guidance file against a sample of real findings when
starting that rule's burn-down run (T-0yh8q2s drafted the files, and most have
not yet been reviewed alongside live findings).

The mission refuses to start when:

- the rule is in `exempt` in `.config/rustqual/qual.toml`;
- `.config/rustqual/guidance/<RULE>.md` doesn't exist.

Before launching the loop, perform a manual check:

- Render roughly five findings for the rule with their guidance (`qual report
  --format ai`) across different crates, and verify that each rendered finding
  gives enough information to make the fix without further lookup.
- Update the guidance file in a standalone commit before the run if any
  instructions are unclear or incomplete.
- If rustqual's own diagnostic message is missing needed context, record the
  issue in T-0yh5j3r.
- If most sampled findings are false positives that the guidance says to leave
  alone, abort the run and revisit the rule's policy in
  `.config/rustqual/qual.toml`.

After the run, review `rejected.jsonl` and fold any recurring rejection reasons
into the guidance file.

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

## Comments

-----

- **From**: jp
- **Date**: 2026-11-12T07:58:47Z

The mission is in `.config/loop/missions/rustqual/` (`mission.sh`,
`next-target`, `gate`, `lib.sh`).

Its first run, one iteration on `CX-004` with `astra`, passed the gate with one
commit.
The commit shortens `response_error` in
`crates/contrib/async-anthropic/src/client.rs`, 62 lines against a limit of 60,
by extracting a helper.
That run executed `preflight`, `next-target`, and the gate's passing path.
A failed gate (the discard, `attempted.jsonl`, the reason in the next turn's
prompt), a rejection, and the stop after 8 failed gates have not run yet.

What the ticket left open, and how it was settled:

- **`--rule`.** The harness had no way for a mission to take an option, so
  `run.sh` gained three optional mission functions: `parse_option`,
  `option_usage`, and `preflight` (documented in the loop README).
- **Exempt rules.** The hooks don't read `qual.toml`.
  `qual report` gained `--rule <ID>`, which fails on a rule the policy exempts,
  and `preflight` calls it.
  The policy's pattern matching and the `enforced`-over-`exempt` precedence stay
  in `qual`, and the mission keeps working when the policy moves into rustqual's
  config.
- **Branch.** `preflight` refuses to run unless `HEAD` is on `qual/<rule,
  lowercased>`.
  It doesn't create or switch branches.
- **A target** is every finding sharing one `(rule, file, symbol)` key, the
  identity `qual diff` compares by.
  "Gone" means `qual report` lists no finding with that key.
- **Tests for the affected crate** means the packages that own the changed
  files, not their dependents.
  `cargo check --workspace --all-targets` covers the dependents.

Where the mission departs from the description:

- **Rejections.** The model doesn't append to `rejected.jsonl`.
  It writes its reason as plain text to `state/rejection`, and the gate appends
  `{key, reason}`.
  The model can't then get the key wrong, break the JSON, or overwrite earlier
  lines.
  A rejection that also changed files is recorded, and the changes are
  discarded.
- **Markers.** The gate refuses `qual:api` and `qual:test_helper` as well as
  `qual:allow`, counted as added minus removed so a marker that moves with its
  item passes.
- **`.config/loop/`** is off limits along with `.config/rustqual/`, since the
  gate runs from the working tree.
- **Stopping early.** `next-target` ends the run with status 1 after 8 failed
  gates in a row.
  Every finding gets one attempt, so a mistake the model repeats would otherwise
  mark the whole rule as attempted.
- **Feedback.** The gate's reason for a discard goes into the next turn's
  prompt.
- **Commit subject.** Checked for `type(scope): Capitalized summary` without a
  closing period, in at most 72 characters.
  The convention says 50, which recent history on `main` doesn't hold to.

Still open on this ticket: the per-rule runs in tier order, the manual guidance
check before each (`cargo run --package qual -- report --rule <ID> --format
ai`), the PRs per crate, and the `enforced` ratchet.
