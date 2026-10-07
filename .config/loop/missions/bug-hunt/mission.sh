# Bug hunt: find, fix, test and commit bugs, one per commit.
#
# Sourced by .config/loop/run.sh. The assistant's durable memory is
# `state/LEDGER.md`, shared by every run; each run's own record is under
# `state/runs/`.
#
#   .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
#   .config/loop/run.sh bug-hunt --hours 10                    # the real thing

# ── settings ────────────────────────────────────────────────────────────────

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

# A failed gate is handed to the next turn to repair, not thrown away.
DISCARD_ON_GATE_FAILURE=0

# ── state ───────────────────────────────────────────────────────────────────

init_state() {
    if [[ -f $STATE/LEDGER.md ]]; then
        return
    fi
    cat > "$STATE/LEDGER.md" <<'EOF'
# Bug hunt ledger

Durable memory for unattended bug hunts. Every conversation reads this file
once, at its start. It is the only thing that survives a conversation rotation.

**Append only.** Never delete or rewrite an earlier entry. A later entry may
correct an earlier one, but say so explicitly rather than editing the original.

Record three kinds of thing:

- **Swept** — a crate or module audited, and what the audit concluded. This is
  what stops the next session reading `jp_config` for the third time.
- **Fixed** — a bug fixed, with the commit subject and the test that now covers
  it.
- **Rejected** — something that looked like a bug and was not, with the reason.
  As valuable as a fix: it stops the next session chasing it.

---
EOF
}

# ── prompts ─────────────────────────────────────────────────────────────────
#
# Quoted heredocs throughout, so backticks and `$` in the prose stay literal.
# `fill` substitutes @RUN_ID@, @BASELINE@ and @STATE@; anything else dynamic is
# printed around the heredocs.

mission_prompt() {
    fill <<'EOF'
You are hunting bugs in this repository, unattended. Nobody is watching, and
nobody will answer a question. Work in small, complete, committed steps.

Read `@STATE@/LEDGER.md` now, before anything else, with `fs_read_file`. It
is the memory of every session that ran before this one: what was swept, what
was fixed, what was examined and ruled out. Do not repeat work it already
records.
Then run `git_log` to see what has been committed since `@BASELINE@`.

Your ledger entries belong under a heading naming this run, `@RUN_ID@`,
so a later reader can tell which session concluded what. Earlier runs
used their own numbering; continue past it rather than reusing it.

Then work this loop, one bug at a time:

1. **Pick one target.** Prefer an open bug ticket: run `ticket_list` with kind
   `bug` and status `todo`, and take one you can reproduce, fix, and test in a
   single sitting. When no ticket fits, audit a crate the ledger has not swept
   yet, and say which one you chose and why.
2. **Reproduce it with a failing test first.** A fix without a test that would
   have caught the bug is not finished. If a test is genuinely impractical,
   write down why in the ledger.
3. **Fix it.** Run `cargo_check`, then `cargo_test` on the affected package,
   then `cargo_format`.
4. **Write it down** in `@STATE@/LEDGER.md`, appended to the end of the file:
   what you swept, what you fixed, what you rejected and why.
5. **Stage exactly this one fix** with the git staging tools, then commit it
   with `git_commit`.

Hard rules:

- One bug per commit. Never bundle two fixes into one commit.
- Never stage or commit anything under `@STATE@/`. It is the harness's
  record, and a maintainer commits it, not you.
- Never rewrite history: no reset, no amend, no rebase, no force. Only add
  commits.
- If `cargo check` fails on code you did not touch, an earlier session left the
  tree broken. Repair that first, commit the repair, and say so.
- If you find a bug you cannot fix safely, file it with `ticket_create` and move
  on. A well-written ticket is a good outcome, not a failure.
- Leave the tree compiling at the end of every commit.

Start now. Say which target you picked and why, then work it.
EOF
}

# What a rotated-in conversation cannot read anywhere else. The ledger carries
# what was learned; this carries the state of the tree it is inheriting.
handover_prompt() {
    cat <<'EOF'

---

The conversation that ran before this one has been rotated out. Where it left
the working tree, measured from git rather than reported by it:

EOF
    printf '%s\n' "$1"
    cat <<'EOF'

Anything uncommitted is work somebody else started and did not finish. Read it
before deciding what to do with it, then either finish and commit it or revert
it. Do not leave it sitting there while you start something new.
EOF
}

continue_prompt() {
    cat <<'EOF'
Continue with the next bug.

Facts from the last iteration, measured from git rather than reported:

EOF
    printf '%s\n' "$1"
    cat <<'EOF'

Pick the next target and work it the same way: failing test, fix, ledger entry,
narrow stage, one commit. If nothing in the ledger's unswept list appeals, audit
a crate nobody has covered yet and name it.
EOF
}

# `$1` is the gate's full output, which the run's logs keep. The turn gets only
# the compiler's error lines, or the tail when there are none to pick out.
repair_prompt() {
    local errors
    # `grep` exits 1 when it matches nothing, which `set -e` would treat as
    # fatal inside an assignment.
    errors=$(printf '%s\n' "$1" | grep -E 'error(\[E[0-9]+\])?:' | head -30 || true)
    if [[ -z $errors ]]; then
        errors=$(printf '%s\n' "$1" | tail -30)
    fi

    cat <<'EOF'
Stop. Repair the build before anything else.

`cargo check --workspace --all-targets` fails on the current working tree:

```
EOF
    printf '%s\n' "$errors"
    fill <<'EOF'
```

Fix this first. Do not pick up new work, and do not commit anything unrelated to
the repair. When the workspace compiles again, commit the repair on its own, and
record in `@STATE@/LEDGER.md` what was broken and what fixed it.
EOF
}
