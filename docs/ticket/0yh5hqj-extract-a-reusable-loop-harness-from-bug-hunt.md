# Extract a reusable loop harness from bug-hunt

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-10-06
- **Label**: domain=tooling
- **Label**: type=task

Split the unattended bug-hunt script into a generic loop harness plus a
`bug-hunt` mission, with no change in behaviour.
The rustqual burn-down becomes a second mission on the same harness.

The script currently lives untracked at `bug-hunt/run.sh` in the `rustqual-next`
worktree.
Its header still refers to `.bug-hunt/`.

## Harness (shared)

Everything that already works and isn't specific to bugs:

- the loop, the wall-clock deadline (`--hours` / `--until`) and the iteration
  limit;
- conversation rotation by commits, events or consecutive failures;
- rate-limit detection, capacity probes and waiting;
- network-stall detection and backoff;
- per-turn timeouts and interrupt handling;
- the commit probe and the refusal to run on `main` or with a staged index;
- facts measured from git after each turn (commits made, dirty files, gate
  result);
- `RUNLOG.md`, with per-run log and snapshot directories.

## Mission (per directory)

- **Settings:** model, rotation limits, `--cfg` persona and skill flags,
  compaction spec.
- **Prompt templates:** mission, continue, handover, repair.
- **`next-target` executable**, optional.
  When a mission has one, the harness runs it before each turn and inserts its
  output into the prompt; empty output means no targets remain and ends the run.
  When a mission has none, the model picks its own target, as bug-hunt does
  today.
- **`gate` executable:** runs after each turn and reports pass or fail with
  output.
  For bug-hunt this is today's `cargo check --workspace --all-targets`.
- **Discard on gate failure**, opt-in per mission: the harness resets its own
  branch to the iteration's starting commit. bug-hunt keeps this off and keeps
  today's "next turn repairs" behaviour.

## Constraints

- The first commit adds `run.sh` unchanged, so the extraction reads as a diff
  against the original.
- Stays in bash.
  Rewriting it in Rust at the same stage invites the second-system effect.
- Commit only the harness and mission definitions.
  Ledgers, run logs, logs and snapshots stay gitignored.
- No behaviour change for bug-hunt.
  Verify with a `--iterations 1` smoke run before and after, and compare the run
  logs and prompts.
- Pick a location in the tree (for example under `.config/`) and record why.
