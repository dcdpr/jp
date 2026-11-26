# Record the policy that JP never moves user data itself

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: relevant-rfd=052
- **Label**: type=task
- **Parent**: T-1n2tnwv

Write down the policy this group of tickets put in place, and settle what RFD
052 says about it.

The policy: JP never moves, rewrites, or deletes user data on its own.
It reports what it cannot load, and changes stored data only when the user names
the change (`jp conversation rm`, `jp doctor --fix <name>`).
The one exception is JP's own bookkeeping that holds no user data: leftover
`.tmp` files, import staging directories, and empty directories.

- [ ] A Reference wiki page that states the policy and its one exception, under
      the architecture pages, so a change that breaks it is checked against it
- [ ] Mark RFD 052's auto-trash design (moving conversations that fail
      validation to `.trash/` at startup) as superseded
- [ ] A backfilled RFD recording the decision: why startup stopped moving data,
      why the user trash was built and then removed, and why fixes live in `jp
      doctor`; it supersedes RFD 052's trash sections
