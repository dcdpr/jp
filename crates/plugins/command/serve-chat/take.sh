#!/bin/sh
# The server half of `just chat-take`, run in the chat bridge's checkout:
#
#   ssh HOST sh -s -- snapshot DIR < take.sh
#   ssh HOST sh -s -- reset DIR BASE SUM < take.sh
#
# `snapshot` prints the checkout's HEAD, then a binary diff of every uncommitted
# change, untracked files included. `reset` discards those changes, but only if
# HEAD is still BASE and the diff still hashes to SUM, so a change made after
# the snapshot is never thrown away.
#
# Both hold the bridge's lock (`flock` on fd 9, the lock `jp serve chat` takes
# for a turn) for the whole script, so `set -eu` covers every command: a failed
# `git add` or `git diff` ends the script before anything is discarded.
set -eu

command="$1"
cd "$2"

exec 9>>"$(git rev-parse --git-path jp-serve-chat.lock)"
flock 9

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1"
    else
        shasum -a 256 "$1"
    fi
}

case "$command" in
snapshot)
    git rev-parse HEAD
    git add -A
    git diff --cached --binary -M HEAD
    ;;
reset)
    base="$3"
    expected="$4"

    head="$(git rev-parse HEAD)"
    if [ "$head" != "$base" ]; then
        echo "The bridge pulled since the take; its checkout is left alone." >&2
        exit 1
    fi

    diff="$(mktemp)"
    trap 'rm -f "$diff"' EXIT
    git add -A
    git diff --cached --binary -M HEAD >"$diff"
    sum="$(sha256 "$diff")"
    if [ "${sum%% *}" != "$expected" ]; then
        echo "New changes arrived since the take; its checkout is left alone. Take again." >&2
        exit 1
    fi

    git reset --quiet --hard
    git clean --quiet -fd
    ;;
*)
    echo "Unknown command \`$command\`: use \`snapshot\` or \`reset\`." >&2
    exit 2
    ;;
esac
