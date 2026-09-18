# Collapse the duplicate getrandom versions in the dependency graph

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-09-14
- **Label**: domain=tooling
- **Label**: package=jp_conversation
- **Label**: type=task

`.clippy.toml` exempts `getrandom` from `allowed-duplicate-crates`, because the
graph carries both 0.2 and 0.3:

- `ring` depends on 0.2.
- `rand_core` and `jp_conversation` (for `EventId::random`) depend on 0.3.

Nothing in the workspace can collapse this today: `ring` reaches us transitively
and pins 0.2 itself.

Drop the exemption once the transitive dependencies converge on one version.
Checking whether they have is a matter of running `cargo tree --invert --package
getrandom` and seeing whether anything still pulls 0.2.

The exemption is cheap and correct, so this is bookkeeping rather than a
problem.
It is filed so the entry does not outlive the reason for it.
