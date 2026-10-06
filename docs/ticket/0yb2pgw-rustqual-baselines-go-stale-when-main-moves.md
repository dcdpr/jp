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

## Comments

-----

- **From**: jp
- **Date**: 2026-10-06T15:03:44Z

## Observed: the gate fires on commits the branch does not contain

First real CI run of `qual-ci` on `#1190` failed with `srp_module_warnings: 207
-> 208 (+1)` while the same recipe passed locally with identical numbers on both
sides.

Cause is narrower than "baselines go stale", and worse:

`.github/workflows/rust.yml:167` checks out with no `ref:`, so a `pull_request`
run gets `refs/pull/N/merge` — the head merged into *current* `main`.
The branch was rebased onto `150ba7868` and its baselines re-recorded there
(`e7e3b424c`).
`main` then took `dbafe5485` (#1242, 16:30) and `dfae02a91` (#1207, 16:33) the
same afternoon.
CI measured `branch + dfae02a91`; the baseline describes `branch + 150ba7868`.
The `+1` is a file crossing a threshold in that delta — #1242 adds net +21
lines to `crates/jp_cli/src/cmd/query/tool/prompter.rs`, which is otherwise
unflagged.

Two consequences beyond staleness:

1. **Not reproducible locally.** No sequence of local commands shows the
   failure, because the tree CI measured does not exist in any worktree.
   The author's only lever is to rebase and re-record — which is the reflex
   this ticket exists to prevent, now with no diagnostic alternative.
2. **The comparison is invalid in both directions.** A committed baseline
   describes exactly one tree.
   Compared against a merge tree, it invents regressions *and* can mask real
   ones: if the merge happens to delete findings elsewhere, a genuine new
   finding nets to zero and passes.

So a committed baseline and the merge ref cannot both be right.
The measured tree and the baseline have to share a base.
That makes option 3 (analyse `HEAD` and the merge-base in the same job, no
committed baseline) the correct target rather than merely the tidiest.

A cheaper interim exists: a per-task `checkout_ref` through `matrix.include`,
set to `github.event.pull_request.head.sha` for `qual` only, leaving `test` and
the other tasks on the merge ref where testing the merge result is the point.
Needs one thing verified first — whether `actions/checkout` treats an empty
`ref:` as unset, which is what the other matrix entries would pass.
