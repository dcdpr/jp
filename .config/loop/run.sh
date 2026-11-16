#!/usr/bin/env bash
#
# Unattended agent loop.
#
# Runs `jp` in a loop against this worktree for a named mission: a directory
# under .config/loop/missions/ that supplies settings, prompts, and an optional
# gate. Progress is measured from git, never from what the assistant claims.
# This script writes its own per-run summary to RUNLOG.md.
#
# All run artifacts are written to the mission's `state/` directory: shared
# files (such as `LEDGER.md`) at the root of `state/`, and per-run logs,
# prompts, and snapshots under `state/runs/<run id>/`. No shared git state is
# modified.
#
#   .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
#   .config/loop/run.sh bug-hunt --hours 10                    # full run
#
# Stop the run with Ctrl-C (or `kill`); it writes a closing summary before
# exiting.
#
# See README.md next to this script for the mission contract.

set -euo pipefail

die() { echo "${MISSION:-loop}: $*" >&2; exit 1; }

# Harness defaults. The mission's `mission.sh` may override any of these, and
# command-line flags override both.

HOURS=9                  # wall-clock budget in hours
UNTIL=""                 # absolute local stop time (HH:MM); overrides HOURS
ITERATIONS=0             # 0 = unlimited, bounded only by the deadline
TURN_TIMEOUT=3600        # per-turn timeout in seconds; 0 disables
ROTATE_COMMITS=10        # start a new conversation after this many commits
ROTATE_EVENTS=40         # start a new conversation after this many events
ROTATE_FAILURES=2        # start a new conversation after this many failed turns
ROTATE_ON_TARGET=0       # 1 = start a new conversation whenever the target changes
MAX_FAILURES=5           # stop after this many consecutive non-limit failures
GATE=1                   # run the mission's gate between turns
COMPACT=''               # `--compact` spec for every turn; empty passes none

# Model used for both turns and capacity probes. Probing with the same model
# ensures the probe checks the same rate-limit quota that turns consume.
MODEL=""

JP_CFG=()                # `--cfg` flags for every turn (persona, skills)
SNAPSHOT_FILES=()        # state files copied into the run's snapshots each turn
GATE_LABEL="gate"        # label for the gate result in the post-turn summary
DISCARD_ON_GATE_FAILURE=0 # reset the branch when an iteration fails the gate
MAX_REPAIRS=3            # in-place repairs before a repairable gate failure counts as a plain one

# When the 5-hour session quota is exhausted, wait and probe periodically so
# the run can resume once the window reopens.
PROBE_INTERVAL=600       # seconds between capacity probes
MAX_WAIT=21600           # maximum seconds to wait for quota recovery (6h)

# Network drops and stream timeouts are retried after a backoff without
# incrementing FAILURES or rotating the active conversation.
TRANSIENT_BACKOFF=120    # seconds to wait before retrying after a stream error
MAX_TRANSIENT=8          # maximum consecutive stream errors before aborting

usage() {
    cat <<EOF
Unattended agent loop.

  .config/loop/run.sh <mission> [options]

  .config/loop/run.sh bug-hunt --iterations 1 --hours 1      # smoke test
  .config/loop/run.sh bug-hunt --hours 10                    # full run

<mission> names a directory under .config/loop/missions/.

Options:
  --hours N           Wall-clock budget (default: $HOURS)
  --until HH:MM       Stop at this local time (or the next day if already past)
  --iterations N      Stop after N iterations (default: unlimited)
  --model M           Model for turns and probes (default: ${MODEL:-none})
  --probe-interval N  Seconds between capacity probes while rate limited
  --timeout N         Per-turn timeout in seconds, 0 to disable (default: $TURN_TIMEOUT)
  --rotate-commits N  Start a new conversation after N commits (default: $ROTATE_COMMITS)
  --rotate-events N   Start a new conversation after N events (default: $ROTATE_EVENTS)
  --no-gate           Skip the mission's gate between turns
  --help
EOF
    if declare -F option_usage >/dev/null; then
        printf '\n%s options:\n' "$MISSION"
        option_usage
    fi
}

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

# Replace @RUN_ID@, @BASELINE@, and @STATE@ placeholders in prompt templates
# read from stdin.
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
        *)
            # Delegate unrecognized options to the mission's `parse_option`,
            # which sets `OPTION_COUNT` to the number of arguments consumed.
            OPTION_COUNT=0
            if declare -F parse_option >/dev/null && parse_option "$@" && [[ $OPTION_COUNT -gt 0 ]]; then
                shift "$OPTION_COUNT"
            else
                echo "unknown option: $1" >&2; usage >&2; exit 2
            fi
            ;;
    esac
done

if [[ -z $MODEL ]]; then
    die "no model: set MODEL in mission.sh or pass --model"
fi

ROOT=$(git -C "$HARNESS_DIR" rev-parse --show-toplevel 2>/dev/null) \
    || die "$HARNESS_DIR is not inside a work tree"
STATE_REL="$(git -C "$HARNESS_DIR" rev-parse --show-prefix)missions/$MISSION/state"
STATE="$ROOT/$STATE_REL"
RUNS="$STATE/runs"

# Pathspec that excludes the mission's state directory when checking for dirty
# files or resetting a failed iteration.
NOT_STATE=(. ":(exclude)$STATE_REL")

cd "$ROOT"
mkdir -p "$RUNS"

# Disable commit signing so git never blocks on an interactive SSH/GPG
# passphrase prompt when invoked by jp-tools.
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=commit.gpgsign
export GIT_CONFIG_VALUE_0=false

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

# Reject staged changes so the commit probe below does not include them.
if ! git diff --cached --quiet; then
    die "the index has staged changes; commit or unstage them first"
fi

# Reject unstaged changes when DISCARD_ON_GATE_FAILURE is enabled, since
# resetting a failed iteration would overwrite them.
if [[ $DISCARD_ON_GATE_FAILURE -eq 1 ]] && ! git diff --quiet -- "${NOT_STATE[@]}"; then
    die "this mission discards failed iterations; commit or revert unstaged changes first"
fi

# Run mission-specific preflight checks before the commit probe or run
# directory creation.
if declare -F preflight >/dev/null; then
    preflight
fi

# Verify that `git commit` succeeds non-interactively before starting the loop.
# `git reset --soft HEAD~1` removes the probe commit while preserving the
# working tree.
echo "$MISSION: probing commit..." >&2
if ! git commit --allow-empty --signoff -m "$MISSION: commit probe" >/dev/null 2>&1; then
    die "the commit probe failed; git cannot commit unattended here"
fi
git reset --soft HEAD~1

BASELINE=$(git rev-parse HEAD)
BASELINE_SHORT=$(git rev-parse --short HEAD)
START_EPOCH=$(date +%s)
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)

# Parse HH:MM with BSD `date -j` or GNU `date -d`. If that time has already
# passed today, advance the deadline to tomorrow.
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

# Use `mkdir` without `-p` so starting two runs within the same second fails
# instead of sharing a run directory.
RUN_DIR="$RUNS/$RUN_ID"
mkdir "$RUN_DIR" || die "could not create $RUN_DIR"
RUN_SNAPS="$RUN_DIR/snapshots"
mkdir "$RUN_SNAPS"
RUNLOG="$RUN_DIR/RUNLOG.md"

# Environment variables exposed to `next-target` and `gate`. LOOP_ITERATION and
# LOOP_ITERATION_BASE are updated at the start of each iteration.
export LOOP_MISSION=$MISSION
export LOOP_MISSION_DIR=$MISSION_DIR
export LOOP_ROOT=$ROOT
export LOOP_STATE=$STATE
export LOOP_RUN_ID=$RUN_ID
export LOOP_RUN_DIR=$RUN_DIR
export LOOP_BASELINE=$BASELINE

# Isolate the run in its own JP session so `--new` turns do not change the
# caller terminal's active conversation or collide with concurrent runs.
export JP_SESSION="loop-$MISSION-$RUN_ID"

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

JP_GLOBALS=(
    -v
    --no-interactive
    ${JP_CFG[@]+"${JP_CFG[@]}"}
    # Raise the stream idle timeout from the 60s default so brief network
    # handoffs do not abort an in-progress turn.
    --cfg=assistant.request.stream_idle_timeout_secs=180
    # `just serve-bookworm` compiles from the working tree. Mark it optional so
    # a broken build does not prevent the repair turn from starting.
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

# Fallback when title lookup fails: return the most recently activated
# conversation in this session.
session_conversation_id() {
    jp -F json conversation ls +s 2>/dev/null \
        | jq -r '.[-1].id // empty' 2>/dev/null || true
}

event_count_for_id() {
    jp -F json conversation ls 2>/dev/null \
        | jq -r --arg id "$1" 'first(.[] | select(.id == $id) | .events) // 0' \
        2>/dev/null || echo 0
}

PROBE_COUNT=0

# Send a minimal ephemeral query to check whether rate-limit quota is available.
# Uses `$MODEL` because weekly quota limits are scoped per model family.
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

# Check whether a failed turn exited due to a rate or quota limit.
# Inspects only the last 40 lines of the log so error strings read from source
# files (such as `jp_llm/src/error.rs`) earlier in the turn do not cause false
# positives.
turn_hit_a_limit() {
    tail -n 40 "$1" | grep -qiE \
        'rate limited|rate.limit.error|insufficient api quota|usage limit|limit reached|subscription limit|quota exceeded|out of credits'
}

# Check whether a failed turn exited due to a transient network or stream error.
# Matches the rendered `kind` table row (`StreamErrorKind::as_str()`) rather
# than bare words like "timeout", which frequently appear in tool output.
turn_hit_transient() {
    tail -n 40 "$1" | grep -qiE \
        'no activity from provider|\| *kind *\| *(timeout|connection error|server error)'
}

# Poll with `run_probe` until quota recovers, `DEADLINE` is reached, or
# `MAX_WAIT` elapses. Returns non-zero when the caller should stop.
wait_for_capacity() {
    local waited=0 now
    echo "$MISSION: allowance exhausted; waiting for the window to reopen" >&2
    {
        echo "## Waiting for capacity: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
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

# Default prompt section when the mission provides `next-target` without
# defining `target_prompt`.
default_target_prompt() {
    cat <<'EOF'

---

Work this target, and only this one:

EOF
    printf '%s\n' "$1"
}

# Keep everything an iteration produced reachable before a discard resets it:
# its commits, plus uncommitted and untracked files outside `state/`, recorded as
# one more commit on top of HEAD. The ref is
# `refs/loop/discarded/<mission>/<run id>/<iteration>`, and git never collects a
# commit a ref points at. Sets `DISCARD_REF`. Returns non-zero when the work
# could not be recorded.
pin_iteration() {
    local base=$1 dir tree commit
    if [[ $(git rev-parse HEAD) == "$base" && -z $(git status --porcelain -- "${NOT_STATE[@]}") ]]; then
        return 0
    fi

    # A temporary index, so recording the work leaves the real one untouched.
    dir=$(mktemp -d) || return 1
    if ! tree=$(
        export GIT_INDEX_FILE=$dir/index
        git read-tree HEAD &&
            git add -A -- "${NOT_STATE[@]}" &&
            git write-tree
    ); then
        rm -rf "$dir"
        return 1
    fi
    rm -rf "$dir"

    commit=$(git rev-parse HEAD) || return 1
    if [[ $tree != "$(git rev-parse 'HEAD^{tree}')" ]]; then
        commit=$(git commit-tree "$tree" -p HEAD \
            -m "$MISSION: uncommitted work of iteration $seq, run $RUN_ID") || return 1
    fi
    DISCARD_REF="refs/loop/discarded/$MISSION/$RUN_ID/$seq"
    git update-ref "$DISCARD_REF" "$commit" || return 1
}

# Reset the branch and the working tree (excluding `state/`) to `$1`, after
# recording the work with `pin_iteration`. Returns non-zero without modifying
# files if HEAD has moved off `$BRANCH` or the work could not be recorded, and
# non-zero if git fails.
discard_iteration() {
    local base=$1 branch_now path
    branch_now=$(git rev-parse --abbrev-ref HEAD)
    if [[ $branch_now != "$BRANCH" ]]; then
        echo "$MISSION: HEAD is on '$branch_now', not '$BRANCH'; refusing to discard" >&2
        return 1
    fi
    if ! pin_iteration "$base"; then
        echo "$MISSION: could not record the iteration's work; refusing to discard" >&2
        return 1
    fi
    # Use `git reset` + `git restore` with `NOT_STATE` instead of
    # `git reset --hard` so the mission's `state/` directory is preserved.
    git reset --quiet "$base" || return 1
    git restore --source="$base" --worktree -- "${NOT_STATE[@]}" || return 1

    # Remove untracked files created during this iteration while preserving any
    # untracked files that existed before it. `core.quotePath=false` makes git
    # print non-ASCII names as-is rather than as quoted octal escapes, which
    # `rm` would not find.
    comm -13 \
        <(printf '%s\n' "$UNTRACKED_BEFORE" | sort) \
        <(git -c core.quotePath=false ls-files --others --exclude-standard -- "${NOT_STATE[@]}" | sort) \
        | while IFS= read -r path; do
            if [[ -n $path ]]; then
                rm -f -- "$path"
            fi
        done
}

CONV=""                   # current conversation id
CONV_TITLE=""
CONV_BASE=$BASELINE       # HEAD when the current conversation started
CONV_TARGET=""            # the target the current conversation started on
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
TARGET=""
REPAIR_BASE=""            # base of the iteration a repair turn continues
REPAIRS=0                 # consecutive in-place repairs
DISCARD_REF=""
rc=0                     # exit status of the last turn
RUN_STATUS=0             # the harness's own exit status; 1 when the run aborts

finish() {
    local total
    total=$(git rev-list --count "$BASELINE..HEAD" 2>/dev/null || echo 0)
    {
        echo "## Finished: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
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

# Bash defers signal traps while a foreground command runs, but interrupts
# `wait` immediately. Running `jp` in the background and waiting on `$TURN_PID`
# lets the trap terminate an in-flight turn on SIGINT/SIGTERM.
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
    # A run that stops on its budget still fails when its last turn did, so a
    # caller running missions in sequence knows not to go on.
    if [[ $now -ge $DEADLINE ]]; then
        echo "$MISSION: wall-clock budget spent" >&2
        if [[ $rc -ne 0 || $GATE_OK -eq 0 ]]; then RUN_STATUS=1; fi
        break
    fi
    if [[ $ITERATIONS -gt 0 && $ITER -ge $ITERATIONS ]]; then
        echo "$MISSION: iteration limit reached" >&2
        if [[ $rc -ne 0 || $GATE_OK -eq 0 ]]; then RUN_STATUS=1; fi
        break
    fi

    # A repair continues the iteration it repairs, so it keeps that iteration's
    # base: the gate checks, and a discard resets, the work of every turn since.
    head_before=${REPAIR_BASE:-$(git rev-parse HEAD)}
    export LOOP_ITERATION
    LOOP_ITERATION=$(printf '%03d' $((ITER + 1)))
    export LOOP_ITERATION_BASE=$head_before

    # Query `next-target` before incrementing ITER so a run that exits when no
    # targets remain does not count an unexecuted iteration. Skip `next-target`
    # on repair turns, which keep the target they repair.
    if [[ $GATE_OK -eq 1 ]]; then
        TARGET=""
    fi
    if [[ -n $NEXT_TARGET && $GATE_OK -eq 1 ]]; then
        set +e
        TARGET=$("$NEXT_TARGET")
        target_rc=$?
        set -e
        if [[ $target_rc -ne 0 ]]; then
            echo "$MISSION: next-target exited $target_rc; stopping" >&2
            RUN_STATUS=1
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

    if [[ -n $CONV ]]; then
        commits_here=$(git rev-list --count "$CONV_BASE..HEAD")
        events_here=$(event_count_for_id "$CONV")
        events_here=${events_here:-0}
        if [[ $ROTATE_ON_TARGET -eq 1 && $TARGET != "$CONV_TARGET" ]]; then
            ROTATE_REASON="new target"
            CONV=""
        elif [[ $commits_here -ge $ROTATE_COMMITS ]]; then
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

    # Assemble the prompt. A new conversation starts with `mission_prompt` (plus
    # `handover_prompt` after a rotation). If the previous turn failed the gate,
    # append `repair_prompt` even on a newly rotated conversation.
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
    if [[ -n $TARGET && $mode != repair ]]; then
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
        CONV_TARGET=$TARGET
        echo "$MISSION: [$seq] new conversation ($ROTATE_REASON): $CONV_TITLE" >&2
        jp_target=(--new --local --title "$CONV_TITLE")
        # Arguments such as `--attach` become part of the conversation's config,
        # so they are passed once, when the conversation starts.
        if declare -F conversation_args >/dev/null; then
            while IFS= read -r arg; do
                jp_target+=("$arg")
            done < <(conversation_args "$TARGET")
        fi
    else
        jp_target=(--id "$CONV")
    fi

    for file in ${SNAPSHOT_FILES[@]+"${SNAPSHOT_FILES[@]}"}; do
        cp "$STATE/$file" "$RUN_SNAPS/$seq-$file" 2>/dev/null || true
    done
    if [[ $DISCARD_ON_GATE_FAILURE -eq 1 && -z $REPAIR_BASE ]]; then
        UNTRACKED_BEFORE=$(git -c core.quotePath=false ls-files --others --exclude-standard -- "${NOT_STATE[@]}")
    fi

    echo "$MISSION: [$seq] $mode turn -> $log" >&2

    # Pass `-E` (`--no-edit`) explicitly. When stdin is piped and neither
    # `--edit` nor `--no-edit` is given, `jp query` opens `$EDITOR` even under
    # `--no-interactive`, which hangs when stdout is redirected to a log file.
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

    # Rate limits and transient stream errors are not assistant failures; do not
    # count them toward FAILURES or conversation rotation.
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

    head_after=$(git rev-parse HEAD)
    if [[ $head_before == "$head_after" ]]; then
        commit_count=0
        commit_lines="  (none)"
    else
        commit_count=$(git rev-list --count "$head_before..$head_after")
        commit_lines=$(git log --no-decorate --format='  - `%h` %s' "$head_before..$head_after")
    fi

    GATE_OK=1
    GATE_OUTPUT=""
    DISCARDED=0
    DISCARD_REF=""
    REPAIRING=0
    gate_text="skipped"
    if [[ $GATE -eq 1 && -n $GATE_BIN ]]; then
        export LOOP_TURN_STATUS=$rc
        set +e
        gate_raw=$("$GATE_BIN" 2>&1)
        gate_rc=$?
        set -e
        printf '%s\n' "$gate_raw" > "$RUN_DIR/iter-$seq.gate.log"
        # A turn cut short by a usage limit or a stall has not tried its repair.
        if [[ $gate_rc -eq 2 && $LIMITED -eq 0 && $TRANSIENT -eq 0 ]]; then
            REPAIRS=$((REPAIRS + 1))
        fi
        if [[ $gate_rc -eq 0 ]]; then
            gate_text="PASS"
        elif [[ $gate_rc -eq 2 && $REPAIRS -le $MAX_REPAIRS ]]; then
            # Exit status 2: the gate keeps the work, and the next turn repairs it
            # in place, whatever DISCARD_ON_GATE_FAILURE says.
            GATE_OK=0
            REPAIRING=1
            gate_text="REPAIR"
            GATE_OUTPUT=$gate_raw
            echo "$MISSION: [$seq] gate kept the work; the next turn repairs it in place" >&2
        elif [[ $DISCARD_ON_GATE_FAILURE -eq 1 ]]; then
            gate_text="FAIL"
            if ! discard_iteration "$head_before"; then
                RUN_STATUS=1
                break
            fi
            DISCARDED=1
            echo "$MISSION: [$seq] gate FAILED; the iteration is discarded${DISCARD_REF:+ and kept at $DISCARD_REF}" >&2
        else
            GATE_OK=0
            gate_text="FAIL"
            GATE_OUTPUT=$gate_raw
            echo "$MISSION: [$seq] gate FAILED; the next turn repairs" >&2
        fi
    fi
    if [[ $REPAIRING -eq 1 ]]; then
        REPAIR_BASE=$head_before
    else
        REPAIR_BASE=""
        REPAIRS=0
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
            if [[ -n $DISCARD_REF ]]; then
                echo "- the discarded work is kept at \`$DISCARD_REF\`"
            fi
        fi
        if [[ $REPAIRING -eq 1 ]]; then
            echo "- the gate kept the changes; the next turn repairs them in place"
        fi
        if [[ $LIMITED -eq 1 ]]; then
            echo "- the turn was interrupted by a usage limit, not by an error in your changes"
        fi
        if [[ $TRANSIENT -eq 1 ]]; then
            echo "- the turn was interrupted by a network stall, not by an error in your changes;" \
                 "continue from where it stopped"
        fi
    )

    {
        echo "## Iteration $seq: $(date -u +%Y-%m-%dT%H:%M:%SZ) ($mode, ${CONV:-unknown})"
        echo ""
        printf '%s\n' "$LAST_FACTS"
        echo ""
    } >> "$RUNLOG"

    echo "$MISSION: [$seq] $commit_count commit(s), gate $gate_text, $dirty file(s) dirty" >&2

    if [[ $LIMITED -eq 1 ]]; then
        if ! wait_for_capacity; then
            RUN_STATUS=1
            break
        fi
        continue
    fi

    if [[ $TRANSIENT -eq 1 ]]; then
        if [[ $TRANSIENTS -ge $MAX_TRANSIENT ]]; then
            echo "$MISSION: $TRANSIENTS stalls in a row; the network is not coming back" >&2
            RUN_STATUS=1
            break
        fi
        sleep "$TRANSIENT_BACKOFF"
        continue
    fi

    # Before aborting after MAX_FAILURES, run a capacity probe in case the turns
    # failed with an unrecognized rate-limit message.
    if [[ $FAILURES -ge $MAX_FAILURES ]]; then
        echo "$MISSION: $MAX_FAILURES consecutive failures; probing for capacity" >&2
        if run_probe; then
            echo "$MISSION: the account is fine, so the failures are real; stopping" >&2
            RUN_STATUS=1
            break
        fi
        if ! wait_for_capacity; then
            RUN_STATUS=1
            break
        fi
        FAILURES=0
    fi
done

finish
exit "$RUN_STATUS"
