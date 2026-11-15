# Require the changelog checks on main

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-08
- **Label**: domain=tooling
- **Label**: relevant-rfd=121
- **Label**: type=task

The changelog gate and format validation ([RFD 121]) block nothing until GitHub
requires them, and the gate reads a label that has to exist first.
Someone with admin access to `dcdpr/jp` applies these once RFD 121's
implementation is on `main`:

- [ ] Create the label `no-changelog`.
- [ ] Make `Changelog fragment or no-changelog label` a required status check on
      `main`.
- [ ] Make the `changelog` task of the `rust` workflow a required status check
      on `main`, so a fragment that fails format validation can't merge.

Until then both checks report but block nothing.
If the gate reports a commit status by then (T-1fets19), the required check is
that status's context rather than the job.

[RFD 121]: ../rfd/121-changelog-fragments-and-validation.md
