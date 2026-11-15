# Protect release branches and release tags on GitHub

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=tooling
- **Label**: relevant-rfd=122
- **Label**: type=task

`jp release` makes release branches and tags, and the person releasing pushes
them to `dcdpr/jp` ([RFD 122]).
They are the record of what each release shipped, and nothing stops a force push
or a deletion from rewriting that record.

Someone with admin access to the repository adds two rulesets once RFD 122's
`cut`, `pick`, and `finalize` have merged:

- [ ] On branches matching `release/*`, block force pushes and deletion.
- [ ] On tags matching `v*`, block updates and deletion.

Neither may require a pull request or restrict creation: release branches and
tags are pushed directly.

[RFD 122]: ../rfd/122-release-pipeline.md
