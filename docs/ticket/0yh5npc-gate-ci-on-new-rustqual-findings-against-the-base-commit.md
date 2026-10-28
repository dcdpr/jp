# Gate CI on new rustqual findings against the base commit

- **Status**: Todo
- **Kind**: Feature
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
  The fail-closed behaviour is covered by unit tests in the `qual` crate.
- Check the `justfile`, the configs and `docs/` for any other references to the
  baselines.
  Keep `just qual` as the exploratory entry point.
