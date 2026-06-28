# Release Notes

This release focuses on performance and a few long-requested features. The
sections below summarize what changed and how to migrate, with a couple of
tables to make the numbers easy to scan at a glance.

## Benchmarks

The table below compares the previous release to the current one across a few
representative workloads. Numbers are median wall-clock times over ten runs.

| Workload         | Before (ms) | After (ms) | Change |
| ---------------- | ----------: | ---------: | :----: |
| cold start       |         420 |        180 |  -57%  |
| warm query       |          95 |         88 |   -7%  |
| large attachment |        1200 |        640 |  -47%  |

As the numbers show, cold start improved the most. Warm queries were already
fast, so the change there is comfortably within the run-to-run noise.

## Alignment

Markdown tables support per-column alignment, which we use above: the change
column is centered while the two timing columns are right-aligned so the digits
line up and longer-running workloads are easy to pick out.
