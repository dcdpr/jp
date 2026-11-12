# rustqual: burn down the findings of one rustqual rule, one finding per commit.
#
# Sourced by `.config/loop/run.sh`. A run works on one rule, named with
# `--rule`, on the branch `qual/<rule>`. `next-target` picks the rule's next
# finding from `qual report`, and `gate` keeps the turn's commit only when the
# finding is gone and nothing else got worse; any other work is discarded.
#
#   .config/loop/run.sh rustqual --rule CX-004 --iterations 1    # one finding
#   .config/loop/run.sh rustqual --rule CX-004 --hours 10        # full run
#
# Before a rule's first run, read a sample of its findings as the model gets
# them, and fix the rule's guidance file wherever a finding plus its guidance
# isn't enough to make the fix:
#
#   cargo run --package qual -- report --rule CX-004 --format ai
#
# Two files in `state/` outlive a run, one `{key, reason}` object per line, with
# a finding's `(rule, file, symbol)` as its key:
#
# - `attempted.jsonl`: findings whose fix failed the gate. Delete a line to have
#   a later run try that finding again.
# - `rejected.jsonl`: findings the model judged shouldn't be fixed, for a
#   maintainer to turn into a reasoned suppression or a policy change.
#
# `next-target` skips both, so every finding gets one attempt.
#
# `--no-gate` would retry a finding the model can't fix forever: only the gate
# records a finding as attempted or rejected.

# shellcheck source=lib.sh
source "$MISSION_DIR/lib.sh"

MODEL=astra
ROTATE_COMMITS=10
ROTATE_EVENTS=40
COMPACT='r+t:..-3'       # strip reasoning + tool calls outside the last 2 turns
JP_CFG=(
    --cfg=personas/dev
    --cfg=skill/git-stage
    --cfg=skill/git-commit-writing
)
SNAPSHOT_FILES=(attempted.jsonl rejected.jsonl)
GATE_LABEL='rustqual gate'

# A fix that fails the gate is discarded, kept under `refs/loop/discarded/`,
# and its finding recorded as attempted.
DISCARD_ON_GATE_FAILURE=1

RULE=""

if ! command -v rustqual >/dev/null 2>&1; then
    die "rustqual is not on PATH; install it with \`just _install-rustqual\`"
fi
if ! cargo nextest --version >/dev/null 2>&1; then
    die "cargo-nextest is not installed; the gate runs the tests with it"
fi

parse_option() {
    case $1 in
        --rule)
            if [[ $# -lt 2 ]]; then
                die "--rule needs a rule ID, such as CX-004"
            fi
            RULE=$2
            OPTION_COUNT=2
            ;;
        *) return 1 ;;
    esac
}

option_usage() {
    cat <<'EOF'
  --rule ID           The rustqual rule to burn down, such as CX-004 (required)
EOF
}

preflight() {
    local guidance branch report
    if [[ -z $RULE ]]; then
        die "no rule: pass --rule <ID>, such as --rule CX-004"
    fi

    guidance=$(guidance_path "$RULE")
    if [[ ! -f $guidance ]]; then
        die "$guidance doesn't exist; a turn needs the rule's guidance in its prompt"
    fi

    branch=qual/$(printf '%s' "$RULE" | tr '[:upper:]' '[:lower:]' | tr / _)
    if [[ $BRANCH != "$branch" ]]; then
        die "a run for $RULE works on the branch '$branch', but HEAD is on '$BRANCH'; switch to it first (\`git switch -c $branch\` creates it)"
    fi

    # `qual report --rule` fails on a rule the policy exempts. Unchecked, an
    # exempt rule has no findings in any report, and the run would end as if
    # the rule were burned down.
    echo "$MISSION: reading the findings of $RULE..." >&2
    if ! report=$(qual report --rule "$RULE" --format json 2>&1); then
        die "\`qual report --rule $RULE\` failed: $report"
    fi

    export QUAL_RULE=$RULE
}

init_state() {
    touch "$STATE/attempted.jsonl" "$STATE/rejected.jsonl"
    # A new run starts with no failed gate behind it.
    echo 0 > "$STATE/failures"
    rm -f "$STATE/gate-reason" "$STATE/rejection"
}

# `fill`, which also replaces `@RULE@`.
fill_rule() {
    local t
    t=$(fill)
    t=${t//@RULE@/"$RULE"}
    printf '%s\n' "$t"
}

# Prompts use quoted heredocs so backticks and `$` remain literal.

mission_prompt() {
    fill_rule <<'EOF'
You are fixing rustqual findings of one rule, `@RULE@`, in this repository,
unattended. Nobody is watching, and nobody will answer questions: make your own
calls.

Every turn names one finding and includes the project's guidance for the rule.
Fix that finding, and only that one:

1. Read the code the finding points at, and enough around it to understand
   what it does and who calls it.
2. Fix it the way the guidance says, trying its fixes in the order it lists
   them.
3. Run `cargo_check`, then `cargo_test` on the package you changed, then
   `cargo_format`.
4. Stage only this fix with the git staging tools, and commit it with
   `git_commit`. Make exactly one commit. Its subject follows the repository's
   convention, such as ``refactor(cli): Split `Query::run` by phase``, and its
   body names the rule and says what changed.

Some findings shouldn't be fixed: a false positive, a case the guidance says to
leave as it is, or a fix that would make the code worse. Reject such a finding
instead of fixing it: write your reason as plain text to a new file,
`@STATE@/rejection`, with `fs_create_file`, and leave the code untouched. A
maintainer reads every rejection. Where the guidance says to report something,
a rejection is how you report it.

A harness checks every turn. It measures the result from git, cargo, and
rustqual, and doesn't read your summary:

- rustqual no longer reports the finding;
- no rustqual finding of any rule is new, anywhere in the workspace;
- `cargo check --workspace --all-targets` passes, and so do the tests of every
  package the commit touches;
- the turn made exactly one commit and left nothing uncommitted.

When a check fails, the harness discards the turn's work and tells you why in
the next turn. A finding gets one attempt.

Hard rules:

- Never add a `qual:allow`, `qual:api`, or `qual:test_helper` marker, and never
  change anything under `.config/rustqual/` or `.config/loop/`. The guidance
  names a marker for when a user asks for one, and nobody does here: reject the
  finding instead.
- Change only what the fix needs. Don't tidy nearby code, and don't fix other
  findings you notice along the way.
- Never stage or commit anything under `@STATE@/`. The harness owns that
  directory.
- Never rewrite git history: no reset, no amend, no rebase, no force-push.
EOF
}

# A turn ends with its fix committed or its work discarded, so a new
# conversation has nothing unfinished to pick up.
handover_prompt() {
    cat <<'EOF'

---

The previous conversation was rotated out. Status after its last turn (measured
from git):

EOF
    printf '%s\n' "$1"
}

continue_prompt() {
    cat <<'EOF'
Status after your last turn (measured from git):

EOF
    printf '%s\n' "$1"
}

target_prompt() {
    printf '\n---\n'
    if [[ -f $STATE/gate-reason ]]; then
        cat <<'EOF'

The previous turn's finding is closed: the harness discarded the work on it,
and it won't be retried. Its checks reported:

```
EOF
        cat "$STATE/gate-reason"
        cat <<'EOF'
```

Don't repeat that on this finding.
EOF
    fi

    printf '\nThe next finding of `%s`, followed by the guidance for the rule:\n\n%s\n' "$RULE" "$1"
}

# The gate never asks for a repair in place: it discards a failed iteration.
repair_prompt() {
    :
}
