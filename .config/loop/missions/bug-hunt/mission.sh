# Bug hunt: find, fix, test, and commit bugs, one per commit.
#
# Sourced by `.config/loop/run.sh`. Cross-run state is stored in
# `state/LEDGER.md`, and per-run logs and snapshots live under `state/runs/`.
#
#   .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
#   .config/loop/run.sh bug-hunt --hours 10                    # full run

MODEL=astra
ROTATE_COMMITS=10
ROTATE_EVENTS=40
COMPACT='r+t:..-3'       # strip reasoning + tool calls outside the last 2 turns
JP_CFG=(
    --cfg=personas/dev
    --cfg=skill/git-stage
    --cfg=skill/git-commit-writing
)
SNAPSHOT_FILES=(LEDGER.md)
GATE_LABEL='`cargo check --workspace --all-targets`'

# Keep failed iterations so the next turn can repair the build.
DISCARD_ON_GATE_FAILURE=0

init_state() {
    if [[ -f $STATE/LEDGER.md ]]; then
        return
    fi
    cat > "$STATE/LEDGER.md" <<'EOF'
# Bug hunt ledger

Durable memory for unattended bug hunts. Every conversation reads this file at
startup. It is the only state that persists across conversation rotations.

**Append only.** Never delete or rewrite an earlier entry. A later entry may
correct an earlier one, but state the correction explicitly rather than editing
the original entry.

Record three types of entries:

- **Swept**: a crate or module audited, and what the audit concluded, so later
  sessions do not re-audit the same code.
- **Fixed**: a bug fixed, including the commit subject and the regression test
  covering it.
- **Rejected**: a suspected bug that turned out not to be one, and why, so later
  sessions do not investigate it again.

---
EOF
}

# Prompts use quoted heredocs so backticks and `$` remain literal.
# `fill` substitutes @RUN_ID@, @BASELINE@, and @STATE@.

mission_prompt() {
    fill <<'EOF'
You are hunting bugs in this repository, unattended. Nobody is watching, and
nobody will answer questions. Work in small, complete, committed steps.

Read `@STATE@/LEDGER.md` first with `fs_read_file`. It records the work of
earlier sessions: what was swept, what was fixed, and what was examined and
ruled out. Do not repeat work already recorded there.
Then run `git_log` to see what has been committed since `@BASELINE@`.

Place your ledger entries under a heading for this run (`@RUN_ID@`) so later
sessions can see which run recorded each entry. Do not reuse entry numbers from
earlier runs.

Work this loop, one bug at a time:

1. **Pick one target.** Prefer an open bug ticket: run `ticket_list` with kind
   `bug` and status `todo`, and pick one you can reproduce, fix, and test in a
   single turn. When no ticket fits, audit a crate that has not been swept in
   the ledger yet, and state which crate you chose and why.
2. **Reproduce the bug with a failing test first.** A fix without a regression
   test is incomplete. If a test is genuinely impractical, explain why in the
   ledger.
3. **Fix the bug.** Run `cargo_check`, then `cargo_test` on the affected
   package, then `cargo_format`.
4. **Record the outcome** at the end of `@STATE@/LEDGER.md`: what you swept,
   what you fixed, or what you rejected and why.
5. **Stage only this fix** with the git staging tools, then commit it with
   `git_commit`.

Hard rules:

- One bug per commit. Never bundle multiple fixes into one commit.
- Never stage or commit anything under `@STATE@/`. The harness owns that
  directory, and a maintainer commits it separately.
- Never rewrite git history: no reset, no amend, no rebase, no force-push. Only
  add new commits.
- If `cargo check` fails on code you did not touch, an earlier session left the
  working tree broken. Repair the build first, commit the repair on its own, and
  record it in the ledger.
- If you find a bug you cannot fix safely, file a ticket with `ticket_create`
  and move on.
- Leave the workspace compiling at the end of every commit.

Start now. State which target you picked and why, then work it.
EOF
}

# Context appended to `mission_prompt` when rotating to a new conversation.
handover_prompt() {
    cat <<'EOF'

---

The previous conversation was rotated out. Status of the working tree after the
last turn (measured from git):

EOF
    printf '%s\n' "$1"
    cat <<'EOF'

Any uncommitted changes are unfinished work from the previous conversation.
Inspect the diff first, then either finish and commit those changes or revert
them before starting a new target.
EOF
}

continue_prompt() {
    cat <<'EOF'
Continue with the next bug.

Status after the last iteration (measured from git):

EOF
    printf '%s\n' "$1"
    cat <<'EOF'

Pick the next target and follow the same steps: failing test, fix, ledger entry,
narrow stage, one commit. If no open ticket or unswept item in the ledger fits,
audit a crate that has not been swept yet and name it.
EOF
}

# Pass only compiler error lines (or the last 30 lines if none match) to keep
# the prompt concise; full gate output is saved in `iter-NNN.gate.log`.
repair_prompt() {
    local errors
    # `grep` exits 1 when nothing matches; `|| true` prevents `set -e` from
    # aborting the script.
    errors=$(printf '%s\n' "$1" | grep -E 'error(\[E[0-9]+\])?:' | head -30 || true)
    if [[ -z $errors ]]; then
        errors=$(printf '%s\n' "$1" | tail -30)
    fi

    cat <<'EOF'
Stop. Repair the build before doing anything else.

`cargo check --workspace --all-targets` failed on the current working tree:

```
EOF
    printf '%s\n' "$errors"
    fill <<'EOF'
```

Fix this build failure first. Do not start new work, and do not commit anything
unrelated to the repair. Once the workspace compiles, commit the repair on its
own and record what was broken and how you fixed it in `@STATE@/LEDGER.md`.
EOF
}
