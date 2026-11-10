# Loop harness

`run.sh` runs `jp` unattended in a loop for a named mission.
The harness handles the loop mechanics shared across missions:

- wall-clock deadline (`--hours`, `--until`) and iteration limit;
- conversation rotation by commit count, event count, or consecutive failures;
- rate-limit detection, capacity probing, and waiting for quota recovery;
- network-stall detection and retry backoff;
- per-turn timeouts and interrupt handling;
- preflight checks (verifying unattended git commits, refusing to run on `main`,
  `master`, or detached `HEAD`, and rejecting a dirty staging index);
- post-turn git checks (commits created, dirty files, gate result);
- `RUNLOG.md`, with per-run log and snapshot directories.

<!-- end list -->

```sh
.config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
.config/loop/run.sh bug-hunt --hours 10                    # full run
```

The first argument names a directory under `missions/`.
Run `run.sh --help` to list all options.

The run exits 0 when `next-target` has no targets left, or when it reaches its
deadline or iteration limit after a turn that succeeded and passed the gate.
It exits 1 when it stops on a failed turn or gate, or aborts early, and 130 when
interrupted.

## Missions

A mission directory contains:

- **`mission.sh`** (required), sourced by the harness after setting defaults and
  before parsing command-line flags.
  It configures the variables below and defines the prompt functions.
- **`gate`** (optional, executable), run from the worktree root after every
  turn.
  Exit status 0 passes.
  Exit status 2 keeps the work and has the next turn repair it in place, even
  with `DISCARD_ON_GATE_FAILURE=1`; the repair continues the same iteration and
  target, up to `MAX_REPAIRS` times in a row.
  Any other non-zero status fails.
  Combined stdout and stderr is saved in the run directory as
  `iter-NNN.gate.log` and passed to `repair_prompt` on failure.
  If `gate` is omitted, the gate result is recorded as `skipped`.
- **`next-target`** (optional, executable), run from the worktree root before
  every non-repair turn.
  Its output is appended to the prompt.
  Empty output indicates that no targets remain and ends the run; a non-zero
  exit status aborts the run.
  If `next-target` is omitted, the assistant selects its own target.

### Settings

| Variable                  | Default | Meaning                                                                   |
| ------------------------- | ------- | ------------------------------------------------------------------------- |
| `MODEL`                   | none    | Model used for every turn and capacity probe (required).                  |
| `JP_CFG`                  | `()`    | `--cfg` flags passed to every turn (persona, skills).                     |
| `COMPACT`                 | empty   | `--compact` spec passed to every turn; omitted when empty.                |
| `ROTATE_COMMITS`          | `10`    | Start a new conversation after this many commits.                         |
| `ROTATE_EVENTS`           | `40`    | Start a new conversation after this many conversation events.             |
| `ROTATE_FAILURES`         | `2`     | Start a new conversation after this many consecutive failed turns.        |
| `ROTATE_ON_TARGET`        | `0`     | Set to `1` to start a new conversation whenever `next-target` changes.    |
| `SNAPSHOT_FILES`          | `()`    | Files under `state/` copied into the run's `snapshots/` before each turn. |
| `GATE_LABEL`              | `gate`  | Label for the gate result in the post-turn summary.                       |
| `DISCARD_ON_GATE_FAILURE` | `0`     | Set to `1` to reset the branch and working tree when the gate fails.      |
| `MAX_REPAIRS`             | `3`     | In-place repairs in a row before gate status 2 counts as a failure.       |

`mission.sh` may also override any other harness default (`HOURS`,
`TURN_TIMEOUT`, `MAX_FAILURES`, and so on).

### Prompt functions

`mission_prompt`, `handover_prompt`, `continue_prompt`, and `repair_prompt` are
required; each writes its section of the prompt to stdout.
`handover_prompt` and `continue_prompt` receive the previous iteration's git
summary as `$1`; `repair_prompt` receives the gate output as `$1`.
`target_prompt` is optional and receives the `next-target` output as `$1`.
`init_state` is optional and runs once per run after the state directory is
created, to initialize files such as `LEDGER.md`.
`conversation_args` is optional, receives the current target as `$1`, and prints
extra `jp query` arguments, one per line, for the first turn of each new
conversation, such as `--attach=<path>`.

The harness provides `fill`, which reads a template from stdin and replaces
`@RUN_ID@`, `@BASELINE@`, and `@STATE@` with the current run's values.

### Environment

`gate` and `next-target` receive `LOOP_MISSION`, `LOOP_MISSION_DIR`,
`LOOP_ROOT`, `LOOP_STATE`, `LOOP_RUN_ID`, `LOOP_RUN_DIR`, `LOOP_BASELINE`, and
the per-iteration variables `LOOP_ITERATION` and `LOOP_ITERATION_BASE` (the
`HEAD` commit at the start of the iteration; a repair turn keeps the base of the
iteration it repairs).
`gate` also receives `LOOP_TURN_STATUS`, the exit status of the turn it checks.

## Discarding an iteration

With `DISCARD_ON_GATE_FAILURE=1`, the harness discards any iteration that fails
the gate: it resets the branch and working tree to `LOOP_ITERATION_BASE` and
removes any untracked files created during that iteration.
The mission's `state/` directory and any untracked files that existed before the
iteration are preserved.

Before resetting, the harness records the discarded work, its commits plus any
uncommitted and untracked files, as a commit under
`refs/loop/discarded/<mission>/<run id>/<iteration>`, so a paid attempt is never
lost to garbage collection.
List them with `git for-each-ref refs/loop/discarded`, inspect one with `git
show <ref>`, and bring one back with `git cherry-pick <ref>`.
Delete them all with `git for-each-ref --format='delete %(refname)'
refs/loop/discarded | git update-ref --stdin`.

When `DISCARD_ON_GATE_FAILURE=1` is set, the harness refuses to start if the
working tree has unstaged changes, because a reset would overwrite them.
Only the harness resets the branch; the assistant is never instructed to rewrite
git history.

## State

All files written during a run live in the mission's `state/` directory:

```text
missions/<name>/state/
  LEDGER.md               # persistent state shared across runs
  runs/<run id>/
    RUNLOG.md             # per-iteration git summary
    iter-NNN.prompt       # prompt sent to the turn
    iter-NNN.log          # turn output
    iter-NNN.gate.log     # gate output
    probe-NNN.log         # capacity probe output
    snapshots/            # SNAPSHOT_FILES copied before each turn
```

Every `jp` invocation in a run uses a dedicated session,
`JP_SESSION=loop-<name>-<run id>`, so unattended turns never change the active
conversation of the terminal that started the run.

The run ID is the UTC start timestamp (`YYYYMMDDTHHMMSSZ`).
A run refuses to start if its directory already exists, so runs never overwrite
earlier logs.

The `state/` directory is not listed in `.gitignore`.
The harness excludes `state/` from git status checks (counting dirty files,
checking for unstaged changes, and resetting failed iterations).
Prompts instruct the assistant never to stage `state/`; a maintainer decides
whether to commit it.

## Why `.config/`

`.config/` holds repository development tooling outside the Cargo workspace,
alongside `jp-tools`, `rustqual`, and `supply-chain`.
Because `.ignore` whitelists `.config/`, JP's `fs_*` tools can read the harness
and mission files while keeping the scripts separate from `crates/` and `.jp/`.
