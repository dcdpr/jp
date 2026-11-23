# Record the policy that JP never moves user data itself

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-18
- **Label**: relevant-rfd=052
- **Label**: type=task
- **Parent**: T-1n2tnwv

Once the fixes land, write down the policy and decide whether to backfill an RFD
superseding RFD 052's auto-trash design.
At minimum, mark RFD 052's trash sections as superseded.

The policy: JP never moves, rewrites, or deletes user data on its own.
It reports what it cannot load, and changes stored data only when the user names
the change (`jp conversation rm`, `jp doctor --fix <name>`).
The one exception is JP's own bookkeeping that holds no user data: leftover
`.tmp` files, import staging directories, and empty directories.
