# Gate CI on new rustqual findings against the base commit

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Replace the committed-baseline gate in `just qual-ci` with `qual diff` from
T-0yh5frr.
CI analyses the tree it builds and its base in the same job, and reports only
the findings that are new.

Closes T-0yb2pgw: a baseline produced in the same run can't go stale, and it
always shares a base with the measured tree.

`qual` is temporary: T-1dhk659's follow-up folds it into a vendored rustqual.
Keep the recipe a thin call to the gate binary, so swapping `qual diff` for
rustqual's own command later touches only that call.

## Base revision

- `pull_request`: the checkout is `refs/pull/N/merge`, so the base is `HEAD^1`,
  the same parent the `changes` job already diffs against.
- `push` to `main`: the base is `github.event.before`.
  On a new branch, where `before` is all zeros, there's nothing to compare:
  analyse only, and fail only on enforced rules.
- The `qual` task's checkout needs enough history to reach the base.
  `fetch-depth: 2` covers pull requests; on push, fetch the `before` sha
  explicitly.

## Output

- `::error` annotations for new findings only, so GitHub's display cap no longer
  hides the regression behind existing findings.
- The full new-findings table, with guidance, in `$GITHUB_STEP_SUMMARY`.
- On success, one line: the head and base counts per pass.

## Remove

- `.config/rustqual/baseline.json`, `baseline-dry.json` and `regressions.jq`.
- The `qual-baseline` recipe.
- The canary in `qual-ci`.
  The fail-closed behaviour is covered by unit tests in the `qual` crate, which
  move with it into rustqual.
- Check the `justfile`, the configs and `docs/` for any other references to the
  baselines.
  Keep `just qual` as the exploratory entry point.

## Comments

-----

- **From**: jp
- **Date**: 2026-11-11T08:22:12Z

Two things differ from the description:

- **No-base case:** `qual report` never fails, so `qual diff --base <rev>` takes
  git's null object ID (all zeros, at full length) as "no base".
  It analyses the working tree only and fails only on enforced rules.
  The workflow passes `github.event.before` through unchanged, and the recipe
  stays one command with no branch in shell.
- **History on push:** fetching the `before` sha on top of a depth-1 checkout
  leaves `git merge-base HEAD <before>` without a common ancestor, so the `qual`
  task checks out the full history on push (`fetch-depth: 0`) and a depth of 2
  on pull requests.

A force-push to `main` whose old tip is no longer reachable makes `git
merge-base` fail, and `qual` exits 2: an error, not a pass.
