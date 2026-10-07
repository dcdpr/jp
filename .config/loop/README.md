# Loop harness

`run.sh` runs `jp` unattended against this worktree, one turn after another, on
behalf of a mission.
It owns everything that is not specific to the work:

- the wall-clock deadline (`--hours`, `--until`) and the iteration limit;
- conversation rotation by commits, events or consecutive failures;
- rate-limit detection, capacity probes and waiting for the window to reopen;
- network-stall detection and backoff;
- per-turn timeouts and interrupt handling;
- the commit probe, and refusing to run on `main` or with a staged index;
- facts measured from git after each turn: commits made, dirty files, gate
  result;
- `RUNLOG.md`, with per-run log and snapshot directories.

<!-- end list -->

```sh
.config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
.config/loop/run.sh bug-hunt --hours 10                    # the real thing
```

The first argument names a directory under `missions/`.
`--help` lists the options.

## Missions

A mission is a directory holding:

- **`mission.sh`** (required), sourced by the harness after its defaults and
  before the command line is parsed.
  It sets any of the settings below and defines the prompt functions.
- **`gate`** (optional, executable), run from the worktree root after every
  turn.
  Exit status 0 passes; anything else fails.
  Its combined output is kept in the run's directory as `iter-NNN.gate.log`
  and, on failure, handed to `repair_prompt`.
  Without one, the gate is reported as skipped.
- **`next-target`** (optional, executable), run from the worktree root before
  every turn except a repair.
  Its output is added to the prompt.
  Empty output means no targets remain and ends the run; a non-zero exit stops
  it.
  Without one, the assistant picks its own target.

### Settings

| Variable                  | Default | Meaning                                                 |
| ------------------------- | ------- | ------------------------------------------------------- |
| `MODEL`                   | none    | Model for every turn and capacity probe. Required.      |
| `JP_CFG`                  | `()`    | `--cfg` flags for every turn: persona, skills.          |
| `COMPACT`                 | empty   | `--compact` spec for every turn; empty passes none.     |
| `ROTATE_COMMITS`          | `10`    | New conversation after this many commits.               |
| `ROTATE_EVENTS`           | `40`    | ...or this many conversation events.                    |
| `ROTATE_FAILURES`         | `2`     | ...or this many consecutive failed turns.               |
| `SNAPSHOT_FILES`          | `()`    | State files snapshotted before each turn.               |
| `GATE_LABEL`              | `gate`  | How the gate's result is named in the facts.            |
| `DISCARD_ON_GATE_FAILURE` | `0`     | `1` resets the branch when an iteration fails the gate. |

Any other harness default (`HOURS`, `TURN_TIMEOUT`, `MAX_FAILURES`, ...) can be
overridden the same way.

### Prompt functions

`mission_prompt`, `handover_prompt`, `continue_prompt` and `repair_prompt` are
required; each prints its part of the prompt to stdout.
`handover_prompt` and `continue_prompt` receive the last iteration's facts as
`$1`; `repair_prompt` receives the gate's output.
`target_prompt`, receiving the `next-target` output, is optional.
`init_state`, optional, runs once per run after the state directory exists, to
seed files such as a ledger.

`fill` reads a template on stdin and replaces `@RUN_ID@`, `@BASELINE@` and
`@STATE@` with the run's values.

### Environment

`gate` and `next-target` see `LOOP_MISSION`, `LOOP_MISSION_DIR`, `LOOP_ROOT`,
`LOOP_STATE`, `LOOP_RUN_ID`, `LOOP_RUN_DIR`, `LOOP_BASELINE`, and, per
iteration, `LOOP_ITERATION` and `LOOP_ITERATION_BASE` (the commit the iteration
started from).

## Discarding an iteration

With `DISCARD_ON_GATE_FAILURE=1`, an iteration that fails the gate is undone:
the harness resets its own branch and the tree to `LOOP_ITERATION_BASE` and
deletes the files the iteration created.
The mission's `state/` directory is left as it is.
Untracked files that existed before the iteration are left alone.
Such a mission refuses to start with unstaged changes, since the reset would
take them too.
The assistant itself is never told to rewrite history; only the harness does.

## State

Everything a run writes lives in the mission's `state/` directory:

```text
missions/<name>/state/
  LEDGER.md               # whatever the mission keeps across runs
  runs/<run id>/
    RUNLOG.md             # facts measured from git, one entry per iteration
    iter-NNN.prompt       # the prompt each turn was sent
    iter-NNN.log          # the turn's output
    iter-NNN.gate.log     # the gate's output
    probe-NNN.log         # capacity probes
    snapshots/            # SNAPSHOT_FILES, copied before each turn
```

Every `jp` invocation in a run uses its own session,
`JP_SESSION=loop-<name>-<run id>`, so the run's conversations never become the
active conversation of the terminal it was started from.

The run id is the start time in UTC, to the second.
A run refuses to start if its directory already exists, so no run writes over
an earlier one.

The state directory is not ignored by git.
The harness leaves it out of everything it asks git about the work: the dirty
file count, the unstaged-changes check, and discarding an iteration.
The assistant is told never to stage it; committing it is up to a maintainer.

## Why `.config/`

`.config/` holds the repository's development tooling that is not part of the
Cargo workspace: the `jp-tools` binary, the rustqual and supply-chain config.
`.ignore` already exposes it to JP's `fs_*` tools, so an assistant can read the
harness and its missions, and it keeps scripts out of `crates/` and out of
`.jp/`, which JP reads as workspace configuration.
