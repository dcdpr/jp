# see: <https://github.com/cargo-bins/cargo-quickinstall/releases>
bacon_version        := "3.23.0"
binstall_version     := "1.20.0"
deny_version         := "0.19.9"
expand_version       := "1.0.123"
insta_version        := "1.48.0"
jilu_version         := "0.13.2"
llvm_cov_version     := "0.8.7"
nextest_version      := "0.9.137"
shear_version        := "1.12.4"
vet_version          := "0.10.2"

quiet_flag := if env_var_or_default("CI", "") == "true" { "" } else { "--quiet" }

alias r := run
alias i := install
alias c := check
alias t := test

alias bc := build-changelog
alias co := commit
alias st := stage
alias sc := stage-and-commit
alias ib := issue-bug
alias if := issue-feat

[private]
default:
    #!/usr/bin/env sh
    set -eu

    if ! which jq >/dev/null; then
        just --list
        exit 0
    fi

    GROUP="main"

    BOLD_YELLOW="\033[1;93m"
    RESET="\033[0;0m"

    echo "Available recipes:"
    echo -e "    ${BOLD_YELLOW}[${GROUP}]${RESET}"
    just --dump --dump-format=json |
        jq '.recipes | to_entries[] | select(.value.attributes | any(try (.group == "main") catch false)) | "\(.key)~# \(.value.doc // "")"' |
        tr -d '"' |
        sed 's/^/    /g' |
        column -t -s "~"

    echo
    echo "Additional recipes are available. To see all, run:"
    echo "    just --list"

# Run the main binary through `cargo run`.
[group('build')]
[group('main')]
[no-cd]
[positional-arguments]
run *ARGS:
    #!/usr/bin/env sh
    set -eu

    cargo run --package jp_cli -- "$@"

# Install the `jp` binary from your local checkout.
[group('build')]
[group('main')]
install $JP_NO_INSTALL="":
    @just quiet_flag="" _install-jp

[group('jp')]
issue-bug +ARGS="Please create a bug report for the following:\n\n": _install-jp
    jp query --new --local --tmp=1h --cfg=personas/po --hide-reasoning --edit=true {{ARGS}}

# Create a feature request issue.
[group('jp')]
issue-feat +ARGS="Please create a feature request for the following:\n\n": _install-jp
    jp query --new --local --tmp=1h --cfg=personas/po --hide-reasoning --edit=true {{ARGS}}

# Open a commit message in the editor, using Jean-Pierre.
[group('jp')]
[positional-arguments]
commit *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    msg="Give me a commit message"

    args=$(just _shape-args "$msg" "$@")

    jp query --new --local --tmp=1h --cfg=personas/committer $args || exit 1
    git commit --amend

[group('jp')]
[positional-arguments]
stage *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    msg="Find related changes in the git diff and stage ONE set of changes in preparation for a \
    commit using the 'git_stage_patch' tool. Follow your prompt instructions carefully."

    args=$(just _shape-args "$msg" "$@")

    jp query --new --local --tmp=1h --cfg=personas/stager $args

stage-and-commit: _install-jp
    #!/usr/bin/env sh
    set -eu

    out=$(just stage -c style.reasoning.display=hidden)
    just commit "$out - now write me a commit message"

# Generate changelog for the project.
[group('build')]
build-changelog: (_install "jilu@" + jilu_version)
    @jilu

[group('profile')]
[positional-arguments]
profile-heap *ARGS:
    #!/usr/bin/env sh
    set -eu

    cargo run --profile profiling --features dhat -- "$@"

# Ask JP to create a new RFD based on the current conversation context.
[group('jp')]
[positional-arguments]
rfd-this *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    msg="I gave you the RFD skill, use it to codify all that we just discussed and concluded in a feature request RFD."

    args=$(just _shape-args "$msg" "$@")

    jp query --cfg=skill/rfd $args

# Review a GitHub pull request, queueing inline comments to a draft review.
#
# Each comment is added one at a time and prompts you to approve or reject
# it before it is posted. The review remains PENDING (only visible to the
# authenticating user, via `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`) until you
# submit it from the GitHub UI.
[group('jp')]
[positional-arguments]
pr-review NNN *ARGS: _install-jp _install-tools
    #!/usr/bin/env sh
    set -eu

    case "{{NNN}}" in
        ''|*[!0-9]*)
            echo "Invalid PR number '{{NNN}}'. Pass a positive integer." >&2
            exit 1 ;;
    esac

    shift # remove NNN from positional params
    msg="Review GitHub pull request #{{NNN}} in dcdpr/jp. Follow your review \
    workflow: enumerate the PR, read every changed file's diff, cross-reference \
    where useful, then call github_pr_review_add_comment with pull_number=\
    {{NNN}} once for EACH finding. After all comments are queued, write a \
    final markdown overview summarizing your review (counts per category, \
    overall take, mergeability). Do NOT submit the review yourself — leave \
    it as a draft."

    args=$(just _shape-args "$msg" "$@")

    # Tell the reviewer whether the working tree holds this PR's code, so it
    # prefers the local fs_*/git_* tools over the slower github_* ones.
    state=$(just _pr-checkout-state {{NNN}})
    case "$state" in
        "LOCAL "*) args="$args\n\nThe working tree is checked out at this PR's head \
        (${state#LOCAL }) and clean. Prefer the local fs_* and git_* tools over github_* for \
        reading files and history; they are faster and complete." ;;
        "DIRTY "*) args="$args\n\nThe working tree is checked out at this PR's head \
        (${state#DIRTY }) but has uncommitted local modifications. You may use the local fs_* \
        and git_* tools; first call git_status to see which files differ (including untracked), \
        then confirm none of those changes affect what you're reviewing before trusting a local \
        read." ;;
    esac

    title="pr-review:{{NNN}}"

    existing=""
    out=$(just _resolve-conversation "$title")
    case "$out" in
        "CONTINUE "*) existing="${out#CONTINUE }" ;;
        "ARCHIVE "*)  jp conversation archive "${out#ARCHIVE }" || true ;;
        NEW)          ;;
        QUIT)         exit 0 ;;
        *)            echo "Unexpected from _resolve-conversation: $out" >&2; exit 1 ;;
    esac

    if [ -n "$existing" ]; then
        printf "Resuming review on PR #{{NNN}} (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/pr-reviewer \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews?include_outdated=true" \
            $args
    else
        printf "Reviewing PR #{{NNN}}\n\n" >&2
        jp query --new --title "$title" --cfg=personas/pr-reviewer \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews?include_outdated=true" \
            $args
    fi
    printf "\nDraft review staged on https://github.com/dcdpr/jp/pull/{{NNN}}/files — open the page and submit it when ready.\n" >&2

# Triage a GitHub pull request's reviews. Reads the PR's diff and every
# review/comment from the attached `gh:pull/N/reviews` resource, then
# produces a per-item verdict (accept / amend / dismiss / defer) with
# reasoning grounded in the code.
#
# The triager does not edit code in this turn. To act on the verdicts,
# follow up in the same conversation with a dev persona, e.g.
# `jp q --id <id> --cfg=personas/dev "implement the proposed changes"`.
#
# When a `pr-triage:NNN` conversation already exists, prompts whether to
# continue, archive-and-start-fresh, or quit. Resuming preserves the
# triager's codebase context across review cycles; starting fresh is
# useful when the conversation has gone off the rails.
[group('jp')]
[positional-arguments]
pr-triage NNN *ARGS: _install-jp _install-tools
    #!/usr/bin/env sh
    set -eu

    case "{{NNN}}" in
        ''|*[!0-9]*)
            echo "Invalid PR number '{{NNN}}'. Pass a positive integer." >&2
            exit 1 ;;
    esac

    shift # remove NNN from positional params
    msg="Triage the reviews on GitHub pull request #{{NNN}} in dcdpr/jp. \
    For each review comment, write one numbered item containing: the \
    comment's \`id=<n>\` from the attached reviews, a short quote of \
    the reviewer's point, a verdict (\`Accept\`, \`Amend\`, \`Dismiss\`, \
    or \`Defer\`) with reasoning grounded in the actual code, and (when \
    accepting or amending) the concrete change you would make. Do NOT \
    edit any files and do NOT post replies yet — output the triage as \
    plain markdown only."

    args=$(just _shape-args "$msg" "$@")

    # Tell the triager whether the working tree holds this PR's code, so it
    # prefers the local fs_*/git_* tools over the slower github_* ones.
    state=$(just _pr-checkout-state {{NNN}})
    case "$state" in
        "LOCAL "*) args="$args\n\nThe working tree is checked out at this PR's head \
        (${state#LOCAL }) and clean. Prefer the local fs_* and git_* tools over github_* for \
        reading files and history; they are faster and complete." ;;
        "DIRTY "*) args="$args\n\nThe working tree is checked out at this PR's head \
        (${state#DIRTY }) but has uncommitted local modifications. You may use the local fs_* \
        and git_* tools; first call git_status to see which files differ (including untracked), \
        then confirm none of those changes affect what you're triaging before trusting a local \
        read." ;;
    esac

    # On the PR's branch, the implementation conversation is probably in this
    # session. Offer to triage there — picking from session-bound conversations
    # — instead of a fresh, context-free one. `jp query` with no target then
    # runs in whatever conversation the picker activates.
    case "$state" in
        "LOCAL "*|"DIRTY "*)
            if [ -r /dev/tty ] && [ -w /dev/tty ]; then
                printf "You're on PR #{{NNN}}'s branch.\n" > /dev/tty
                printf "  Triage in a [p]icked conversation / [n]ew triage conversation / [q]uit: " > /dev/tty
                IFS= read -r ans < /dev/tty
            else
                ans=n
            fi
            case "$ans" in
                p|P)
                    jp conversation use '?session'
                    jp query --cfg=personas/pr-triager \
                        --attach "gh:pull/{{NNN}}/diff" \
                        --attach "gh:pull/{{NNN}}/reviews?include_outdated=true" \
                        $args
                    exit 0 ;;
                q|Q) exit 0 ;;
                *) ;;
            esac ;;
    esac

    title="pr-triage:{{NNN}}"

    existing=""
    out=$(just _resolve-conversation "$title")
    case "$out" in
        "CONTINUE "*) existing="${out#CONTINUE }" ;;
        "ARCHIVE "*)  jp conversation archive "${out#ARCHIVE }" || true ;;
        NEW)          ;;
        QUIT)         exit 0 ;;
        *)            echo "Unexpected from _resolve-conversation: $out" >&2; exit 1 ;;
    esac

    if [ -n "$existing" ]; then
        printf "Resuming triage on PR #{{NNN}} (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/pr-triager \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews?include_outdated=true" \
            $args
    else
        printf "Triaging PR #{{NNN}}\n\n" >&2
        jp query --new --title "$title" --cfg=personas/pr-triager \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews?include_outdated=true" \
            $args
    fi

# Review the current diff with revdiff and send the annotations back to the
# active jp conversation. ARGS before a `--` are forwarded to revdiff (see
# `revdiff --help`); ARGS after a `--` are forwarded to the `jp query` that
# receives the annotations:
#
#   just review                     # uncommitted changes (default)
#   just review HEAD~3              # last 3 commits
#   just review main                # current branch vs main
#   just review --staged            # staged changes
#   just review --staged -- --edit  # staged changes; edit the jp prompt
#
# Exits silently if revdiff produces no annotations (e.g. you quit with `q`
# without leaving notes, or `Q` to discard). The matching `git diff` is
# attached so the assistant can resolve line-anchored notes against the same
# context revdiff showed you. Sends to the active conversation; use
# `jp conversation use <ID>` first to target a specific one.
[group('jp')]
[positional-arguments]
review *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    if ! command -v revdiff >/dev/null 2>&1; then
        echo "revdiff not found on PATH." >&2
        echo "Install via 'brew install umputun/apps/revdiff' or see" >&2
        echo "https://github.com/umputun/revdiff/releases for binaries." >&2
        exit 1
    fi

    # Split ARGS at the first `--`: everything before it is for revdiff,
    # everything after it is for the `jp query` below. Rotating the
    # positional params leaves revdiff's args quoted in "$@" (so e.g.
    # `--include '*.rs'` survives), while jp args collect into a string
    # expanded unquoted, like the other recipes.
    jp_args=""
    found_sep=false
    n=$#
    while [ "$n" -gt 0 ]; do
        n=$((n - 1))
        arg="$1"
        shift
        if [ "$found_sep" = true ]; then
            jp_args="${jp_args:+$jp_args }$arg"
        elif [ "$arg" = "--" ]; then
            found_sep=true
        else
            set -- "$@" "$arg"
        fi
    done

    set +e
    annotations=$(mktemp)
    revdiff --output="$annotations" --vim-motion --word-diff --cross-file-hunks "$@"
    rev_exit=$?
    annotations=$(cat "$annotations")
    set -e
    if [ "$rev_exit" -ne 0 ]; then
        exit "$rev_exit"
    fi

    if [ -z "$annotations" ]; then
        echo "No review annotations recorded; nothing to send." >&2
        exit 0
    fi

    # Build a cmd:// URL mirroring revdiff's diff scope so the assistant
    # sees the same diff revdiff showed (line numbers in the annotations
    # are anchored to that exact diff). Positional args (refs, base..feat)
    # forward as-is; --staged/--cached are git-diff-compatible. Other
    # flags are revdiff-specific (--theme, --include, -A, ...) and would
    # make `git diff` fail, so they're dropped.
    diff_attach="cmd://git?arg=diff"
    for arg in "$@"; do
        case "$arg" in
            --staged|--cached)
                encoded=$(printf '%s' "$arg" | jq -sRr @uri)
                diff_attach="${diff_attach}&arg=${encoded}"
                ;;
            -*) ;;
            *)
                encoded=$(printf '%s' "$arg" | jq -sRr @uri)
                diff_attach="${diff_attach}&arg=${encoded}"
                ;;
        esac
    done

    preamble="Below are my review notes from \`revdiff\` on the diff you just produced. \
    Each entry header is \`## path:line[-line] (+|-)\` (anchored to a specific position) \
    or \`## path (file-level)\` (whole file). The matching \`git diff\` is attached so you \
    can resolve those positions. Address each note with targeted edits only — no wholesale \
    re-generation, no unrelated cleanup."

    printf '%s\n\n%s\n' "$preamble" "$annotations" \
        | jp query --attach "$diff_attach" $jp_args

# Review an RFD. Accepts a permanent number (41, 041) or a draft ID (D01).
#
# When an `rfd-review:<id>` conversation already exists, prompts whether to
# continue, archive-and-start-fresh, or quit. In continuation mode, also
# attaches the latest turn from the matching `rfd-triage:<id>` conversation
# (if one exists) so the reviewer can engage with the triager's response
# and the author's notes from the previous cycle.
#
# Looks up Bear notes tagged `rfd/<id>/review` and attaches them. If none
# match, prompts whether to continue without notes, edit the prompt inline,
# or quit.
[group('rfd')]
[positional-arguments]
rfd-review NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    shift # remove NNN from positional params
    msg="Please review the attached RFD. Review the RFD in isolation, \
    including its explicit dependencies, or any implicit dependencies, but \
    keep in mind that Draft RFDs are still in the design phase, and Discussion \
    RFDs are aspirational, but not necessarily final, so any inconsistencies \
    against those should be noted, but not blockers."

    out=$(just _rfd-resolve "{{NNN}}") || exit 1
    rfd_id="${out%% *}"
    file="${out#* }"

    title="rfd-review:${rfd_id}"

    existing=""
    out=$(just _resolve-conversation "$title")
    case "$out" in
        "CONTINUE "*) existing="${out#CONTINUE }" ;;
        "ARCHIVE "*)  jp conversation archive "${out#ARCHIVE }" || true ;;
        NEW)          ;;
        QUIT)         exit 0 ;;
        *)            echo "Unexpected from _resolve-conversation: $out" >&2; exit 1 ;;
    esac

    # In continuation mode, fold in the latest triage turn so the reviewer
    # can engage with the triager's response and the author's notes from
    # the prior cycle.
    triage_attach=""
    if [ -n "$existing" ]; then
        triage_id=$(jp -F json conversation ls 2>/dev/null \
            | jq -r --arg t "rfd-triage:${rfd_id}" \
                'first(.[] | select(.Title == $t) | .ID) // empty' \
            2>/dev/null || true)
        if [ -n "$triage_id" ]; then
            triage_attach="--attach jp://${triage_id}?select=u,a:-1"
            printf "Attaching last triage turn from %s\n" "$triage_id" >&2
        fi
    fi

    note_attach=""
    extra_edit=""
    note_out=$(just _bear-note "rfd/${rfd_id}/review")
    case "$note_out" in
        "FOUND "*) note_attach="--attach ${note_out#FOUND }"
                   printf "Attaching Bear notes tagged 'rfd/%s/review'\n" "$rfd_id" >&2 ;;
        EDIT)      extra_edit="--edit" ;;
        CONTINUE)  ;;
        QUIT)      exit 0 ;;
        *)         echo "Unexpected from _bear-note: $note_out" >&2; exit 1 ;;
    esac

    args=$(just _shape-args "$msg" "$@")

    if [ -n "$existing" ]; then
        printf "Resuming review on $file (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/rfd-reviewer \
            --attach "$file" \
            $triage_attach \
            $note_attach \
            $extra_edit \
            $args
    else
        printf "Reviewing $file\n\n" >&2
        jp query --new --title "$title" --cfg=personas/rfd-reviewer \
            --attach "$file" \
            $note_attach \
            $extra_edit \
            $args
    fi

# Triage feedback on an RFD from its review conversation.
#
# Looks up the matching `rfd-review:<id>` conversation by title and attaches
# its latest user/assistant turn (the reviewer's verdicts plus any author
# notes from that round).
#
# When the current session has an active conversation (typically the agentic
# session that drafted the RFD), offers to triage there so the assistant keeps
# that accumulated context. Declining falls back to a titled `rfd-triage:<id>`
# conversation, prompting to continue, archive-and-start-fresh, or quit when one
# already exists.
#
# Looks up Bear notes tagged `rfd/<id>/triage` and attaches them. If none
# match, prompts whether to continue without notes, edit the prompt inline,
# or quit.
#
# Accepts a permanent number (41, 041) or a draft ID (D01).
[group('rfd')]
[positional-arguments]
rfd-triage NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    shift # remove NNN from positional params
    msg="I received feedback on the RFD. Read the attached reviewer response \
    carefully, then triage it item by item. Ground each point against the code \
    and related RFDs. Do not assume the feedback is correct. For each item \
    give a verdict (accept / amend / dismiss / defer) with reasoning, and for \
    accepted or amended items describe the concrete change you would make to \
    the RFD. Do NOT edit the RFD yet; give your opinion first."

    out=$(just _rfd-resolve "{{NNN}}") || exit 1
    rfd_id="${out%% *}"
    file="${out#* }"

    # The triage step needs the sibling review conversation to exist.
    review_id=$(jp -F json conversation ls 2>/dev/null \
        | jq -r --arg t "rfd-review:${rfd_id}" \
            'first(.[] | select(.Title == $t) | .ID) // empty' \
        2>/dev/null || true)
    if [ -z "$review_id" ]; then
        echo "No 'rfd-review:${rfd_id}' conversation found. Run 'just rfd-review ${rfd_id}' first." >&2
        exit 1
    fi

    # Prefer the session's active conversation, when there is one. An RFD is
    # usually drafted in an agentic session that already holds the relevant
    # context (the RFD text, related code, prior discussion); triaging the
    # review there keeps that context instead of starting fresh. Falls back to
    # the titled `rfd-triage:<id>` conversation when there's no active
    # conversation or the offer is declined.
    target=""
    active_id=$(jp -F json conversation ls +s 2>/dev/null \
        | jq -r '.[-1].ID // empty' 2>/dev/null || true)
    if [ -n "$active_id" ]; then
        if [ -r /dev/tty ] && [ -w /dev/tty ]; then
            printf "This session's active conversation is %s.\n" "$active_id" > /dev/tty
            printf "  Triage in the [a]ctive conversation / [n]ew triage conversation / [q]uit: " > /dev/tty
            IFS= read -r ans < /dev/tty
        else
            ans=n
        fi
        case "$ans" in
            a|A) target="--id $active_id" ;;
            q|Q) exit 0 ;;
            *)   ;;
        esac
    fi

    if [ -z "$target" ]; then
        title="rfd-triage:${rfd_id}"
        out=$(just _resolve-conversation "$title")
        case "$out" in
            "CONTINUE "*) target="--id ${out#CONTINUE }" ;;
            "ARCHIVE "*)  jp conversation archive "${out#ARCHIVE }" || true
                          target="--new --title $title" ;;
            NEW)          target="--new --title $title" ;;
            QUIT)         exit 0 ;;
            *)            echo "Unexpected from _resolve-conversation: $out" >&2; exit 1 ;;
        esac
    fi

    note_attach=""
    extra_edit=""
    note_out=$(just _bear-note "rfd/${rfd_id}/triage")
    case "$note_out" in
        "FOUND "*) note_attach="--attach ${note_out#FOUND }"
                   printf "Attaching Bear notes tagged 'rfd/%s/triage'\n" "$rfd_id" >&2 ;;
        EDIT)      extra_edit="--edit" ;;
        CONTINUE)  ;;
        QUIT)      exit 0 ;;
        *)         echo "Unexpected from _bear-note: $note_out" >&2; exit 1 ;;
    esac

    args=$(just _shape-args "$msg" "$@")

    printf "Triaging feedback on $file\n\n" >&2
    jp query $target --cfg=personas/rfd-triager \
        --attach "file://$file" \
        --attach "jp://${review_id}?select=u,a:-1" \
        $note_attach \
        $extra_edit \
        $args

# Implement an Accepted RFD. Accepts a permanent number (41, 041).
#
# The implementor reads the RFD as the contract: minor inconsistencies with
# current code are reconciled unilaterally and noted in the report; major
# conflicts pause for user input. Begins at phase 1 of the Implementation Plan
# unless the user explicitly requests a different phase via positional args.
#
# Refuses anything other than Accepted or Implemented — Implemented is allowed
# so that follow-up runs can fix implementation drift on already-shipped RFDs.
#
# When an `rfd-implement:<id>` conversation already exists, prompts whether
# to continue, archive-and-start-fresh, or quit. Looks up Bear notes tagged
# `rfd/<id>/implement` and attaches them.
[group('rfd')]
[positional-arguments]
rfd-implement NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    shift # remove NNN from positional params
    msg="Implement the attached RFD. Read it fully first, then locate the \
    Implementation Plan and begin with phase 1 (or the phase the user has \
    requested in additional args). The RFD is Accepted; treat it as the \
    contract. For minor inconsistencies with the current code, make a minimal \
    reconciliation and list it in the final report. For major conflicts (a \
    section's assumptions no longer hold, a data shape or API the RFD relies \
    on is gone, a newer RFD has changed the boundary), stop and surface the \
    problem instead of resolving it yourself. End the turn with the final \
    report exactly as your instructions describe."

    out=$(just _rfd-resolve "{{NNN}}") || exit 1
    rfd_id="${out%% *}"
    file="${out#* }"

    # Status gate: only Accepted or Implemented RFDs are valid targets.
    # Drafts pass `_rfd-resolve` and get a meaningful "is 'Draft'" error
    # here instead of "file not found".
    status=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$file" | head -1)
    case "$status" in
        Accepted|Implemented) ;;
        *)
            echo "Cannot implement: $(basename "$file") is '${status}'." >&2
            echo "Only Accepted or Implemented RFDs may be implemented." >&2
            exit 1 ;;
    esac

    title="rfd-implement:${rfd_id}"

    existing=""
    out=$(just _resolve-conversation "$title")
    case "$out" in
        "CONTINUE "*) existing="${out#CONTINUE }" ;;
        "ARCHIVE "*)  jp conversation archive "${out#ARCHIVE }" || true ;;
        NEW)          ;;
        QUIT)         exit 0 ;;
        *)            echo "Unexpected from _resolve-conversation: $out" >&2; exit 1 ;;
    esac

    note_attach=""
    extra_edit=""
    note_out=$(just _bear-note "rfd/${rfd_id}/implement")
    case "$note_out" in
        "FOUND "*) note_attach="--attach ${note_out#FOUND }"
                   printf "Attaching Bear notes tagged 'rfd/%s/implement'\n" "$rfd_id" >&2 ;;
        EDIT)      extra_edit="--edit" ;;
        CONTINUE)  ;;
        QUIT)      exit 0 ;;
        *)         echo "Unexpected from _bear-note: $note_out" >&2; exit 1 ;;
    esac

    args=$(just _shape-args "$msg" "$@")

    if [ -n "$existing" ]; then
        printf "Resuming implementation of $file (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/rfd-implementor \
            --attach "$file" \
            $note_attach \
            $extra_edit \
            $args
    else
        printf "Implementing $file\n\n" >&2
        jp query --new --title "$title" --cfg=personas/rfd-implementor \
            --attach "$file" \
            $note_attach \
            $extra_edit \
            $args
    fi

# Create a new RFD draft. CATEGORY is 'design', 'decision', 'guide', or 'process'.
# Drafts are created as docs/rfd/drafts/DNN-slug.md; a permanent number is assigned
# and the file is moved up to docs/rfd/ at Discussion.
[group('rfd')]
rfd-draft CATEGORY +TITLE:
    #!/usr/bin/env sh
    set -eu

    category="{{CATEGORY}}"

    # Validate the category and resolve the template.
    case "$category" in
        design)   template="design"  ;;
        decision) template="decision" ;;
        guide)    template="guide"   ;;
        process)  template="guide"   ;;
        *) echo "Unknown category '$category'. Use 'design', 'decision', 'guide', or 'process'." >&2; exit 1 ;;
    esac

    # Find the first available draft number (D01–D99).
    draft_id=$(just _rfd-next-draft-slot) || exit 1

    # Resolve the author from git config, falling back to $USER.
    git_name=$(git config user.name 2>/dev/null || true)
    git_email=$(git config user.email 2>/dev/null || true)
    if [ -n "$git_name" ] && [ -n "$git_email" ]; then
        author="${git_name} <${git_email}>"
    elif [ -n "$git_name" ]; then
        author="$git_name"
    else
        author="${USER:-unknown}"
    fi

    # Capitalize the category for the metadata header.
    cap_category=$(echo "$category" | awk '{print toupper(substr($0,1,1)) substr($0,2)}')

    # Build the filename slug from the title.
    slug=$(echo "{{TITLE}}" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9_-')
    file="docs/rfd/drafts/${draft_id}-${slug}.md"
    mkdir -p "$(dirname "$file")"

    # Copy the template and fill in metadata.
    sed \
        -e "s/RFD NNN: TITLE/RFD ${draft_id}: {{TITLE}}/" \
        -e "s/^- \*\*Category\*\*: .*/- **Category**: ${cap_category}/" \
        -e "s/AUTHOR/${author}/" \
        -e "s/DATE/$(date +%Y-%m-%d)/" \
        "docs/rfd/000-${template}-template.md" > "$file"

    echo "Created $file"

# Supersede RFD NNN with RFD MMM, updating both documents.
[group('rfd')]
rfd-supersede NNN MMM:
    #!/usr/bin/env sh
    set -eu

    old_n=$(echo "{{NNN}}" | sed 's/^0*//')
    old_num=$(printf "%03d" "${old_n:-0}")
    new_n=$(echo "{{MMM}}" | sed 's/^0*//')
    new_num=$(printf "%03d" "${new_n:-0}")
    old_file=$(ls docs/rfd/${old_num}-*.md 2>/dev/null | head -1)
    new_file=$(ls docs/rfd/${new_num}-*.md 2>/dev/null | head -1)
    if [ -z "$old_file" ]; then
        echo "No RFD found with number ${old_num}." >&2; exit 1
    fi
    if [ -z "$new_file" ]; then
        echo "No RFD found with number ${new_num}." >&2; exit 1
    fi

    # Validate the old RFD can be superseded.
    current=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$old_file" | head -1)
    case "$current" in
        Accepted|Implemented) ;;
        *)
            echo "Cannot supersede from '${current}'." >&2
            echo "Only Accepted or Implemented RFDs can be superseded." >&2
            exit 1 ;;
    esac

    # Resolve basenames for relative markdown links.
    new_basename=$(basename "$new_file")
    old_basename=$(basename "$old_file")

    # Update old RFD: status -> Superseded, add/update "Superseded by" link.
    awk -v new="RFD ${new_num}" -v new_file="${new_basename}" '
        /^- \*\*Status\*\*:/ { print "- **Status**: Superseded"; next }
        /^- \*\*Superseded by\*\*:/ { next }
        /^- \*\*Date\*\*:/ { print; print "- **Superseded by**: [" new "](" new_file ")"; next }
        { print }
    ' "$old_file" > "${old_file}.tmp"
    mv "${old_file}.tmp" "$old_file"

    # Update new RFD: add/update "Supersedes" link.
    awk -v old="RFD ${old_num}" -v old_file="${old_basename}" '
        /^- \*\*Supersedes\*\*:/ { next }
        /^- \*\*Date\*\*:/ { print; print "- **Supersedes**: [" old "](" old_file ")"; next }
        { print }
    ' "$new_file" > "${new_file}.tmp"
    mv "${new_file}.tmp" "$new_file"

    echo "${old_file}: Superseded by RFD ${new_num}"
    echo "${new_file}: Supersedes RFD ${old_num}"

    # Remind the user to close the old tracking issue if one exists.
    old_tracking=$(sed -n 's/^- \*\*Tracking Issue\*\*: \[#\([0-9]*\)\].*/\1/p' "$old_file" | head -1)
    if [ -n "$old_tracking" ]; then
        echo "Remember to close the superseded tracking issue: https://github.com/dcdpr/jp/issues/${old_tracking}"
    fi

# Record that RFD MMM extends RFD NNN, updating both documents.
#
# Both NNN and MMM may be permanent numbers (e.g. 042) or draft IDs (e.g. D05).
# Bidirectional metadata is maintained per the draft policy: a published RFD
# never receives a `Extended by: RFD D«nn»` back-link from a draft (would
# violate the no-link-from-published-to-draft rule).
[group('rfd')]
rfd-extend NNN MMM:
    #!/usr/bin/env sh
    set -eu

    # Validate that the extended RFD (NNN, the older) is not Abandoned or
    # Superseded — extending a withdrawn or replaced design is almost certainly
    # a mistake.
    nnn_file=$(ls docs/rfd/{{NNN}}-*.md docs/rfd/drafts/{{NNN}}-*.md 2>/dev/null | head -1)
    if [ -z "$nnn_file" ]; then
        # `_rfd-link` will produce a clearer error; let it handle the
        # not-found case.
        :
    else
        nnn_status=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$nnn_file" | head -1)
        case "$nnn_status" in
            Draft|Discussion|Accepted|Implemented) ;;
            *)
                echo "Cannot extend RFD {{NNN}} (status: '${nnn_status}')." >&2
                echo "Abandoned and Superseded RFDs cannot be extended." >&2
                exit 1 ;;
        esac
    fi

    # Delegate to the shared bidirectional-link helper.
    # rfd-extend NNN MMM means "MMM extends NNN":
    #   MMM (source) gets `Extends: NNN`.
    #   NNN (target) gets `Extended by: MMM` (per the draft-aware matrix).
    just _rfd-link "{{MMM}}" "{{NNN}}" "Extends" "Extended by"

# Record that RFD NNN requires RFD MMM, updating both documents.
#
# Both NNN and MMM may be permanent numbers (e.g. 042) or draft IDs (e.g. D05).
# Bidirectional metadata is maintained per the draft policy: a published RFD
# never receives a `Required by: RFD D«nn»` back-link from a draft.
#
# `Requires` participates in the promotion gate enforced by `rfd-promote`:
# Discussion → Accepted requires every dependency to be Accepted, Implemented
# or Superseded; Accepted → Implemented requires every dependency to be
# Implemented or Superseded.
[group('rfd')]
rfd-require NNN MMM:
    #!/usr/bin/env sh
    set -eu

    # Delegate to the shared bidirectional-link helper.
    # rfd-require NNN MMM means "NNN requires MMM":
    #   NNN (source) gets `Requires: MMM`.
    #   MMM (target) gets `Required by: NNN` (per the draft-aware matrix).
    just _rfd-link "{{NNN}}" "{{MMM}}" "Requires" "Required by"

# Internal: write a bidirectional relationship between two RFDs.
#
# SOURCE gets `FORWARD: TARGET` in its metadata.
# TARGET gets `INVERSE: SOURCE` — except when SOURCE is a draft and TARGET is a
# non-draft (rule 2 of the draft-aware matrix: published RFDs never link to
# drafts).
#
# Refused: SOURCE non-draft + TARGET draft (would create a draft link from a
# published RFD).
#
# Cycles in the FORWARD field are detected by walking TARGET's transitive
# FORWARD chain and refusing if SOURCE appears anywhere in it.
[no-exit-message]
[private]
_rfd-link SOURCE TARGET FORWARD INVERSE:
    #!/usr/bin/env sh
    set -eu

    # --- Helpers ---

    # Resolve an RFD id ("D05" or "42" or "042") to its file path.
    resolve_file() {
        if echo "$1" | grep -qE '^D[0-9]+$'; then
            ls "docs/rfd/drafts/$1-"*.md 2>/dev/null | head -1
        else
            n=$(echo "$1" | sed 's/^0*//')
            num=$(printf "%03d" "${n:-0}")
            ls "docs/rfd/${num}-"*.md 2>/dev/null | head -1
        fi
    }

    # Canonicalize an id to display form ("D05" stays "D05"; "42"/"042" become "042").
    display_id() {
        if echo "$1" | grep -qE '^D[0-9]+$'; then
            echo "$1"
        else
            n=$(echo "$1" | sed 's/^0*//')
            printf "%03d\n" "${n:-0}"
        fi
    }

    is_draft() {
        echo "$1" | grep -qE '^D[0-9]+$'
    }

    # Compute relative path from $1 (a file) to $2 (a file).
    relative_link() {
        from_dir=$(dirname "$1")
        to_dir=$(dirname "$2")
        to_base=$(basename "$2")
        if [ "$from_dir" = "$to_dir" ]; then
            echo "$to_base"
        elif [ "$from_dir" = "docs/rfd/drafts" ] && [ "$to_dir" = "docs/rfd" ]; then
            echo "../$to_base"
        elif [ "$from_dir" = "docs/rfd" ] && [ "$to_dir" = "docs/rfd/drafts" ]; then
            echo "drafts/$to_base"
        else
            echo "$to_base"
        fi
    }

    # Add a `- **FIELD**: LINK` entry to FILE for an entry referring to ID.
    # Skips if the entry is already present (idempotent).
    # Returns 0 if added, 1 if skipped.
    #
    # All operations are scoped to the metadata header (lines before the first
    # `## ` heading). RFDs may include metadata-shaped examples inside code
    # blocks (RFD 001 in particular), and we must not read or write against
    # those.
    add_link() {
        f="$1"; field="$2"; link="$3"; id="$4"

        first_heading=$(grep -n '^## ' "$f" | head -1 | cut -d: -f1)
        header_end="${first_heading:-9999}"

        existing=$(head -n "$header_end" "$f" | sed -n "s/^- \\*\\*${field}\\*\\*: //p" | head -1)
        if echo "$existing" | grep -qE "RFD ${id}([^0-9]|\$)"; then
            return 1
        fi

        if [ -n "$existing" ]; then
            # Append to the existing header line, scoped to the header range.
            sed "1,${header_end}s|^- \\*\\*${field}\\*\\*: .*|&, ${link}|" "$f" > "$f.tmp"
            mv "$f.tmp" "$f"
        else
            last_meta=$(head -n "$header_end" "$f" | grep -n '^- \*\*' | tail -1 | cut -d: -f1)
            awk -v ln="$last_meta" -v entry="- **${field}**: ${link}" '
                NR == ln { print; print entry; next }
                { print }
            ' "$f" > "$f.tmp"
            mv "$f.tmp" "$f"
        fi
        return 0
    }

    # --- Resolve source and target ---

    src_id=$(display_id "{{SOURCE}}")
    tgt_id=$(display_id "{{TARGET}}")
    src_file=$(resolve_file "{{SOURCE}}")
    tgt_file=$(resolve_file "{{TARGET}}")

    if [ -z "$src_file" ]; then
        echo "RFD not found: {{SOURCE}}" >&2; exit 1
    fi
    if [ -z "$tgt_file" ]; then
        echo "RFD not found: {{TARGET}}" >&2; exit 1
    fi

    # --- Refuse non-draft source → draft target (rule 2 of the matrix) ---

    if ! is_draft "$src_id" && is_draft "$tgt_id"; then
        echo "Refused: published RFD ${src_id} cannot link to draft RFD ${tgt_id}." >&2
        echo "Promote ${tgt_id} first, or move the relationship to a draft." >&2
        exit 1
    fi

    # --- Refuse duplicate across `Extends` and `Requires` ---
    #
    # Extension implies dependency, so the same target must not appear under
    # both fields. Decide the "other" field to inspect from FORWARD.
    case "{{FORWARD}}" in
        Requires) other_forward="Extends" ;;
        Extends)  other_forward="Requires" ;;
        *)        other_forward="" ;;
    esac

    if [ -n "$other_forward" ]; then
        other_line=$(sed -n "s/^- \\*\\*${other_forward}\\*\\*: //p" "$src_file" | head -1)
        if echo "$other_line" | grep -qE "RFD ${tgt_id}([^0-9]|\$)"; then
            echo "Refused: RFD ${src_id} already lists RFD ${tgt_id} under '${other_forward}'." >&2
            echo "Extension implies dependency; don't list the same target under both. Drop one entry first." >&2
            exit 1
        fi
    fi

    # --- Cycle detection: walk TARGET's transitive `Requires`+`Extends` chain ---
    #
    # The two relationships are unified for gating and cycle purposes
    # (extension is a kind of dependency), so the cycle walk traverses both
    # fields as a single edge set.

    visited=""
    frontier="$tgt_id"
    while [ -n "$frontier" ]; do
        next_frontier=""
        for cur in $frontier; do
            case " $visited " in *" $cur "*) continue ;; esac
            visited="$visited $cur"

            if [ "$cur" = "$src_id" ]; then
                echo "Refused: cycle detected. Adding RFD ${src_id} → RFD ${tgt_id} on '{{FORWARD}}' would close a loop in the Requires/Extends graph." >&2
                exit 1
            fi

            cur_file=$(resolve_file "$cur")
            [ -z "$cur_file" ] && continue

            for cyc_field in Requires Extends; do
                line=$(sed -n "s/^- \\*\\*${cyc_field}\\*\\*: //p" "$cur_file" | head -1)
                ids=$(echo "$line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}')
                for id in $ids; do
                    next_frontier="$next_frontier $id"
                done
            done
        done
        frontier="$next_frontier"
    done

    # --- Compute display links and write metadata ---

    fwd_link="[RFD ${tgt_id}]($(relative_link "$src_file" "$tgt_file"))"
    inv_link="[RFD ${src_id}]($(relative_link "$tgt_file" "$src_file"))"

    # Write FORWARD on source.
    if add_link "$src_file" "{{FORWARD}}" "$fwd_link" "$tgt_id"; then
        echo "${src_file}: {{FORWARD}}: RFD ${tgt_id}"
    else
        echo "${src_file}: already lists RFD ${tgt_id} under {{FORWARD}}"
    fi

    # Write INVERSE on target unless rule 3 of the matrix (draft → non-draft).
    if is_draft "$src_id" && ! is_draft "$tgt_id"; then
        echo "${tgt_file}: skipped {{INVERSE}}: RFD ${src_id} (suppressed: draft → non-draft)"
    else
        if add_link "$tgt_file" "{{INVERSE}}" "$inv_link" "$src_id"; then
            echo "${tgt_file}: {{INVERSE}}: RFD ${src_id}"
        else
            echo "${tgt_file}: already lists RFD ${src_id} under {{INVERSE}}"
        fi
    fi

# Advance an RFD's status: Draft -> Discussion -> Accepted -> Implemented.
#
# For drafts (DNN-prefixed files), assigns the next available permanent number
# and renames the file. When promoting to Accepted, offers to create a GitHub
# tracking issue via `jp` (prompting on TTY, defaulting to yes in
# non-interactive runs) and injects the link into the metadata.
#
# Accepts: a permanent number (41, 041) or a draft ID (D01).
[group('rfd')]
rfd-promote NNN: _install-jp _install-comfort
    #!/usr/bin/env sh
    set -eu

    out=$(just _rfd-resolve "{{NNN}}") || exit 1
    rfd_id="${out%% *}"
    file="${out#* }"

    current=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$file" | head -1)
    case "$current" in
        Draft)       next="Discussion" ;;
        Discussion)  next="Accepted" ;;
        Accepted)    next="Implemented" ;;
        *)
            echo "Cannot promote from '${current}'." >&2
            echo "Promotable statuses: Draft, Discussion, Accepted." >&2
            exit 1 ;;
    esac

    # --- Pre-flight (Draft -> Discussion): refuse if Requires or Extends
    # contain draft references. The promoted RFD becomes a published file;
    # published files cannot reference drafts (the loader's DNN check would
    # fail the next docs build).
    if [ "$current" = "Draft" ]; then
        for field in Requires Extends; do
            line=$(sed -n "s/^- \\*\\*${field}\\*\\*: //p" "$file" | head -1)
            if echo "$line" | grep -qE 'RFD D[0-9]+'; then
                echo "Cannot promote: '${field}' on $(basename "$file") contains draft references." >&2
                echo "  ${line}" >&2
                echo "Promote those drafts first, or remove the entries." >&2
                exit 1
            fi
        done

        # The docs build also rejects `DNN` tokens anywhere in a published
        # RFD's body, so refuse while the document references other drafts.
        # Exempt: this draft's own id (the cross-reference pass below
        # rewrites it to the permanent number) and the `Required by` /
        # `Extended by` metadata lines (draft back-links are stripped
        # automatically further down).
        stray=$(grep -v -e '^- \*\*Required by\*\*: ' -e '^- \*\*Extended by\*\*: ' "$file" \
            | grep -oE '(^|[^A-Za-z0-9_])D[0-9][0-9]([^A-Za-z0-9_]|$)' \
            | grep -oE 'D[0-9][0-9]' | sort -u | grep -v "^${rfd_id}\$" || true)
        if [ -n "$stray" ]; then
            echo "Cannot promote: $(basename "$file") references other drafts in its body:" >&2
            echo "$stray" | sed 's/^/  RFD /' >&2
            echo "Published RFDs must not reference drafts (the docs build rejects DNN tokens)." >&2
            echo "Promote those drafts first, or reword the references." >&2
            exit 1
        fi
    fi

    # --- Promotion gate (Discussion -> Accepted, Accepted -> Implemented):
    # check that all `Requires` dependencies are at a sufficient status.
    # Discussion -> Accepted requires deps to be Accepted, Implemented, or
    # Superseded; Accepted -> Implemented requires deps to be Implemented or
    # Superseded.
    case "$current" in
        Discussion) gate_states="Accepted Implemented Superseded" ;;
        Accepted)   gate_states="Implemented Superseded" ;;
        *)          gate_states="" ;;
    esac

    if [ -n "$gate_states" ]; then
        # Gather deps from `Requires` and `Extends` (unified gate: extension
        # is a kind of dependency, both participate).
        deps=""
        for field in Requires Extends; do
            line=$(sed -n "s/^- \\*\\*${field}\\*\\*: //p" "$file" | head -1)
            field_deps=$(echo "$line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}')
            deps="$deps $field_deps"
        done
        deps=$(echo "$deps" | tr ' ' '\n' | awk 'NF' | sort -u)

        if [ -n "$deps" ]; then
            unmet=""
            for dep in $deps; do
                if echo "$dep" | grep -qE '^D[0-9]+$'; then
                    dep_file=$(ls "docs/rfd/drafts/${dep}-"*.md 2>/dev/null | head -1)
                else
                    dep_file=$(ls "docs/rfd/${dep}-"*.md 2>/dev/null | head -1)
                fi
                if [ -z "$dep_file" ]; then
                    unmet="${unmet}\n  RFD ${dep} (not found)"
                    continue
                fi
                dep_status=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$dep_file" | head -1)
                case " $gate_states " in
                    *" $dep_status "*) ;;
                    *) unmet="${unmet}\n  RFD ${dep} (${dep_status})" ;;
                esac
            done
            if [ -n "$unmet" ]; then
                echo "Cannot promote to ${next}: dependencies not satisfied:" >&2
                printf "${unmet}\n" >&2
                echo "Required: status is one of: ${gate_states}" >&2
                exit 1
            fi
        fi
    fi

    # Resolved path of the promoted file, after any rename. Used by the
    # closing `comfort` pass.
    final_file="$file"

    # --- Draft -> Discussion: assign permanent number, rename file ---
    if [ "$current" = "Draft" ]; then
        basename_f=$(basename "$file")
        old_draft_id=$(echo "$basename_f" | sed 's/^\(D[0-9]*\)-.*/\1/')
        slug=$(echo "$basename_f" | sed 's/^[A-Z]*[0-9]*-//; s/\.md$//')

        # Assign next available permanent number.
        num=$(just _rfd-next-number) || exit 1
        new_basename="${num}-${slug}.md"
        new_file="docs/rfd/${new_basename}"
        final_file="$new_file"

        # Rewrite heading and status. Also strip one `../` level from markdown
        # link targets: the file moves from `docs/rfd/drafts/` up to
        # `docs/rfd/`, so any backlink to a non-draft RFD would otherwise
        # resolve one directory too high. Both inline links (`[...](../foo.md)`)
        # and reference definitions (`[label]: ../foo.md`) are handled.
        sed \
            -e "s/^# RFD [A-Z]*[0-9]*:/# RFD ${num}:/" \
            -e "s/^- \*\*Status\*\*: Draft/- **Status**: Discussion/" \
            -e 's|](\.\./|](|g' \
            -e 's|^\(\[[^]]*\]:[ ]*\)\.\./|\1|' \
            "$file" > "$new_file"
        rm "$file"

        # Carry the board position across renumbering.
        just _rfd-priority-rewrite "$old_draft_id" "$num"

        # Update cross-references in every RFD, including the promoted file
        # itself: its prose can self-reference by draft id ("once D24
        # lands"), and the initial rewrite above only covers the heading,
        # status, and link prefixes. Replace `RFD DNN` with `RFD NNN` in
        # prose, `DNN-slug.md` with the correct relative path to
        # `NNN-slug.md` in link targets, and standalone short mentions like
        # `DNN` (e.g. "D27 also widens the scope") with the bare number
        # `NNN`. Drafts under `drafts/` need a `../` prefix because the
        # promoted file moved up a directory.
        #
        # The short-form pass runs last so the long-form and basename
        # rewrites get first crack at their specific shapes (the
        # basename rule adds the `../` prefix, which the short-form
        # rule cannot). It runs twice, because sed's `g` flag consumes
        # the leading boundary character of each match — back-to-back
        # mentions like "D27 D27" need a second pass for the second
        # one to be recognised.
        updated=0
        for other in docs/rfd/*.md docs/rfd/drafts/*.md; do
            [ -f "$other" ] || continue
            if ! grep -qE \
                    -e "RFD ${old_draft_id}" \
                    -e "${basename_f}" \
                    -e "(^|[^A-Za-z0-9_])${old_draft_id}([^A-Za-z0-9_]|\$)" \
                    "$other"; then
                continue
            fi
            if [ "$(dirname "$other")" = "docs/rfd/drafts" ]; then
                link_replacement="../${new_basename}"
            else
                link_replacement="${new_basename}"
            fi
            sed -E \
                -e "s|RFD ${old_draft_id}|RFD ${num}|g" \
                -e "s|${basename_f}|${link_replacement}|g" \
                -e "s#(^|[^A-Za-z0-9_])${old_draft_id}([^A-Za-z0-9_]|\$)#\1${num}\2#g" \
                -e "s#(^|[^A-Za-z0-9_])${old_draft_id}([^A-Za-z0-9_]|\$)#\1${num}\2#g" \
                "$other" > "${other}.tmp"
            mv "${other}.tmp" "$other"
            echo "  updated ${old_draft_id} -> ${num} references in ${other}"
            updated=$((updated + 1))
        done

        # Strip draft entries from `Required by` and `Extended by` on the
        # promoted file. These are bookkeeping artefacts of the bidirectional
        # draft-draft policy; the file is now published and cannot carry
        # draft back-links.
        for field in "Required by" "Extended by"; do
            awk -v field="$field" '
                BEGIN { search = "^- \\*\\*" field "\\*\\*: " }
                $0 ~ search {
                    sub(search, "", $0)
                    n = split($0, entries, /, /)
                    new = ""
                    for (i = 1; i <= n; i++) {
                        if (entries[i] !~ /RFD D[0-9]+/) {
                            new = (new == "") ? entries[i] : new ", " entries[i]
                        }
                    }
                    if (new != "") print "- **" field "**: " new
                    next
                }
                { print }
            ' "$new_file" > "${new_file}.tmp"
            mv "${new_file}.tmp" "$new_file"
        done

        # Backfill: for each entry in `Requires` and `Extends`, ensure the
        # target lists the promoted RFD under the inverse field. Targets that
        # were drafts at link-time may not have the back-link (rule 3 of the
        # draft-aware matrix); add it now.
        for pair in "Requires:Required by" "Extends:Extended by"; do
            forward=$(echo "$pair" | cut -d: -f1)
            inverse=$(echo "$pair" | cut -d: -f2)

            line=$(sed -n "s/^- \\*\\*${forward}\\*\\*: //p" "$new_file" | head -1)
            [ -z "$line" ] && continue

            for dep in $(echo "$line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}'); do
                if echo "$dep" | grep -qE '^D[0-9]+$'; then
                    dep_file=$(ls "docs/rfd/drafts/${dep}-"*.md 2>/dev/null | head -1)
                else
                    dep_file=$(ls "docs/rfd/${dep}-"*.md 2>/dev/null | head -1)
                fi
                [ -z "$dep_file" ] && continue

                existing=$(sed -n "s/^- \\*\\*${inverse}\\*\\*: //p" "$dep_file" | head -1)
                if echo "$existing" | grep -qE "RFD ${num}([^0-9]|\$)"; then
                    continue
                fi

                dep_dir=$(dirname "$dep_file")
                if [ "$dep_dir" = "docs/rfd/drafts" ]; then
                    rel="../${new_basename}"
                else
                    rel="${new_basename}"
                fi
                link="[RFD ${num}](${rel})"

                if [ -n "$existing" ]; then
                    sed "s|^- \\*\\*${inverse}\\*\\*: .*|&, ${link}|" "$dep_file" > "${dep_file}.tmp"
                    mv "${dep_file}.tmp" "$dep_file"
                else
                    first_heading=$(grep -n '^## ' "$dep_file" | head -1 | cut -d: -f1)
                    last_meta=$(head -n "${first_heading:-9999}" "$dep_file" | grep -n '^- \*\*' | tail -1 | cut -d: -f1)
                    awk -v ln="$last_meta" -v entry="- **${inverse}**: ${link}" '
                        NR == ln { print; print entry; next }
                        { print }
                    ' "$dep_file" > "${dep_file}.tmp"
                    mv "${dep_file}.tmp" "$dep_file"
                fi
                echo "  backfilled ${inverse}: RFD ${num} into ${dep_file}"
            done
        done

        echo "${new_file}: Draft -> Discussion (assigned ${num})"
        if [ "$updated" -gt 0 ]; then
            echo "Updated ${updated} cross-reference(s) in RFD files."
        fi

    # --- Discussion -> Accepted: create tracking issue via jp ---
    elif [ "$current" = "Discussion" ]; then
        sed "s/^- \*\*Status\*\*: Discussion/- **Status**: Accepted/" "$file" > "${file}.tmp"
        mv "${file}.tmp" "$file"

        # Decide whether to create a tracking issue. When a TTY is
        # attached, ask the caller so they can skip issue creation. In
        # non-interactive runs (e.g. CI), default to creating one to
        # preserve prior behaviour.
        create_issue=true
        if [ -r /dev/tty ] && [ -w /dev/tty ]; then
            printf "Create GitHub tracking issue for %s? [Y/n] " "$(basename "$file")" > /dev/tty
            if IFS= read -r answer < /dev/tty; then
                case "$answer" in
                    n|N|no|No|NO) create_issue=false ;;
                esac
            fi
        fi

        if [ "$create_issue" = true ]; then
            # Create tracking issue using jp + structured output.
            SCHEMA='{"type":"object","properties":{"number":{"type":"integer","description":"GitHub issue number"},"url":{"type":"string","description":"GitHub issue URL"}},"required":["number","url"]}'
            PROMPT="Read the attached RFD. Create a tracking issue for it by calling the github_create_issue_rfd_tracking tool. Return the issue number and url."
            TOOL_CFG='conversation.tools.github_create_issue_rfd_tracking:={"enable":true,"run":"unattended"}'

            result=$(
                jp query --new --local --tmp=5m --format=json --no-reasoning \
                    -c "$TOOL_CFG" \
                    --schema "$SCHEMA" \
                    --attachment "$file" \
                    "$PROMPT" \
                | jq -s '.[-1]' 2>/dev/null
            ) || true

            issue_num=$(echo "$result" | jq -r '.number // empty' 2>/dev/null || true)
            issue_url=$(echo "$result" | jq -r '.url // empty' 2>/dev/null || true)

            if [ -n "$issue_num" ] && [ -n "$issue_url" ]; then
                first_heading=$(grep -n '^## ' "$file" | head -1 | cut -d: -f1)
                last_meta=$(head -n "${first_heading:-9999}" "$file" | grep -n '^- \*\*' | tail -1 | cut -d: -f1)
                awk -v ln="$last_meta" -v ti="- **Tracking Issue**: [#${issue_num}](${issue_url})" '
                    NR == ln { print; print ti; next }
                    { print }
                ' "$file" > "${file}.tmp"
                mv "${file}.tmp" "$file"
                echo "${file}: Discussion -> Accepted"
                echo "Tracking issue: #${issue_num} (${issue_url})"
            else
                echo "${file}: Discussion -> Accepted"
                echo "Warning: tracking issue creation failed or was skipped." >&2
                echo "Create one manually and add '- **Tracking Issue**: #NNN' to the metadata." >&2
            fi
        else
            echo "${file}: Discussion -> Accepted"
            echo "Skipped tracking issue creation. Add one manually if needed." >&2
        fi

    # --- Accepted -> Implemented ---
    else
        sed "s/^- \*\*Status\*\*: Accepted/- **Status**: Implemented/" "$file" > "${file}.tmp"
        mv "${file}.tmp" "$file"

        # Strip `Requires` on promotion to Implemented. By the promotion gate,
        # every `Requires` target is already Implemented or Superseded, so the
        # dependency is satisfied for good and the link serves no further
        # purpose (see RFD 001). Drop the `Requires` line from this file, and
        # drop the matching `Required by: RFD <this>` back-link from each
        # former target.
        this_n=$(basename "$file" | sed 's/^\([0-9]*\)-.*/\1/; s/^0*//')
        this_n=${this_n:-0}
        requires_line=$(sed -n 's/^- \*\*Requires\*\*: //p' "$file" | head -1)

        sed '/^- \*\*Requires\*\*: /d' "$file" > "${file}.tmp"
        mv "${file}.tmp" "$file"

        if [ -n "$requires_line" ]; then
            for dep in $(echo "$requires_line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}'); do
                if echo "$dep" | grep -qE '^D[0-9]+$'; then
                    dep_file=$(ls "docs/rfd/drafts/${dep}-"*.md 2>/dev/null | head -1)
                else
                    dep_file=$(ls "docs/rfd/${dep}-"*.md 2>/dev/null | head -1)
                fi
                [ -z "$dep_file" ] && continue

                awk -v num="$this_n" '
                    BEGIN { search = "^- \\*\\*Required by\\*\\*: " }
                    $0 ~ search {
                        sub(search, "", $0)
                        n = split($0, entries, /, /)
                        new = ""
                        for (i = 1; i <= n; i++) {
                            if (entries[i] !~ ("RFD 0*" num "([^0-9]|$)")) {
                                new = (new == "") ? entries[i] : new ", " entries[i]
                            }
                        }
                        if (new != "") print "- **Required by**: " new
                        next
                    }
                    { print }
                ' "$dep_file" > "${dep_file}.tmp"
                mv "${dep_file}.tmp" "$dep_file"
                echo "  stripped Required by: RFD ${this_n} from ${dep_file}"
            done
        fi

        # Mirror of the strip above, in the other direction. Any RFD still in
        # flight that lists this one under `Requires` now points at an
        # Implemented dependency, which the docs build rejects (see
        # `checkRequiresOnImplemented`). Drop this file's `Required by` line and
        # the matching `Requires: RFD <this>` entry from each dependent.
        required_by_line=$(sed -n 's/^- \*\*Required by\*\*: //p' "$file" | head -1)

        sed '/^- \*\*Required by\*\*: /d' "$file" > "${file}.tmp"
        mv "${file}.tmp" "$file"

        if [ -n "$required_by_line" ]; then
            for dependent in $(echo "$required_by_line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}'); do
                if echo "$dependent" | grep -qE '^D[0-9]+$'; then
                    dep_file=$(ls "docs/rfd/drafts/${dependent}-"*.md 2>/dev/null | head -1)
                else
                    dep_file=$(ls "docs/rfd/${dependent}-"*.md 2>/dev/null | head -1)
                fi
                [ -z "$dep_file" ] && continue

                awk -v num="$this_n" '
                    BEGIN { search = "^- \\*\\*Requires\\*\\*: " }
                    $0 ~ search {
                        sub(search, "", $0)
                        n = split($0, entries, /, /)
                        new = ""
                        for (i = 1; i <= n; i++) {
                            if (entries[i] !~ ("RFD 0*" num "([^0-9]|$)")) {
                                new = (new == "") ? entries[i] : new ", " entries[i]
                            }
                        }
                        if (new != "") print "- **Requires**: " new
                        next
                    }
                    { print }
                ' "$dep_file" > "${dep_file}.tmp"
                mv "${dep_file}.tmp" "$dep_file"
                echo "  stripped Requires: RFD ${this_n} from ${dep_file}"
            done
        fi

        echo "${file}: Accepted -> Implemented"
    fi

    # Reflow the promoted file: wrap prose with semantic line breaks and
    # consolidate reference-style link definitions at the bottom, matching the
    # markdown formatting CI enforces (`fmt-markdown-ci`).
    comfort --language markdown --format-markdown --reference-links "$final_file"

# Renumber an RFD to a new id, updating every cross-reference.
#
# NNN is the RFD to renumber: a permanent number (95, 095) or a draft ID
# (D24). MMM is the target id and must live in the same id-space as NNN
# (drafts renumber to another draft slot, published RFDs to another permanent
# number; moving between spaces is `rfd-promote`'s job). When MMM is omitted,
# the next available id in that space is used. The scan only sees local
# files, so a number taken on another branch must be avoided by passing MMM
# explicitly.
#
# Rewrites the file name, the document heading, `RFD <old>` mentions and
# `<old>-slug.md` link targets across all RFDs (including the renumbered file
# itself), bare `DNN` tokens for draft-space renumbers, and the id in
# `priority.json`. The file stays in its directory, so existing link prefixes
# (`../`, `./`) remain correct and only the basename is substituted.
# References outside `docs/rfd/` (code comments, other docs) are reported but
# not rewritten.
#
# Renumbering a published RFD changes its site URL and invalidates its
# summary-cache entry; run `just rfd-summaries` afterwards.
[group('rfd')]
rfd-renumber NNN MMM="":
    #!/usr/bin/env sh
    set -eu

    out=$(just _rfd-resolve "{{NNN}}") || exit 1
    old_id="${out%% *}"
    file="${out#* }"

    if [ "$old_id" = "000" ]; then
        echo "Refusing to renumber a template." >&2; exit 1
    fi

    dir=$(dirname "$file")
    old_basename=$(basename "$file")
    slug=$(echo "$old_basename" | sed 's/^[A-Z]*[0-9]*-//; s/\.md$//')

    case "$old_id" in
        D*) is_draft=true ;;
        *)  is_draft=false ;;
    esac

    # --- Determine and validate the target id ---
    target="{{MMM}}"
    if [ -n "$target" ]; then
        if [ "$is_draft" = true ]; then
            if ! echo "$target" | grep -qiE '^D[0-9]{1,2}$'; then
                echo "Target for a draft must be a draft slot (D01-D99), got '${target}'." >&2
                exit 1
            fi
            n=$(echo "$target" | sed 's/^[Dd]0*//')
            new_id=$(printf "D%02d" "${n:-0}")
        else
            if ! echo "$target" | grep -qE '^[0-9]+$'; then
                echo "Target for a published RFD must be a number, got '${target}'." >&2
                exit 1
            fi
            n=$(echo "$target" | sed 's/^0*//')
            new_id=$(printf "%03d" "${n:-0}")
        fi
        if [ "${n:-0}" -eq 0 ]; then
            echo "Target id must be greater than zero." >&2; exit 1
        fi
    else
        if [ "$is_draft" = true ]; then
            new_id=$(just _rfd-next-draft-slot) || exit 1
        else
            new_id=$(just _rfd-next-number) || exit 1
        fi
    fi

    if [ "$new_id" = "$old_id" ]; then
        echo "RFD ${old_id} already has that id; nothing to do." >&2; exit 1
    fi

    if [ "$is_draft" = true ]; then
        taken=$(ls "docs/rfd/drafts/${new_id}-"*.md 2>/dev/null | head -1)
    else
        taken=$(ls "docs/rfd/${new_id}-"*.md 2>/dev/null | head -1)
    fi
    if [ -n "$taken" ]; then
        echo "Target id ${new_id} is taken by $(basename "$taken")." >&2; exit 1
    fi

    # --- Rename the file and rewrite its heading ---
    new_basename="${new_id}-${slug}.md"
    new_file="${dir}/${new_basename}"
    sed "s/^# RFD [A-Z]*[0-9]*:/# RFD ${new_id}:/" "$file" > "$new_file"
    rm "$file"

    # --- Carry the board position across renumbering ---
    just _rfd-priority-rewrite "$old_id" "$new_id"

    # --- Cross-references in every RFD, including the renumbered file ---
    # Bare-token rewriting is draft-space only: `D24` is a distinctive
    # token, a bare `095` is not. The bare-token rule runs twice because
    # sed's `g` flag consumes the leading boundary character of a match,
    # hiding the second of two back-to-back mentions.
    updated=0
    for other in docs/rfd/*.md docs/rfd/drafts/*.md; do
        [ -f "$other" ] || continue
        if [ "$is_draft" = true ]; then
            sed -E \
                -e "s#RFD ${old_id}([^0-9]|\$)#RFD ${new_id}\1#g" \
                -e "s|${old_basename}|${new_basename}|g" \
                -e "s#(^|[^A-Za-z0-9_])${old_id}([^A-Za-z0-9_]|\$)#\1${new_id}\2#g" \
                -e "s#(^|[^A-Za-z0-9_])${old_id}([^A-Za-z0-9_]|\$)#\1${new_id}\2#g" \
                "$other" > "${other}.tmp"
        else
            sed -E \
                -e "s#RFD ${old_id}([^0-9]|\$)#RFD ${new_id}\1#g" \
                -e "s|${old_basename}|${new_basename}|g" \
                "$other" > "${other}.tmp"
        fi
        if cmp -s "$other" "${other}.tmp"; then
            rm "${other}.tmp"
            continue
        fi
        mv "${other}.tmp" "$other"
        echo "  updated ${old_id} -> ${new_id} references in ${other}"
        updated=$((updated + 1))
    done

    echo "${old_basename} -> ${new_file} (${old_id} -> ${new_id})"
    if [ "$updated" -gt 0 ]; then
        echo "Updated ${updated} file(s) with cross-references."
    fi

    # --- Report references the rewrite does not touch ---
    leftovers=$(rg -l -e "RFD ${old_id}\b" -e "${old_basename}" \
        --glob '!docs/rfd/**' . 2>/dev/null || true)
    if [ -n "$leftovers" ]; then
        echo "" >&2
        echo "Warning: references outside docs/rfd/ still mention ${old_id}:" >&2
        echo "$leftovers" | sed 's/^/  /' >&2
    fi

    if [ "$is_draft" = false ]; then
        echo "Run \`just rfd-summaries\` to refresh the summary cache." >&2
    fi

# Internal: print the first available draft slot id (D01–D99).
#
# Exits 1 when all 99 slots are in use. Callers should propagate the exit
# status with `|| exit 1`.
[no-exit-message]
[private]
_rfd-next-draft-slot:
    #!/usr/bin/env sh
    set -eu

    next=1
    while [ "$next" -le 99 ]; do
        draft_id=$(printf "D%02d" "$next")
        if ! ls docs/rfd/drafts/${draft_id}-*.md >/dev/null 2>&1; then
            break
        fi
        next=$((next + 1))
    done
    if [ "$next" -gt 99 ]; then
        echo "No draft slots available (D01–D99 all in use)." >&2; exit 1
    fi
    printf "D%02d\n" "$next"

# Internal: print the next available permanent RFD number, zero-padded.
#
# Walks the sorted existing numbers under `docs/rfd/` and takes the first gap
# (max + 1 when there are none). Only local files are visible: a number taken
# on another branch is not detected.
[private]
_rfd-next-number:
    #!/usr/bin/env sh
    set -eu

    existing=$(ls docs/rfd/[0-9][0-9][0-9]-*.md 2>/dev/null \
        | sed 's|.*/||; s|-.*||' \
        | sort -n)
    next_num=1
    for num_iter in $existing; do
        n=$(echo "$num_iter" | sed 's/^0*//')
        n=${n:-0}
        [ "$n" -lt "$next_num" ] && continue
        [ "$n" -gt "$next_num" ] && break
        next_num=$((next_num + 1))
    done
    printf "%03d\n" "$next_num"

# Internal: rewrite an RFD id in the priority board.
#
# `priority.json` stores RFD ids; substitute OLD for NEW wherever the id
# appears (the `planned` milestone groups, `backlog`, `in_development`, and
# the legacy flat `order`). A missing board file is a no-op.
[private]
_rfd-priority-rewrite OLD NEW:
    #!/usr/bin/env sh
    set -eu

    priority_file="docs/rfd/priority.json"
    [ -f "$priority_file" ] || exit 0
    jq --arg old "{{OLD}}" --arg new "{{NEW}}" '
        def sub_id: map(if . == $old then $new else . end);
        (if .planned then .planned |= map(.ids |= sub_id) else . end)
        | (if .order then .order |= sub_id else . end)
        | .backlog = ((.backlog // []) | sub_id)
        | .in_development = ((.in_development // []) | sub_id)
    ' "$priority_file" > "${priority_file}.tmp" && mv "${priority_file}.tmp" "$priority_file"

# Mark an RFD as abandoned with the given reason.
[group('rfd')]
rfd-abandon NNN +REASON:
    #!/usr/bin/env sh
    set -eu

    n=$(echo "{{NNN}}" | sed 's/^0*//')
    num=$(printf "%03d" "${n:-0}")
    file=$(ls docs/rfd/${num}-*.md 2>/dev/null | head -1)
    if [ -z "$file" ]; then
        echo "No RFD found with number ${num}." >&2; exit 1
    fi

    current=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$file" | head -1)
    case "$current" in
        Implemented|Superseded|Abandoned)
            echo "Cannot abandon from '${current}'." >&2; exit 1 ;;
    esac

    sed "s/^- \*\*Status\*\*: ${current}/- **Status**: Abandoned/" "$file" > "${file}.tmp"
    mv "${file}.tmp" "$file"

    # Append the reason as a note after the metadata block.
    awk -v reason="{{REASON}}" '
        /^## / && !done { print "> **Abandoned**: " reason; print ""; done=1 }
        { print }
    ' "$file" > "${file}.tmp"
    mv "${file}.tmp" "$file"

    # Remind the user to close the tracking issue if one exists.
    tracking=$(sed -n 's/^- \*\*Tracking Issue\*\*: \[#\([0-9]*\)\].*/\1/p' "$file" | head -1)
    echo "${file}: Abandoned (${current} -> Abandoned)"
    if [ -n "$tracking" ]; then
        echo "Remember to close the tracking issue: https://github.com/dcdpr/jp/issues/${tracking}"
    fi

    # Warn about RFDs that depend on this one (`Required by` field). The
    # abandonment doesn't auto-cascade or auto-fix; the dependents need
    # manual review. The check uses this RFD's own `Required by` field as
    # the source of truth (assumes `rfd-require` was used to maintain it).
    required_by_line=$(sed -n 's/^- \*\*Required by\*\*: //p' "$file" | head -1)
    if [ -n "$required_by_line" ]; then
        echo "" >&2
        echo "Warning: the following RFDs depend on this one (Required by):" >&2
        for r in $(echo "$required_by_line" | grep -oE 'RFD (D[0-9]+|[0-9]{3})' | awk '{print $2}'); do
            echo "  RFD ${r}" >&2
        done
        echo "Their dependency on RFD ${num} is now broken — review and update." >&2
    fi

# Generate or update AI summaries for RFD documents.
#
# Only re-generates summaries for RFDs whose content has changed since
# the last run (based on SHA-256). Pass `--force` to regenerate all.
#
# Usage:
#   just rfd-summaries              # changed RFDs only, default model
#   just rfd-summaries --force       # regenerate all
#   just rfd-summaries flash         # use a different model
#   just rfd-summaries flash --force # both
[group('rfd')]
rfd-summaries *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    CACHE="docs/.vitepress/rfd-summaries.json"
    MODEL="haiku"
    FORCE=false
    BASE_PROMPT="summarize this document in one sentence of max 20 words, don't start with 'The/This RFD ...'"
    SCHEMA='{"type":"object","properties":{"changed":{"type":"boolean","description":"false if the existing summary is still accurate, true if you wrote a new one"},"summary":{"type":"string"}},"required":["changed","summary"]}'

    for arg in {{ARGS}}; do
        case "$arg" in
            --force) FORCE=true ;;
            *)       MODEL="$arg" ;;
        esac
    done

    [ -f "$CACHE" ] || echo '{}' > "$CACHE"

    generated=0
    kept=0
    skipped=0

    for file in docs/rfd/[0-9][0-9][0-9]-*.md; do
        [ -f "$file" ] || continue
        basename=$(basename "$file")
        case "$basename" in 000-*) continue ;; esac

        hash=$(shasum -a 256 "$file" | cut -d' ' -f1)
        cached_hash=$(jq -r --arg f "$basename" '.[$f].hash // ""' "$CACHE")

        if [ "$FORCE" = false ] && [ "$hash" = "$cached_hash" ]; then
            skipped=$((skipped + 1))
            continue
        fi

        num=$(echo "$basename" | sed 's/-.*//')
        existing=$(jq -r --arg f "$basename" '.[$f].summary // ""' "$CACHE")

        if [ -n "$existing" ]; then
            PROMPT="The current summary is: \"${existing}\". If this still accurately captures the document, set changed=false and return it as-is. Otherwise set changed=true and ${BASE_PROMPT}"
        else
            PROMPT="Set changed=true and ${BASE_PROMPT}"
        fi

        printf "RFD %s..." "$num" >&2

        result=$(
            jp -! q --format=json --no-tools --new \
                --schema "$SCHEMA" --no-reasoning \
                --attachment "$file" --model "$MODEL" \
                "$PROMPT" \
            | jq -s '.[-1]'
        )

        changed=$(echo "$result" | jq -r '.changed')

        if [ "$changed" = "true" ]; then
            summary=$(echo "$result" | jq -r '.summary')
            generated=$((generated + 1))
            printf " updated\n" >&2
        else
            summary="$existing"
            kept=$((kept + 1))
            printf " kept\n" >&2
        fi

        jq --arg f "$basename" --arg h "$hash" --arg s "$summary" \
            '.[$f] = {hash: $h, summary: $s}' "$CACHE" > "${CACHE}.tmp"
        mv "${CACHE}.tmp" "$CACHE"
    done

    # Remove entries for deleted RFDs.
    existing=$(ls -1 docs/rfd/[0-9][0-9][0-9]-*.md 2>/dev/null | xargs -I{} basename {} | jq -R -s 'split("\n") | map(select(. != ""))')
    jq --argjson keep "$existing" 'with_entries(select(.key as $k | $keep | index($k)))' "$CACHE" > "${CACHE}.tmp"
    mv "${CACHE}.tmp" "$CACHE"

    printf "\nDone: %d updated, %d kept, %d cached\n" "$generated" "$kept" "$skipped" >&2

# Search across all RFD documents.
[group('rfd')]
rfd-grep +ARGS:
    @rg {{ARGS}} docs/rfd/

# List RFDs in priority order, optionally filtered by category.
#
# Shares the priority board's data (priority.json + summaries + relationship
# graph) via `docs/.vitepress/loaders/rfd-shared.mjs`, so this list and the web
# board at `/rfd/priority` stay in sync.
#
# Usage:
#   just rfd-list                 # planned RFDs, in priority order (default)
#   just rfd-list --backlog       # the unranked backlog instead
#   just rfd-list --all           # planned + backlog + terminal/implemented
#   just rfd-list --full          # add summaries and dependencies
#   just rfd-list design          # filter to the "design" category
#   just rfd-list --json          # every entry, tagged, for `jq` etc.
[group('rfd')]
rfd-list *ARGS:
    node docs/.vitepress/rfd-list.mjs {{ARGS}}

# Locally develop the documentation, with hot-reloading.
[group('docs')]
develop-docs *FLAGS="--open": rfd-summaries
    just _docs "dev" {{FLAGS}}

# Open the RFD priority board for drag-and-drop reordering.
#
# Starts the docs dev server and opens the board at `/rfd/priority`. Dragging
# rows and toggling "in development" writes `docs/rfd/priority.json`; commit that
# file to publish the new order. The board is read-only in the production build
# — the write endpoint only exists on the dev server.
[group('rfd')]
rfd-manage: rfd-summaries
    just _docs "dev" "--open" "/rfd/priority"

# Build the statically built documentation.
[group('docs')]
build-docs: (_docs "build")

# Preview the statically built documentation.
[group('docs')]
preview-docs: (_docs "preview")

# Live-check the code, using Clippy and Bacon.
[group('check')]
check *FLAGS:
    @just _bacon clippy {{FLAGS}}

# Live-check the code, including tests, using Clippy and Bacon.
[group('check')]
[group('main')]
check-all *FLAGS:
    @just _bacon clippy_all {{FLAGS}}

# Live-check the code, using Clippy and Bacon, auto-fixing as much as possible.
[group('check')]
check-and-fix *FLAGS:
    @just check --fix --allow-dirty {{FLAGS}}

# Run tests, using nextest.
[group('check')]
[group('main')]
test *FLAGS="--workspace": (_install "cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version)
    cargo nextest run --all-targets --cargo-profile=nextest {{FLAGS}}

# Continuously run tests, using Bacon.
[group('check')]
testw *FLAGS:
    just _bacon test {{FLAGS}}

# Check for unused dependencies.
[group('check')]
shear *FLAGS="--fix": (_install "cargo-shear@" + shear_version)
    cargo shear {{FLAGS}}

[group('check')]
coverage: _coverage-setup
    # FIXME: Branch coverage seems to have broken recently?
    # cargo llvm-cov --doctests --branch --lcov --no-cfg-coverage --no-cfg-coverage-nightly --profile=coverage --output-path=target/lcov.info
    cargo llvm-cov --doctests --lcov --no-cfg-coverage --no-cfg-coverage-nightly --profile=coverage --output-path=target/lcov.info

_bacon CMD *FLAGS: (_install "bacon@" + bacon_version)
    @bacon {{CMD}} -- {{FLAGS}}

[group('tools')]
install-tools:
    cargo install --locked --path .config/jp/tools --debug

[group('tools')]
serve-tools CONTEXT TOOL:
    @jp-tools {{quote(CONTEXT)}} {{quote(TOOL)}}

# Run the bookworm MCP server (docs.rs documentation tools).
#
# Rebuilds the release binary first; `cargo build` is incremental, so this is
# a no-op when nothing has changed and a fast incremental compile when it has.
# The repo's `.jp/config.toml` points `providers.mcp.bookworm.command` at this
# recipe, so every `jp query` that uses bookworm tools picks up the latest
# local source automatically.
[group('tools')]
serve-bookworm: _build-bookworm
    @$(cargo metadata --format-version 1 | jq -r .build_directory)/release/bookworm mcp

[private]
@_build-bookworm:
    cargo build {{quiet_flag}} --release --package bookworm

# Build all command plugin binaries for a target (defaults to host).
[group('plugins')]
plugin-build TARGET="":
    #!/usr/bin/env sh
    set -eu
    target="${TARGET:-$(rustc -vV | sed -n 's/host: //p')}"
    for manifest in crates/plugins/command/*/Cargo.toml; do
        [ -f "$manifest" ] || continue
        echo "Building $(basename "$(dirname "$manifest")") for $target..."
        cargo build --release --manifest-path "$manifest" --target "$target"
    done

# Generate plugins.json from workspace metadata.
# Without CHECKSUMS, produces a registry with no binary download info.
[group('plugins')]
plugin-registry-build CHECKSUMS="":
    #!/usr/bin/env sh
    set -eu
    args="--groups docs/registry/groups.toml"
    if [ -n "{{CHECKSUMS}}" ]; then
        args="$args --checksums {{CHECKSUMS}}"
    fi
    cargo run --quiet -p build-registry -- $args

# Fetch the latest released plugin registry from GitHub.
#
# Writes into the VitePress public directory so the file is served verbatim at
# the site root (https://jp.computer/plugins.json), which is the URL the CLI
# reads the registry from.
[group('plugins')]
plugin-registry-fetch:
    #!/usr/bin/env sh
    set -eu
    mkdir -p docs/public
    curl -fL https://raw.githubusercontent.com/dcdpr/jp/plugin-registry/plugins.json \
        -o docs/public/plugins.json

# Build plugins for the host and install to the local plugin directory.
[group('plugins')]
plugin-build-local: _install-jp (plugin-build "")
    #!/usr/bin/env sh
    set -eu
    target=$(rustc -vV | sed -n 's/host: //p')
    dir="$(jp path user-local --plugins=command)"
    mkdir -p "$dir"
    for manifest in crates/plugins/command/*/Cargo.toml; do
        [ -f "$manifest" ] || continue
        id=$(cargo metadata --manifest-path "$manifest" --format-version=1 --no-deps \
            | jq -r '.packages[0].metadata["jp-registry"].id')
        src="target/${target}/release/jp-${id}"
        [ -f "$src" ] || continue
        cp "$src" "${dir}/jp-${id}"
        chmod +x "${dir}/jp-${id}"
        echo "Installed jp-${id} → ${dir}/jp-${id}"
    done

# Run all ci tasks.
[group('ci')]
ci: lint-ci fmt-ci test-ci docs-ci coverage-ci deny-ci insta-ci shear-ci vet-ci

# Lint the code on CI.
[group('ci')]
lint-ci: (_rustup_component "clippy") _install_ci_matchers
    cargo clippy --locked --workspace --all-targets --all-features --no-deps --profile=lint -- --deny warnings

# Check code formatting on CI.
[group('ci')]
fmt-ci: (_rustup_component "rustfmt") _install_ci_matchers
    cargo fmt --all --check

# Check Rust doc-comment formatting on CI.
[group('ci')]
fmt-comments-ci: _install-comfort _install_ci_matchers
    comfort --check --workspace --language rust --format-markdown --reference-links --prune-reference-links

# Check standalone Markdown formatting on CI.
[group('ci')]
fmt-markdown-ci: _install-comfort _install_ci_matchers
    comfort --check --workspace --language markdown --format-markdown --reference-links --prune-reference-links

# Test the code on CI.
[group('ci')]
test-ci: (_install "cargo-nextest@" + nextest_version) _install_ci_matchers
    cargo nextest run --locked --lib --tests --cargo-profile=nextest --workspace --no-fail-fast

# Generate documentation on CI.
[group('ci')]
docs-ci: _install_ci_matchers
    #!/usr/bin/env sh
    set -eu

    export RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links -D rustdoc::private-intra-doc-links -D rustdoc::invalid-codeblock-attributes -D rustdoc::invalid-html-tags -D rustdoc::invalid-rust-codeblocks -D rustdoc::bare-urls -D rustdoc::unescaped-backticks -D rustdoc::redundant-explicit-links"
    cargo doc --locked --workspace --profile=docs --all-features --keep-going --document-private-items --no-deps

# Generate code coverage on CI.
[group('ci')]
coverage-ci: _coverage-setup _install_ci_matchers
    cargo llvm-cov --locked --no-cfg-coverage --no-cfg-coverage-nightly --cargo-profile=coverage --no-report nextest
    cargo llvm-cov --locked --no-cfg-coverage --no-cfg-coverage-nightly --profile=coverage --no-report --doc
    cargo llvm-cov report --doctests --lcov --output-path lcov.info --profile=coverage

_coverage-setup: (_rustup_component "llvm-tools") _install-llvm-cov (_install "cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version)

# cargo-llvm-cov disables the QuickInstall strategy in its binstall metadata,
# so `--only-signed` can never be satisfied. Install separately without it.
@_install-llvm-cov: _install-binstall
    cargo binstall {{quiet_flag}} --locked --disable-telemetry --no-confirm cargo-llvm-cov@{{llvm_cov_version}}

# Check for security vulnerabilities on CI.
[group('ci')]
deny-ci: (_install "cargo-deny@" + deny_version) _install_ci_matchers
    cargo deny check -A index-failure --hide-inclusion-graph

# Validate insta snapshots on CI.
[group('ci')]
insta-ci: _insta-ci-setup
    cargo insta test --check --unreferenced=auto

_insta-ci-setup: (_install "cargo-nextest@" + nextest_version + " cargo-insta@" + insta_version + " cargo-expand@" + expand_version)

# Check for unused dependencies on CI.
[group('ci')]
shear-ci: (_install "cargo-expand@" + expand_version)
    @just shear --expand

# Verify supply-chain audits on CI.
[group('ci')]
vet-ci: (_install "cargo-vet@" + vet_version)
    cargo vet --locked

@_install_ci_matchers:
    echo "::add-matcher::.github/matchers.json"

[working-directory: 'docs']
@_docs CMD="dev" *FLAGS: _docs-install
    yarn vitepress {{CMD}} {{FLAGS}}

@_install +CRATES: _install-binstall
    cargo binstall {{quiet_flag}} --locked --disable-telemetry --no-confirm --only-signed {{CRATES}}

_install-jp *args:
    #!/usr/bin/env sh
    set -eu
    if [ -n "${JP_NO_INSTALL:-}" ]; then
        echo "Skipping jp rebuild (JP_NO_INSTALL set); using the installed binary." >&2
        exit 0
    fi
    cargo install {{quiet_flag}} --locked --path crates/jp_cli {{args}}

# Build and install the `jp-tools` binary that `serve-tools` runs for local MCP
# tools. Recipes that invoke local tools (e.g. `git_status`) depend on this so a
# stale binary doesn't return `Unknown tool` for a tool added in the checkout.
_install-tools *args:
    #!/usr/bin/env sh
    set -eu
    if [ -n "${JP_NO_INSTALL:-}" ]; then
        echo "Skipping jp-tools rebuild (JP_NO_INSTALL set); using the installed binary." >&2
        exit 0
    fi
    cargo install {{quiet_flag}} --locked --path .config/jp/tools --debug {{args}}

@_install-comfort *args:
    cargo install {{quiet_flag}} --locked --path crates/contrib/comfort {{args}}

@_install-binstall:
    command -v cargo-binstall >/dev/null 2>&1 || { \
        curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | BINSTALL_VERSION={{binstall_version}} sh; \
    }

[working-directory: 'docs']
@_docs-install:
    yarn install --immutable

@_rustup_component +COMPONENTS:
    rustup component add {{COMPONENTS}}

# Internal: resolve a conversation by title.
#
# Looks up an active conversation whose title equals TITLE. If found and the
# caller is on a TTY, prompts the user for [c]ontinue / [n]ew (archive old) /
# [q]uit. Outputs one of:
#
#   CONTINUE <id>   - caller should resume this conversation
#   ARCHIVE <id>    - caller should archive this id and start fresh
#   NEW             - no existing match, just start fresh
#   QUIT            - caller should exit cleanly
#
# The actual archive is left to the caller because `jp conversation archive`
# may itself prompt for confirmation (e.g. on the active conversation), and
# its prompt has to be visible to the user, not captured by `$()`.
[no-exit-message]
[private]
_resolve-conversation TITLE:
    #!/usr/bin/env sh
    set -eu

    existing=$(jp -F json conversation ls 2>/dev/null \
        | jq -r --arg t "{{TITLE}}" 'first(.[] | select(.Title == $t) | .ID) // empty' \
        2>/dev/null \
        || true)

    if [ -z "$existing" ]; then
        echo "NEW"
        exit 0
    fi

    if [ -r /dev/tty ] && [ -w /dev/tty ]; then
        printf "Found existing conversation %s titled '%s'.\n" "$existing" "{{TITLE}}" > /dev/tty
        printf "  [c]ontinue / [n]ew (archive old) / [q]uit: " > /dev/tty
        IFS= read -r choice < /dev/tty
    else
        choice=c
    fi

    case "$choice" in
        ""|c|C) echo "CONTINUE $existing" ;;
        n|N)    echo "ARCHIVE $existing" ;;
        q|Q)    echo "QUIT" ;;
        *)      echo "Unknown choice '$choice'; aborting." >&2; exit 1 ;;
    esac

# Internal: report whether the working tree holds the given PR's code.
#
# Resolves the PR head sha from refs/pull/N/head on the dcdpr/jp remote (no gh
# needed) and compares it to local HEAD. Outputs one of:
#
#   LOCAL <sha>   - HEAD is the PR head and the tree is clean
#   DIRTY <sha>   - HEAD is the PR head but the tree has uncommitted changes
#   REMOTE        - tree doesn't match the PR, or the head can't be resolved
#
# Callers inject the result into the prompt so the assistant knows whether to
# prefer the local fs_*/git_* tools over the slower github_* ones.
[no-exit-message]
[private]
_pr-checkout-state NNN:
    #!/usr/bin/env sh
    set -eu

    remote=$(git remote -v 2>/dev/null | awk '/dcdpr\/jp/ {print $1; exit}')
    remote=${remote:-origin}

    head_sha=$(git ls-remote "$remote" "refs/pull/{{NNN}}/head" 2>/dev/null \
        | awk '{print $1; exit}')

    [ -n "$head_sha" ] || { echo "REMOTE"; exit 0; }
    [ "$head_sha" = "$(git rev-parse HEAD 2>/dev/null || true)" ] \
        || { echo "REMOTE"; exit 0; }

    if [ -n "$(git status --porcelain 2>/dev/null)" ]; then
        echo "DIRTY $head_sha"
    else
        echo "LOCAL $head_sha"
    fi

# Internal: look up a Bear note (or notes) by tag.
#
# Resolves `bear://search/?tag=TAG` against the local Bear database. Archived
# notes are excluded: they're kept for reference, not for feeding into a
# session. Outputs one of:
#
#   FOUND <bear-uri>   - at least one note matched; caller should attach URI
#   EDIT               - no notes matched; caller should add `--edit`
#   CONTINUE           - no notes matched; caller should skip notes silently
#   QUIT               - caller should exit cleanly
#
# Resolution uses `jp attachment print`, which is read-only and stateless.
[no-exit-message]
[private]
_bear-note TAG:
    #!/usr/bin/env sh
    set -eu

    uri="bear://search/?tag={{TAG}}&exclude_archived=true"
    if jp attachment print "$uri" 2>/dev/null | grep -q .; then
        echo "FOUND $uri"
        exit 0
    fi

    if [ -r /dev/tty ] && [ -w /dev/tty ]; then
        printf "No Bear note tagged '%s' found.\n" "{{TAG}}" > /dev/tty
        printf "  [c]ontinue without note / [e]dit prompt inline / [q]uit: " > /dev/tty
        IFS= read -r ans < /dev/tty
    else
        ans=c
    fi

    case "$ans" in
        ""|c|C) echo "CONTINUE" ;;
        e|E)    echo "EDIT" ;;
        q|Q)    echo "QUIT" ;;
        *)      echo "Unknown choice '$ans'; aborting." >&2; exit 1 ;;
    esac

# Internal: resolve an RFD argument (DNN draft ID or NNN/NN permanent number)
# to its canonical id and file path.
#
# On success, prints `<rfd_id> <file>` to stdout on a single line:
#   - rfd_id is `DNN` for drafts, zero-padded `NNN` for permanent numbers.
#   - file is the relative path under `docs/rfd/` or `docs/rfd/drafts/`.
#
# On failure (invalid argument, file not found), writes a message to stderr
# and exits 1. Callers should propagate the exit status with `|| exit 1`.
[no-exit-message]
[private]
_rfd-resolve NNN:
    #!/usr/bin/env sh
    set -eu

    arg="{{NNN}}"
    if echo "$arg" | grep -qiE '^D[0-9]+$'; then
        rfd_id=$(echo "$arg" | tr '[:lower:]' '[:upper:]')
        file=$(ls docs/rfd/drafts/${rfd_id}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No draft RFD found with ID ${rfd_id}." >&2; exit 1
        fi
    elif echo "$arg" | grep -qE '^[0-9]+$'; then
        n=$(echo "$arg" | sed 's/^0*//')
        rfd_id=$(printf "%03d" "${n:-0}")
        file=$(ls docs/rfd/${rfd_id}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No RFD found with number ${rfd_id}." >&2; exit 1
        fi
    else
        echo "Invalid argument '${arg}'. Use a number (41) or draft ID (D01)." >&2; exit 1
    fi

    echo "${rfd_id} ${file}"

# Internal: shape a recipe's `*ARGS` and a default prompt MSG into a single
# `args` string that the recipe forwards to `jp query`.
#
# Resolves four shapes (in this order):
#
#   1. ARGS starts with a single `-- text` arg: pass-through. The user
#      supplied their own prompt; don't double up with MSG.
#   2. ARGS starts with a flag (-X) and doesn't contain `--`: the user is
#      passing jp flags only, so append `-- $MSG` to make MSG the prompt.
#   3. ARGS is non-empty free-form text: use MSG as preamble, ARGS as extra
#      context (separated by `\n\n Here is additional context: `).
#   4. ARGS is empty: use MSG alone.
#
# In every shape the free-text message is placed after a `--` so `jp query`
# treats it as the positional query, never as flags. Without this guard, a
# preceding option (e.g. `--edit`) word-split next to the message would swallow
# its first token as the option's value.
#
# Prints the resulting `args` string to stdout with no trailing newline.
# Callers use it as: `args=$(just _shape-args "$msg" "$@")`.
[no-exit-message]
[private]
[positional-arguments]
_shape-args MSG *ARGS:
    #!/usr/bin/env sh
    set -eu

    msg="$1"; shift

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains()    { case ${2-} in *"$1"*) true;; *) false;; esac; }

    args="$*"
    if starts_with "-- " "$@"; then
        :
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="-- $msg\n\n Here is additional context: $args"
    else
        args="-- $msg"
    fi

    printf '%s' "$args"
