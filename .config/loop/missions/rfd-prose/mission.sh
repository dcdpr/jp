# RFD prose: improve the prose of every RFD and draft, one RFD per commit.
#
# Sourced by `.config/loop/run.sh`. `next-target` walks the RFDs oldest first,
# each one gets its own conversation with the RFD attached, and `gate` checks
# that the commit changes only that RFD's prose. Progress is recorded in
# `state/progress.tsv`, so a later run resumes where an earlier one stopped.
#
#   .config/loop/run.sh rfd-prose --iterations 1        # one RFD
#   .config/loop/run.sh rfd-prose --hours 10            # full run
#
# `--no-gate` would loop on the first RFD forever: only the gate records an RFD
# as finished.

MODEL=google/gemini-4-argon
JP_CFG=(
    --cfg=personas/prose
    --cfg=skill/git-stage
    --cfg=skill/git-commit-writing
)
ROTATE_ON_TARGET=1
# One conversation per RFD, retries included. A single prose turn runs to 40-80
# events, so the harness's event limit would start every retry from scratch.
ROTATE_EVENTS=100000
SNAPSHOT_FILES=(progress.tsv)
GATE_LABEL='prose gate'

# A rewrite that fails the gate is thrown away, and the RFD is retried with the
# gate's reason in the prompt, up to three times.
DISCARD_ON_GATE_FAILURE=1

if ! command -v comfort >/dev/null 2>&1; then
    die "comfort is not on PATH; install it with \`just _install-comfort\`"
fi
if ! command -v node >/dev/null 2>&1; then
    die "node is not on PATH; the gate's anchor check needs it"
fi

# The gate counts a failed anchor check as a failed attempt, so a checker that
# crashes would spend three turns on every RFD. Run it once before any turn.
if ! anchor_smoke=$(cd "$MISSION_DIR" && node check-anchors.mjs HEAD HEAD docs/rfd/001-jp-rfd-process.md 2>&1); then
    die "the anchor check fails on its own: $anchor_smoke"
fi

init_state() {
    touch "$STATE/progress.tsv"
}

rfd_id() {
    local name
    name=$(basename "$1")
    printf '%s\n' "${name%%-*}"
}

conversation_args() {
    printf -- '--attach=%s\n' "$1"
}

# Prompts use quoted heredocs so backticks and `$` remain literal.
# `fill` substitutes @RUN_ID@, @BASELINE@, and @STATE@.

mission_prompt() {
    fill <<'EOF'
Evaluate and fix the prose of the attached RFD. You are working unattended, so
nobody will answer questions: make your own calls.

This is a full prose pass, not a light touch-up. Tighten wordy sentences, cut
padding and repetition, merge sentences that say the same thing, untangle
riddle and garden-path sentences, replace vague wording with the precise
technical term, and rewrite or reorder sentences and paragraphs wherever that
makes the RFD read better. Headings are prose too: rewrite one when a clearer
heading exists, and update this RFD's `#anchor` links that point at it. Leave
every other link's anchor as written. The scope is this one file, so skip
looking for recently changed files.

Read the whole RFD first and note the changes that cross sections. Then work
through it a section or two at a time: apply those sections' changes with
`fs_modify_file` before reasoning about the next ones. Don't plan the whole
RFD before the first edit: reasoning counts toward the output token limit, and
a long RFD planned in one response runs out before anything is edited.

Keep what the RFD says. It records a decision as it stood when it was written,
whether it is published, superseded, abandoned, or a draft. Every decision,
claim, and example stays, and identifiers (type names, config keys, CLI flags,
file paths) stay as written even where the code has since changed. Don't add
notes, tips, or cross-references to other RFDs.

The harness checks that these stay byte-identical:

- the title line and the metadata list below it;
- fenced code blocks;
- link reference definitions. Don't add new ones.

Edit only this RFD, and don't rename, renumber, promote, or relate it.

When you are done:

1. Run `markdown_format` with `paths` set to the RFD's path only. Without
   `paths` it reformats every Markdown file in the workspace.
2. Stage the RFD with the git staging tools, and commit it with `git_commit`.
   The subject is exactly `docs(rfd): Improve prose of RFD <id>`; the body
   names the main patterns you corrected.

Make exactly one commit. Never stage or commit anything under `@STATE@/`, and
never rewrite git history.
EOF
}

# Each conversation covers one RFD, so there is nothing to hand over.
handover_prompt() {
    :
}

continue_prompt() {
    cat <<'EOF'
Your previous turn on this RFD did not finish. Status after it (measured from
git):

EOF
    printf '%s\n' "$1"
}

target_prompt() {
    local id reason=""
    id=$(rfd_id "$1")
    if [[ -f $STATE/gate-reason && $(head -n 1 "$STATE/gate-reason") == "$id" ]]; then
        reason=$(tail -n +2 "$STATE/gate-reason")
    fi

    printf '\n---\n\nThe RFD is RFD %s, `%s`.\n' "$id" "$1"
    if [[ -n $reason ]]; then
        cat <<'EOF'

An earlier attempt at this RFD failed the harness's checks and was discarded.
The file is back at its original text. The checks reported:

```
EOF
        printf '%s\n' "$reason"
        cat <<'EOF'
```
EOF
    fi
}

# Unreachable while DISCARD_ON_GATE_FAILURE=1, which discards a failed
# iteration instead of asking for a repair.
repair_prompt() {
    printf 'The previous change failed the prose gate:\n\n%s\n' "$1"
}
