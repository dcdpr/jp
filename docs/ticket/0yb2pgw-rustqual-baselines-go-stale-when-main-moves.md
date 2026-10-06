# Rustqual baselines go stale when main moves

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=follow-up

`just qual-ci` compares the workspace against counts committed in
`.config/rustqual/baseline.json` and `baseline-dry.json`.
Those counts describe the tree at the moment they were recorded, so they go
stale as soon as `main` moves.

The first real run of the gate demonstrated it.
The baselines were recorded on 22 Sep; rebasing the branch onto 6 Oct `main` put
382 extra findings in the main pass and 116 in the DRY pass, across twelve
categories, none of them from the branch — its whole Rust diff was seven
comment lines.
The gate was right that the numbers had moved and had no way to say whose
numbers they were.

## Why it matters

The author who hits the failure is whoever rebases next, and the documented
response (`just qual-baseline`) makes them re-record a baseline covering two
weeks of someone else's code.
Refreshing is the only action that turns the job green, it always works, and it
takes one command — so the gate teaches contributors to reach for the thing
that silences it.
That is the failure mode the gate exists to prevent, arrived at from the other
direction.

The refresh is at least visible: the baselines are pretty-printed, so
`"total_findings": 2404 → 2786` shows up in the pull request diff where a
reviewer can see it.
That is a review-time catch, not a tooling one.

## Options

1. **Accept it.** Refresh on rebase and treat the failure as noise.
   Free, and normalizes the reflex.
2. **Re-record from `main` on a schedule.** A workflow runs `qual-baseline` on
   `main` and commits the result, so a branch compares against a baseline that
   is at most a day old.
   One extra workflow; the drift window shrinks but does not close, and the
   commits are noise in the log.
3. **Drop the committed baseline.** Analyse `HEAD` and the merge-base in the
   same job and compare the two reports.
   Staleness becomes impossible, because both numbers come from the run.
   Costs four analyses per job instead of two, and needs the merge-base tree
   checked out — probably a second worktree.

Option 3 removes the whole class of problem and is the one to aim for.
Worth measuring how long one `rustqual` pass takes over the workspace before
committing to it: if a pass is seconds, the extra cost is irrelevant and option
3 wins outright.

## Related

Introduced with the gate itself in `#1190`.
The same pull request already replaced rustqual's own `--compare
--fail-on-regression`, which scored a regression as the aggregate quality ratio
dropping and so missed new findings entirely; `.config/rustqual/regressions.jq`
now does a per-category comparison.
Whatever replaces the committed baseline should keep that filter — the
per-category check is orthogonal to where the numbers come from.
