#!/usr/bin/env bash
#
# Unattended agent loop.
#
# Runs `jp` in a loop against this worktree on behalf of a mission: a directory
# under .config/loop/missions/ that supplies the settings, the prompts, and the
# gate. Progress is measured from git, never from what the assistant claims.
# This script's own record is RUNLOG.md.
#
# Everything a run writes lives in the mission's `state/` directory: what the
# mission keeps across runs (a ledger, say) at its top, and one directory per
# run under `state/runs/<run id>/`, which no later run writes into. No shared
# git state is touched.
#
#   .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
#   .config/loop/run.sh bug-hunt --hours 10                    # the real thing
#
# Stop it with Ctrl-C (or `kill`); it writes a closing summary either way.
#
# README.md, next to this script, describes what a mission provides.

set -euo pipefail

die() { echo "${MISSION:-loop}: $*" >&2; exit 1; }

# ── configuration ───────────────────────────────────────────────────────────
#
# Harness defaults. The mission's mission.sh may override any of these, and the
# command line overrides both.

HOURS=9                  # wall-clock budget; the weekly window resets after this
UNTIL=""                 # absolute local stop time, HH:MM; overrides HOURS
ITERATIONS=0             # 0 = unlimited, bounded only by the deadline
TURN_TIMEOUT=3600        # per-turn kill, seconds; 0 disables
ROTATE_COMMITS=10        # new conversation after this many commits
ROTATE_EVENTS=40         # ...or this many conversation events
ROTATE_FAILURES=2        # ...or this many consecutive failed turns
MAX_FAILURES=5           # give up after this many in a row that are not limits
GATE=1                   # run the mission's gate between turns
COMPACT=''               # `--compact` spec for every turn; empty passes none

# The model every turn and every capacity probe uses. One variable, because a
# probe on a different model than the turns measures a different allowance and
# reports capacity that the turns cannot spend. The mission sets it.
MODEL=""

JP_CFG=()                # the mission's `--cfg` flags: persona, skills
SNAPSHOT_FILES=()        # state files copied into the run's snapshots each turn
GATE_LABEL="gate"        # how the gate's result is named in the facts
DISCARD_ON_GATE_FAILURE=0 # reset the branch when an iteration fails the gate

# The session (5-hour) window fills long before the weekly one. Hitting it is
# not a failure to give up on: the script waits it out and probes for capacity,
# so the remaining weekly allowance still gets spent.
PROBE_INTERVAL=600       # seconds between capacity probes
MAX_WAIT=21600           # give up waiting after this long (6h), deadline aside

# A laptop that changes wifi node mid-turn kills the stream. That is neither a
# limit nor the assistant's doing, so it must not spend the failure budget or
# throw away a warm conversation.
TRANSIENT_BACKOFF=120    # pause before retrying a turn a stalled stream killed
MAX_TRANSIENT=8          # consecutive stalls before calling it something else

usage() {
    cat <<EOF
Unattended agent loop.

  .config/loop/run.sh <mission> [options]

  .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
  .config/loop/run.sh bug-hunt --hours 10                    # the real thing

<mission> names a directory under .config/loop/missions/.

Options:
  --hours N           Wall-clock budget (default: $HOURS)
  --until HH:MM       Stop at this local time instead; the next one if past
  --iterations N      Stop after N iterations (default: unlimited)
  --model M           Model for turns and probes alike (default: ${MODEL:-none})
  --probe-interval N  Seconds between capacity probes while rate limited
  --timeout N         Per-turn timeout in seconds, 0 to disable (default: $TURN_TIMEOUT)
  --rotate-commits N  Start a new conversation after N commits (default: $ROTATE_COMMITS)
  --rotate-events N   ...or after N conversation events (default: $ROTATE_EVENTS)
  --no-gate           Skip the mission's gate between turns
  --help
EOF
}

# ── mission ─────────────────────────────────────────────────────────────────

HARNESS_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

if [[ $# -eq 0 ]]; then
    usage >&2
    exit 2
fi
case $1 in
    --help|-h) usage; exit 0 ;;
    -*) echo "loop: the first argument names the mission" >&2; usage >&2; exit 2 ;;
esac

if [[ ! -d $HARNESS_DIR/missions/$1 ]]; then
    die "no mission '$1' in $HARNESS_DIR/missions/"
fi
MISSION_DIR=$(cd "$HARNESS_DIR/missions/$1" && pwd)
shift
MISSION=$(basename "$MISSION_DIR")

if [[ ! -f $MISSION_DIR/mission.sh ]]; then
    die "$MISSION_DIR has no mission.sh"
fi

# Missions write their prompts as quoted heredocs and pipe them through this.
# It replaces @RUN_ID@, @BASELINE@ and @STATE@ with this run's values, which are
# only known once the run has started.
fill() {
    local t
    t=$(cat)
    t=${t//@RUN_ID@/"$RUN_ID"}
    t=${t//@BASELINE@/"$BASELINE_SHORT"}
    t=${t//@STATE@/"$STATE_REL"}
    printf '%s\n' "$t"
}

# shellcheck source=/dev/null
source "$MISSION_DIR/mission.sh"

for fn in mission_prompt handover_prompt continue_prompt repair_prompt; do
    if ! declare -F "$fn" >/dev/null; then
        die "$MISSION_DIR/mission.sh does not define $fn"
    fi
done

NEXT_TARGET=""
GATE_BIN=""
for exe in next-target gate; do
    path="$MISSION_DIR/$exe"
    if [[ ! -e $path ]]; then
        continue
    fi
    if [[ ! -x $path ]]; then
        die "$path exists but is not executable"
    fi
    case $exe in
        next-target) NEXT_TARGET=$path ;;
        gate) GATE_BIN=$path ;;
    esac
done

while [[ $# -gt 0 ]]; do
    case $1 in
        --hours)           HOURS=$2; shift 2 ;;
        --until)           UNTIL=$2; shift 2 ;;
        --iterations)      ITERATIONS=$2; shift 2 ;;
        --probe-interval)  PROBE_INTERVAL=$2; shift 2 ;;
        --model)           MODEL=$2; shift 2 ;;
        --timeout)         TURN_TIMEOUT=$2; shift 2 ;;
        --rotate-commits)  ROTATE_COMMITS=$2; shift 2 ;;
        --rotate-events)   ROTATE_EVENTS=$2; shift 2 ;;
        --no-gate)         GATE=0; shift ;;
        --help|-h)         usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ -z $MODEL ]]; then
    die "no model: set MODEL in mission.sh or pass --model"
fi

# ── paths ───────────────────────────────────────────────────────────────────

ROOT=$(git -C "$HARNESS_DIR" rev-parse --show-toplevel 2>/dev/null) \
    || die "$HARNESS_DIR is not inside a work tree"
STATE_REL="$(git -C "$HARNESS_DIR" rev-parse --show-prefix)missions/$MISSION/state"
STATE="$ROOT/$STATE_REL"
RUNS="$STATE/runs"

# The state directory sits in the tree, but it is the harness's record, not the
# assistant's work. Every git question about the work (what is dirty, what to
# reset) is asked of this pathspec instead of the whole tree.
NOT_STATE=(. ":(exclude)$STATE_REL")

cd "$ROOT"
mkdir -p "$RUNS"

# ── git must not stop to ask for a passphrase ───────────────────────────────
#
# Commit signing goes through the 1Password SSH agent, which prompts. These
# reach `git` through jp -> jp-tools -> duct, which inherits this environment.
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=commit.gpgsign
export GIT_CONFIG_VALUE_0=false

# ── preflight ───────────────────────────────────────────────────────────────

for bin in jp jp-tools jq cargo git; do
    if ! command -v "$bin" >/dev/null 2>&1; then
        die "$bin is not on PATH"
    fi
done

TIMEOUT_BIN=""
if [[ $TURN_TIMEOUT -gt 0 ]]; then
    if command -v timeout >/dev/null 2>&1; then
        TIMEOUT_BIN=timeout
    elif command -v gtimeout >/dev/null 2>&1; then
        TIMEOUT_BIN=gtimeout
    else
        echo "$MISSION: no timeout(1) found; turns will run unbounded" >&2
    fi
fi

if ! git rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    die "not inside a work tree"
fi

BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [[ $BRANCH == "main" || $BRANCH == "master" || $BRANCH == "HEAD" ]]; then
    die "refusing to run on '$BRANCH'"
fi

# The probe below commits whatever is staged. Anything already in the index
# would land in it, and would also be swept into the assistant's first commit.
if ! git diff --cached --quiet; then
    die "the index has staged changes; commit or unstage them first"
fi

# Discarding an iteration resets the tree outside the state directory, which
# would take unstaged edits that predate the run with it.
if [[ $DISCARD_ON_GATE_FAILURE -eq 1 ]] && ! git diff --quiet -- "${NOT_STATE[@]}"; then
    die "this mission discards failed iterations; commit or revert unstaged changes first"
fi

# Run the exact command shape `git_commit` uses. If it prompts or fails, it
# would do so at 3am with nobody watching. `--soft` undoes it without touching
# the index or the working tree, so unstaged work survives.
echo "$MISSION: probing commit..." >&2
if ! git commit --allow-empty --signoff -m "$MISSION: commit probe" >/dev/null 2>&1; then
    die "the commit probe failed; git cannot commit unattended here"
fi
git reset --soft HEAD~1

BASELINE=$(git rev-parse HEAD)
BASELINE_SHORT=$(git rev-parse --short HEAD)
START_EPOCH=$(date +%s)
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)

# An absolute stop time survives a restart; a duration does not. The weekly
# allowance resets at a fixed instant, and spending past it eats next week's.
if [[ -n $UNTIL ]]; then
    today=$(date +%F)
    if ! DEADLINE=$(date -j -f "%Y-%m-%d %H:%M" "$today $UNTIL" +%s 2>/dev/null); then
        if ! DEADLINE=$(date -d "$today $UNTIL" +%s 2>/dev/null); then
            die "could not parse --until '$UNTIL'; expected HH:MM"
        fi
    fi
    if [[ $DEADLINE -le $START_EPOCH ]]; then
        DEADLINE=$((DEADLINE + 86400))
    fi
else
    DEADLINE=$((START_EPOCH + HOURS * 3600))
fi
DEADLINE_HUMAN=$(date -r "$DEADLINE" '+%Y-%m-%d %H:%M %Z' 2>/dev/null \
    || date -d "@$DEADLINE" '+%Y-%m-%d %H:%M %Z' 2>/dev/null \
    || echo "$DEADLINE")

# One directory per run. The iteration counter restarts at 1 every run, so a
# shared directory would have each run overwrite the last one's prompts, turn
# logs and snapshots. `mkdir` without `-p` fails rather than reuse one.
RUN_DIR="$RUNS/$RUN_ID"
mkdir "$RUN_DIR" || die "could not create $RUN_DIR"
RUN_SNAPS="$RUN_DIR/snapshots"
mkdir "$RUN_SNAPS"
RUNLOG="$RUN_DIR/RUNLOG.md"

# What `next-target` and `gate` can rely on. LOOP_ITERATION and
# LOOP_ITERATION_BASE are refreshed before each iteration.
export LOOP_MISSION=$MISSION
export LOOP_MISSION_DIR=$MISSION_DIR
export LOOP_ROOT=$ROOT
export LOOP_STATE=$STATE
export LOOP_RUN_ID=$RUN_ID
export LOOP_RUN_DIR=$RUN_DIR
export LOOP_BASELINE=$BASELINE

# A session of its own. Without one, `jp` keys the session to the terminal the
# harness was started from, and every `--new` turn becomes that terminal's
# active conversation: the next `jp query` typed there lands in the run's
# conversation. One per run, so two runs never share a session either.
export JP_SESSION="loop-$MISSION-$RUN_ID"

# ── state and run log ───────────────────────────────────────────────────────

if declare -F init_state >/dev/null; then
    init_state
fi

{
    echo "# $MISSION run $RUN_ID"
    echo ""
    echo "Written by \`.config/loop/run.sh\`, measured from git rather than reported by"
    echo "the assistant. Do not edit."
    echo ""
    echo "- branch: \`$BRANCH\`"
    echo "- baseline: \`$BASELINE_SHORT\`"
    echo "- model: \`$MODEL\`"
    echo "- stops at: $DEADLINE_HUMAN"
    echo "- rotate at: ${ROTATE_COMMITS} commits / ${ROTATE_EVENTS} events"
    echo ""
} > "$RUNLOG"

# ── jp plumbing ─────────────────────────────────────────────────────────────

JP_GLOBALS=(
    -v
    --no-interactive
    ${JP_CFG[@]+"${JP_CFG[@]}"}
    # 60s is tuned for someone watching. Unattended, a wifi handoff or a moved
    # laptop routinely exceeds it, and the turn dies with committed work behind
    # it. The cost of raising it is that a genuinely dead stream takes three
    # minutes per attempt to notice instead of one.
    --cfg=assistant.request.stream_idle_timeout_secs=180
    # bookworm is a required MCP server by default, spawned through
    # `just serve-bookworm`, which builds from this checkout. Without this, a
    # workspace the assistant broke aborts every turn — including the one that
    # would have repaired it.
    --cfg=providers.mcp.bookworm.optional=true
)

COMPACT_ARGS=()
if [[ -n $COMPACT ]]; then
    COMPACT_ARGS=(--compact="$COMPACT")
fi

conversation_id_for_title() {
    jp -F json conversation ls 2>/dev/null \
        | jq -r --arg t "$1" 'first(.[] | select(.title == $t) | .id) // empty' \
        2>/dev/null || true
}

# The conversation this session last activated, which a `--new` run just made
# its own. Used only when the title lookup comes up empty.
session_conversation_id() {
    jp -F json conversation ls +s 2>/dev/null \
        | jq -r '.[-1].id // empty' 2>/dev/null || true
}

event_count_for_id() {
    jp -F json conversation ls 2>/dev/null \
        | jq -r --arg id "$1" 'first(.[] | select(.id == $id) | .events) // 0' \
        2>/dev/null || echo 0
}

# ── rate limits ─────────────────────────────────────────────────────────────
#
# The subscription has two windows: a 5-hour one that fills fast, and a weekly
# one. Neither is readable from `jp`, and the subscription flow on this branch
# runs Claude Code out of process (RFD 110), so there is no header for the
# script to read either. Rather than reverse-engineer an undocumented usage
# endpoint, ask the only question that matters — can a request go through right
# now — by sending one.

PROBE_COUNT=0

# One cheap request on the run's own model. The 5-hour window is account-wide
# but the weekly ones are per model family, so a probe on a smaller model would
# answer a different question than the one being asked.
run_probe() {
    PROBE_COUNT=$((PROBE_COUNT + 1))
    local out="$RUN_DIR/probe-$(printf '%03d' "$PROBE_COUNT").log"
    set +e
    jp --no-interactive --no-persist query --new --no-tools --no-reasoning -E \
        --model "$MODEL" "Reply with: ok" > "$out" 2>&1
    local rc=$?
    set -e
    return $rc
}

# Whether a failed turn failed because the allowance ran out.
#
# Only the tail of the log, and only after a non-zero exit: the assistant reads
# this repository's own source, and `jp_llm/src/error.rs` is full of the very
# phrases being matched here.
turn_hit_a_limit() {
    tail -n 40 "$1" | grep -qiE \
        'rate limited|rate.limit.error|insufficient api quota|usage limit|limit reached|subscription limit|quota exceeded|out of credits'
}

# Whether a failed turn died on the network rather than on anything it did.
#
# `jp` renders a stream error as a two-row table whose `kind` row carries
# `StreamErrorKind::as_str()`, so the kind is matched where it is labelled
# rather than as a loose word: a turn that spent itself reading this repository
# leaves "timeout" all over the log without having hit one.
turn_hit_transient() {
    tail -n 40 "$1" | grep -qiE \
        'no activity from provider|\| *kind *\| *(timeout|connection error|server error)'
}

# Sleep until a request succeeds again, the deadline passes, or waiting stops
# being plausible. Returns non-zero when the caller should stop.
wait_for_capacity() {
    local waited=0 now
    echo "$MISSION: allowance exhausted; waiting for the window to reopen" >&2
    {
        echo "## Waiting for capacity — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo ""
    } >> "$RUNLOG"

    while :; do
        now=$(date +%s)
        if [[ $now -ge $DEADLINE ]]; then
            echo "$MISSION: deadline reached while waiting" >&2
            return 1
        fi
        if [[ $waited -ge $MAX_WAIT ]]; then
            echo "$MISSION: waited $((waited / 60))m without recovering, stopping" >&2
            return 1
        fi

        # Never sleep past the deadline.
        local nap=$PROBE_INTERVAL
        if [[ $((now + nap)) -gt $DEADLINE ]]; then
            nap=$((DEADLINE - now))
        fi
        if [[ $nap -gt 0 ]]; then
            sleep "$nap"
        fi
        waited=$((waited + nap))

        if run_probe; then
            echo "$MISSION: capacity is back after $((waited / 60))m" >&2
            {
                echo "- recovered after $((waited / 60))m, $(date -u +%Y-%m-%dT%H:%M:%SZ)"
                echo ""
            } >> "$RUNLOG"
            return 0
        fi
        echo "$MISSION: still limited after $((waited / 60))m" >&2
    done
}

# ── targets and discards ────────────────────────────────────────────────────

# Used when the mission has a `next-target` but no `target_prompt` of its own.
default_target_prompt() {
    cat <<'EOF'

---

Work this target, and only this one:

EOF
    printf '%s\n' "$1"
}

# Reset the branch, and the tree outside the state directory, to where the
# iteration started. Returns non-zero when HEAD has left the run's branch,
# without touching anything, or when git fails.
discard_iteration() {
    local base=$1 branch_now path
    branch_now=$(git rev-parse --abbrev-ref HEAD)
    if [[ $branch_now != "$BRANCH" ]]; then
        echo "$MISSION: HEAD is on '$branch_now', not '$BRANCH'; refusing to discard" >&2
        return 1
    fi
    # Not `reset --hard`: that would also roll back the state directory, which
    # holds this run's own record.
    git reset --quiet "$base" || return 1
    git restore --source="$base" --worktree -- "${NOT_STATE[@]}" || return 1

    # Files the iteration created, committed or not, are untracked now. Remove
    # those and only those: anything untracked before it started is not ours.
    comm -13 \
        <(printf '%s\n' "$UNTRACKED_BEFORE" | sort) \
        <(git ls-files --others --exclude-standard -- "${NOT_STATE[@]}" | sort) \
        | while IFS= read -r path; do
            if [[ -n $path ]]; then
                rm -f -- "$path"
            fi
        done
}

# ── the loop ────────────────────────────────────────────────────────────────

CONV=""                   # current conversation id
CONV_TITLE=""
CONV_BASE=$BASELINE       # HEAD when the current conversation started
CONV_INDEX=0
ITER=0
FAILURES=0                # consecutive
GATE_OUTPUT=""
GATE_OK=1
LAST_FACTS=""
ROTATE_REASON="first conversation"
TURN_PID=""
TRANSIENTS=0             # consecutive stalled streams
UNTRACKED_BEFORE=""

finish() {
    local total
    total=$(git rev-list --count "$BASELINE..HEAD" 2>/dev/null || echo 0)
    {
        echo "## Finished — $(date -u +%Y-%m-%dT%H:%M:%SZ)"
        echo ""
        echo "- iterations: $ITER"
        echo "- conversations: $CONV_INDEX"
        echo "- commits: $total"
        echo "- capacity probes sent: $PROBE_COUNT"
        echo "- range: \`$BASELINE_SHORT..$(git rev-parse --short HEAD)\`"
        echo ""
    } >> "$RUNLOG"

    {
        echo ""
        echo "$MISSION: $total commit(s) over $ITER iteration(s), $CONV_INDEX conversation(s)"
        echo "$MISSION: review with  git log --oneline $BASELINE_SHORT..HEAD"
        echo "$MISSION: state at     $STATE"
        echo "$MISSION: run log at   $RUNLOG"
    } >&2
}

# Bash defers a trap until the running foreground command returns, so a turn is
# started in the background and waited on: `wait` is interruptible, and the trap
# fires while the turn is still in flight rather than after it.
on_interrupt() {
    echo "" >&2
    echo "$MISSION: interrupted" >&2
    if [[ -n $TURN_PID ]]; then
        kill -TERM "$TURN_PID" 2>/dev/null || true
    fi
    finish
    exit 130
}
trap on_interrupt INT TERM

echo "$MISSION: branch $BRANCH, model $MODEL, baseline $BASELINE_SHORT," \
     "stopping at $DEADLINE_HUMAN" >&2

while :; do
    now=$(date +%s)
    if [[ $now -ge $DEADLINE ]]; then
        echo "$MISSION: wall-clock budget spent" >&2
        break
    fi
    if [[ $ITERATIONS -gt 0 && $ITER -ge $ITERATIONS ]]; then
        echo "$MISSION: iteration limit reached" >&2
        break
    fi

    head_before=$(git rev-parse HEAD)
    export LOOP_ITERATION
    LOOP_ITERATION=$(printf '%03d' $((ITER + 1)))
    export LOOP_ITERATION_BASE=$head_before

    # Asked before the iteration counts, so a run that ends for want of targets
    # does not report an iteration that never ran. A repair turn has its target
    # already: the broken tree.
    TARGET=""
    if [[ -n $NEXT_TARGET && $GATE_OK -eq 1 ]]; then
        set +e
        TARGET=$("$NEXT_TARGET")
        target_rc=$?
        set -e
        if [[ $target_rc -ne 0 ]]; then
            echo "$MISSION: next-target exited $target_rc; stopping" >&2
            break
        fi
        if [[ -z ${TARGET//[[:space:]]/} ]]; then
            echo "$MISSION: no targets remain" >&2
            break
        fi
    fi

    ITER=$((ITER + 1))
    seq=$(printf '%03d' "$ITER")
    log="$RUN_DIR/iter-$seq.log"
    prompt="$RUN_DIR/iter-$seq.prompt"

    # Rotate when the conversation has done enough, grown too large, or failed
    # twice running. A fresh conversation remembers nothing, which is exactly
    # why the mission's state has to carry everything that matters.
    if [[ -n $CONV ]]; then
        commits_here=$(git rev-list --count "$CONV_BASE..HEAD")
        events_here=$(event_count_for_id "$CONV")
        events_here=${events_here:-0}
        if [[ $commits_here -ge $ROTATE_COMMITS ]]; then
            ROTATE_REASON="$commits_here commits"
            CONV=""
        elif [[ $events_here -ge $ROTATE_EVENTS ]]; then
            ROTATE_REASON="$events_here events"
            CONV=""
        elif [[ $FAILURES -ge $ROTATE_FAILURES ]]; then
            ROTATE_REASON="$FAILURES consecutive failures"
            CONV=""
        fi
    fi

    # A new conversation always gets the mission, because it remembers nothing.
    # A broken tree always gets the repair. Both can be true at once.
    : > "$prompt"
    if [[ -z $CONV ]]; then
        mission_prompt >> "$prompt"
        mode="bootstrap"
        if [[ -n $LAST_FACTS ]]; then
            handover_prompt "$LAST_FACTS" >> "$prompt"
        fi
    fi
    if [[ $GATE_OK -eq 0 ]]; then
        if [[ -z $CONV ]]; then
            printf '\n---\n\n' >> "$prompt"
        fi
        repair_prompt "$GATE_OUTPUT" >> "$prompt"
        mode="repair"
    elif [[ -n $CONV ]]; then
        continue_prompt "$LAST_FACTS" >> "$prompt"
        mode="continue"
    fi
    if [[ -n $TARGET ]]; then
        if declare -F target_prompt >/dev/null; then
            target_prompt "$TARGET" >> "$prompt"
        else
            default_target_prompt "$TARGET" >> "$prompt"
        fi
    fi

    if [[ -z $CONV ]]; then
        CONV_INDEX=$((CONV_INDEX + 1))
        CONV_TITLE="$MISSION $RUN_ID #$CONV_INDEX"
        CONV_BASE=$(git rev-parse HEAD)
        echo "$MISSION: [$seq] new conversation ($ROTATE_REASON): $CONV_TITLE" >&2
        jp_target=(--new --local --title "$CONV_TITLE")
    else
        jp_target=(--id "$CONV")
    fi

    for file in ${SNAPSHOT_FILES[@]+"${SNAPSHOT_FILES[@]}"}; do
        cp "$STATE/$file" "$RUN_SNAPS/$seq-$file" 2>/dev/null || true
    done
    if [[ $DISCARD_ON_GATE_FAILURE -eq 1 ]]; then
        UNTRACKED_BEFORE=$(git ls-files --others --exclude-standard -- "${NOT_STATE[@]}")
    fi

    echo "$MISSION: [$seq] $mode turn -> $log" >&2

    # `-E` is load-bearing. A query arriving on stdin with neither `--edit` nor
    # `--no-edit` opens the editor seeded with it, and `--no-interactive` does
    # not suppress that: `force_no_edit()` reads the flag and nothing else. With
    # stdout on a file there is no terminal for the editor to draw on, and the
    # turn blocks until the timeout kills it.
    set +e
    if [[ -n $TIMEOUT_BIN ]]; then
        "$TIMEOUT_BIN" "$TURN_TIMEOUT" \
            jp "${JP_GLOBALS[@]}" query "${jp_target[@]}" -E --model "$MODEL" \
            ${COMPACT_ARGS[@]+"${COMPACT_ARGS[@]}"} < "$prompt" > "$log" 2>&1 &
    else
        jp "${JP_GLOBALS[@]}" query "${jp_target[@]}" -E --model "$MODEL" \
            ${COMPACT_ARGS[@]+"${COMPACT_ARGS[@]}"} < "$prompt" > "$log" 2>&1 &
    fi
    TURN_PID=$!
    wait "$TURN_PID"
    rc=$?
    TURN_PID=""
    set -e

    if [[ -z $CONV ]]; then
        CONV=$(conversation_id_for_title "$CONV_TITLE")
        if [[ -z $CONV ]]; then
            CONV=$(session_conversation_id)
        fi
        if [[ -z $CONV ]]; then
            echo "$MISSION: [$seq] could not resolve the conversation id;" \
                 "the next turn starts a fresh conversation" >&2
        fi
    fi

    # A spent allowance is not the assistant failing. It must not count toward
    # the failure budget, and it must not rotate the conversation: the work so
    # far is fine, there is simply nothing to send it with.
    LIMITED=0
    TRANSIENT=0
    if [[ $rc -eq 0 ]]; then
        FAILURES=0
        TRANSIENTS=0
    elif turn_hit_a_limit "$log"; then
        LIMITED=1
        echo "$MISSION: [$seq] jp exited $rc against a usage limit" >&2
    elif turn_hit_transient "$log"; then
        TRANSIENT=1
        TRANSIENTS=$((TRANSIENTS + 1))
        echo "$MISSION: [$seq] jp exited $rc on a stalled stream" \
             "($TRANSIENTS in a row)" >&2
    else
        FAILURES=$((FAILURES + 1))
        echo "$MISSION: [$seq] jp exited $rc (failure $FAILURES in a row)" >&2
    fi

    # What actually happened, measured rather than reported.
    head_after=$(git rev-parse HEAD)
    if [[ $head_before == "$head_after" ]]; then
        commit_count=0
        commit_lines="  (none)"
    else
        commit_count=$(git rev-list --count "$head_before..$head_after")
        commit_lines=$(git log --no-decorate --format='  - `%h` %s' "$head_before..$head_after")
    fi

    # The gate is not a quality bar — CI is that, on whatever you cherry-pick.
    # It exists so a broken tree is named in the next prompt, instead of being
    # rediscovered mid-edit by a conversation that will blame its own change.
    # A mission that discards failed iterations uses it as the bar instead.
    GATE_OK=1
    GATE_OUTPUT=""
    DISCARDED=0
    gate_text="skipped"
    if [[ $GATE -eq 1 && -n $GATE_BIN ]]; then
        set +e
        gate_raw=$("$GATE_BIN" 2>&1)
        gate_rc=$?
        set -e
        printf '%s\n' "$gate_raw" > "$RUN_DIR/iter-$seq.gate.log"
        if [[ $gate_rc -eq 0 ]]; then
            gate_text="PASS"
        elif [[ $DISCARD_ON_GATE_FAILURE -eq 1 ]]; then
            gate_text="FAIL"
            if ! discard_iteration "$head_before"; then
                break
            fi
            DISCARDED=1
            echo "$MISSION: [$seq] gate FAILED; the iteration is discarded" >&2
        else
            GATE_OK=0
            gate_text="FAIL"
            GATE_OUTPUT=$gate_raw
            echo "$MISSION: [$seq] gate FAILED; the next turn repairs" >&2
        fi
    fi

    dirty=$(git status --porcelain -- "${NOT_STATE[@]}" | wc -l | tr -d ' ')

    LAST_FACTS=$(
        echo "- turn exit status: $rc"
        echo "- commits made: $commit_count"
        printf '%s\n' "$commit_lines"
        echo "- uncommitted files left in the tree: $dirty"
        echo "- $GATE_LABEL: $gate_text"
        if [[ $DISCARDED -eq 1 ]]; then
            echo "- the gate failed, so the harness discarded the iteration;" \
                 "the branch is back at \`$(git rev-parse --short "$head_before")\`"
        fi
        if [[ $LIMITED -eq 1 ]]; then
            echo "- the turn was cut short by a usage limit, not by anything you did"
        fi
        if [[ $TRANSIENT -eq 1 ]]; then
            echo "- the turn was cut short by a network stall, not by anything you did;" \
                 "pick up where it stopped"
        fi
    )

    {
        echo "## Iteration $seq — $(date -u +%Y-%m-%dT%H:%M:%SZ) — $mode — ${CONV:-unknown}"
        echo ""
        printf '%s\n' "$LAST_FACTS"
        echo ""
    } >> "$RUNLOG"

    echo "$MISSION: [$seq] $commit_count commit(s), gate $gate_text, $dirty file(s) dirty" >&2

    if [[ $LIMITED -eq 1 ]]; then
        if ! wait_for_capacity; then
            break
        fi
        continue
    fi

    if [[ $TRANSIENT -eq 1 ]]; then
        if [[ $TRANSIENTS -ge $MAX_TRANSIENT ]]; then
            echo "$MISSION: $TRANSIENTS stalls in a row; the network is not coming back" >&2
            break
        fi
        sleep "$TRANSIENT_BACKOFF"
        continue
    fi

    # The patterns above are a fast path, not the authority. Before giving up on
    # a run of failures, spend one request finding out whether the account can
    # talk to the model at all — a limit worded in a way nothing here matches
    # looks exactly like five broken turns.
    if [[ $FAILURES -ge $MAX_FAILURES ]]; then
        echo "$MISSION: $MAX_FAILURES consecutive failures; probing for capacity" >&2
        if run_probe; then
            echo "$MISSION: the account is fine, so the failures are real; stopping" >&2
            break
        fi
        if ! wait_for_capacity; then
            break
        fi
        FAILURES=0
    fi
done

finish
