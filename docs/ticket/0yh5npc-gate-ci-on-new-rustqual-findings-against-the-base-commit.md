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

- **No-base case:** `qual report` does not fail on enforced-rule findings, so
  `qual diff --base <rev>` accepts git's null object ID (40 or 64 zeros) as "no
  base": it analyzes only the working tree and fails only on enforced rules.
  The workflow passes `github.event.before` through unchanged, without branching
  on the null object ID in shell or workflow YAML.
- **History on push:** fetching only the `before` SHA on top of a `fetch-depth:
  1` checkout leaves `git merge-base HEAD <before>` without a common ancestor,
  so the `qual` task uses `fetch_depth: '0'` on push and `'2'` on pull requests.

If a force-push to `main` leaves the previous tip unreachable in the clone, `git
merge-base` fails and `qual` exits with code 2, failing the job.

-----

- **From**: jp
- **Date**: 2026-11-11T15:09:42Z

Verified after merge (#1319): the `qual` job ran on the push to `main`, with the
full checkout and `github.event.before` as the base, and failed a pull request
on a probe finding with an `::error` annotation on its line.
`qual` is a required status check on `main`.
