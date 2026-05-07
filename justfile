bacon_version        := "3.22.0"
binstall_version     := "1.17.9"
deny_version         := "0.19.0"
expand_version       := "1.0.121"
insta_version        := "1.46.3"
jilu_version         := "0.13.2"
llvm_cov_version     := "0.8.5"
nextest_version      := "0.9.132"
shear_version        := "1.11.2"
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
    cargo run --package jp_cli -- "$@"

# Install the `jp` binary from your local checkout.
[group('build')]
[group('main')]
install:
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
    args="$@"
    msg="Give me a commit message"

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --new --local --tmp=1h --cfg=personas/committer $args || exit 1
    git commit --amend

[group('jp')]
[positional-arguments]
stage *ARGS: _install-jp
    #!/usr/bin/env sh
    args="$@"
    msg="Find related changes in the git diff and stage ONE set of changes in preparation for a \
    commit using the 'git_stage_patch' tool. Follow your prompt instructions carefully."

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --new --local --tmp=1h --cfg=personas/stager $args

stage-and-commit: _install-jp
    #!/usr/bin/env sh
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
    cargo run --profile profiling --features dhat -- "$@"

# Ask JP to create a new RFD based on the current conversation context.
[group('jp')]
[positional-arguments]
rfd-this *ARGS: _install-jp
    #!/usr/bin/env sh
    args="$@"
    msg="I gave you the RFD skill, use it to codify all that we just discussed and concluded in a feature request RFD."

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --cfg=skill/rfd $args

# Review a GitHub pull request, queueing inline comments to a draft review.
#
# Each comment is added one at a time and prompts you to approve or reject
# it before it is posted. The review remains PENDING (only visible to the
# authenticating user, via `JP_GITHUB_TOKEN` or `GITHUB_TOKEN`) until you
# submit it from the GitHub UI.
[group('jp')]
[positional-arguments]
pr-review NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    case "{{NNN}}" in
        ''|*[!0-9]*)
            echo "Invalid PR number '{{NNN}}'. Pass a positive integer." >&2
            exit 1 ;;
    esac

    shift # remove NNN from positional params
    args="$@"
    msg="Review GitHub pull request #{{NNN}} in dcdpr/jp. Follow your review \
    workflow: enumerate the PR, read every changed file's diff, cross-reference \
    where useful, then call github_pr_review_add_comment with pull_number=\
    {{NNN}} once for EACH finding. After all comments are queued, write a \
    final markdown overview summarizing your review (counts per category, \
    overall take, mergeability). Do NOT submit the review yourself — leave \
    it as a draft."

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    title="pr-review:{{NNN}}"

    # Look up an existing active conversation with this exact title.
    existing=$(jp -F json conversation ls 2>/dev/null \
        | jq -r --arg t "$title" 'first(.[] | select(.Title == $t) | .ID) // empty' \
        2>/dev/null \
        || true)

    resume=false
    if [ -n "$existing" ]; then
        if [ -t 0 ] && [ -t 1 ]; then
            printf "Found existing review conversation %s for PR #{{NNN}}.\n" "$existing" >&2
            printf "  [c]ontinue / [n]ew (archive old) / [q]uit: " >&2
            read -r choice
        else
            choice=c
        fi
        case "$choice" in
            ""|c|C)
                resume=true ;;
            n|N)
                jp conversation archive "$existing" >&2 || true
                ;;
            q|Q)
                exit 0 ;;
            *)
                echo "Unknown choice '$choice'; aborting." >&2
                exit 1 ;;
        esac
    fi

    if [ "$resume" = "true" ]; then
        printf "Resuming review on PR #{{NNN}} (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/pr-reviewer \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews" \
            $args
    else
        printf "Reviewing PR #{{NNN}}\n\n" >&2
        jp query --new --title "$title" --cfg=personas/pr-reviewer \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews" \
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
# Reuse the same `pr-triage:NNN` conversation across multiple review
# cycles so the codebase context doesn't have to be rediscovered each
# round. To start over, archive the existing conversation
# (`jp conversation rm <id>`) and re-run.
[group('jp')]
[positional-arguments]
pr-triage NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    case "{{NNN}}" in
        ''|*[!0-9]*)
            echo "Invalid PR number '{{NNN}}'. Pass a positive integer." >&2
            exit 1 ;;
    esac

    shift # remove NNN from positional params
    args="$@"
    msg="Triage the reviews on GitHub pull request #{{NNN}} in dcdpr/jp. \
    For each review comment, write one numbered item containing: the \
    comment's \`id=<n>\` from the attached reviews, a short quote of \
    the reviewer's point, a verdict (\`Accept\`, \`Amend\`, \`Dismiss\`, \
    or \`Defer\`) with reasoning grounded in the actual code, and (when \
    accepting or amending) the concrete change you would make. Do NOT \
    edit any files and do NOT post replies yet — output the triage as \
    plain markdown only."

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    title="pr-triage:{{NNN}}"

    # Look up an existing active conversation with this exact title;
    # resume it if found, otherwise create a fresh one.
    existing=$(jp -F json conversation ls 2>/dev/null \
        | jq -r --arg t "$title" 'first(.[] | select(.Title == $t) | .ID) // empty' \
        2>/dev/null \
        || true)

    if [ -n "$existing" ]; then
        printf "Resuming triage on PR #{{NNN}} (%s)\n\n" "$existing" >&2
        jp query --id "$existing" --cfg=personas/pr-triager \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews" \
            $args
    else
        printf "Triaging PR #{{NNN}}\n\n" >&2
        jp query --new --title "$title" --cfg=personas/pr-triager \
            --attach "gh:pull/{{NNN}}/diff" \
            --attach "gh:pull/{{NNN}}/reviews" \
            $args
    fi

# Review an RFD. Accepts a permanent number (41, 041) or a draft ID (D01).
[group('rfd')]
[positional-arguments]
rfd-review NNN *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    shift # remove NNN from positional params
    args="$@"
    msg="Please review the attached RFD. Review the RFD in isolation, \
    including its explicit dependencies, or any implicit dependencies, but \
    keep in mind that Draft RFDs are still in the design phase, and Discussion \
    RFDs are aspirational, but not necessarily final, so any inconsistencies \
    against those should be noted, but not blockers."

    arg="{{NNN}}"
    if echo "$arg" | grep -qiE '^D[0-9]+$'; then
        draft_id=$(echo "$arg" | tr '[:lower:]' '[:upper:]')
        file=$(ls docs/rfd/drafts/${draft_id}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No draft RFD found with ID ${draft_id}." >&2; exit 1
        fi
    elif echo "$arg" | grep -qE '^[0-9]+$'; then
        n=$(echo "$arg" | sed 's/^0*//')
        num=$(printf "%03d" "${n:-0}")
        file=$(ls docs/rfd/${num}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No RFD found with number ${num}." >&2; exit 1
        fi
    else
        echo "Invalid argument '${arg}'. Use a number (41) or draft ID (D01)." >&2; exit 1
    fi

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    printf "Reviewing $file\n\n" >&2
    jp query --attach "$file" --new --cfg=personas/rfd-reviewer $args

# Triage feedback on an RFD from a reviewer conversation.
#
# NNN is the RFD (permanent number like 41/041, or draft ID like D01).
# MODE is either `new` (start a fresh triage conversation) or `continue`
# (append to the current session, e.g. to follow up on the implementation
# conversation that produced the RFD).
# CONVO is the conversation ID of the `rfd-review` run to pull feedback from.
# Only the final assistant response of that conversation is attached.
[group('rfd')]
[positional-arguments]
rfd-triage NNN MODE CONVO *ARGS: _install-jp
    #!/usr/bin/env sh
    set -eu

    shift 3 # remove NNN, MODE, CONVO from positional params
    args="$@"
    msg="I received feedback on the RFD. Read the attached reviewer response \
    carefully, then triage it item by item. Ground each point against the code \
    and related RFDs. Do not assume the feedback is correct. For each item \
    give a verdict (accept / amend / dismiss / defer) with reasoning, and for \
    accepted or amended items describe the concrete change you would make to \
    the RFD. Do NOT edit the RFD yet; give your opinion first."

    # Resolve the target RFD file.
    arg="{{NNN}}"
    if echo "$arg" | grep -qiE '^D[0-9]+$'; then
        draft_id=$(echo "$arg" | tr '[:lower:]' '[:upper:]')
        file=$(ls docs/rfd/drafts/${draft_id}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No draft RFD found with ID ${draft_id}." >&2; exit 1
        fi
    elif echo "$arg" | grep -qE '^[0-9]+$'; then
        n=$(echo "$arg" | sed 's/^0*//')
        num=$(printf "%03d" "${n:-0}")
        file=$(ls docs/rfd/${num}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No RFD found with number ${num}." >&2; exit 1
        fi
    else
        echo "Invalid argument '${arg}'. Use a number (41) or draft ID (D01)." >&2; exit 1
    fi

    # Resolve MODE. Explicit to avoid silently picking a default.
    case "{{MODE}}" in
        new)      new_flag="--new" ;;
        continue) new_flag="" ;;
        *)
            echo "Invalid MODE '{{MODE}}'. Use 'new' or 'continue'." >&2
            exit 1 ;;
    esac

    starts_with() { case ${2-} in "$1"*) true;; *) false;; esac; }
    contains() { case ${2-} in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    printf "Triaging feedback on $file (mode: {{MODE}})\n\n" >&2
    jp query \
        --attach "file://$file" \
        --attach "jp://{{CONVO}}?select=a" \
        $new_flag \
        --cfg=personas/rfd-triager \
        $args

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
    draft_id=$(printf "D%02d" "$next")

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
rfd-promote NNN: _install-jp
    #!/usr/bin/env sh
    set -eu

    arg="{{NNN}}"

    # --- Resolve the RFD file from the argument. ---
    if echo "$arg" | grep -qiE '^D[0-9]+$'; then
        # Draft ID (e.g. D01, D12).
        draft_id=$(echo "$arg" | tr '[:lower:]' '[:upper:]')
        file=$(ls docs/rfd/drafts/${draft_id}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No draft RFD found with ID ${draft_id}." >&2; exit 1
        fi
    elif echo "$arg" | grep -qE '^[0-9]+$'; then
        # Permanent number.
        n=$(echo "$arg" | sed 's/^0*//')
        num=$(printf "%03d" "${n:-0}")
        file=$(ls docs/rfd/${num}-*.md 2>/dev/null | head -1)
        if [ -z "$file" ]; then
            echo "No RFD found with number ${num}." >&2; exit 1
        fi
    else
        echo "Invalid argument '${arg}'. Use a number (41) or draft ID (D01)." >&2; exit 1
    fi

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

    # --- Draft -> Discussion: assign permanent number, rename file ---
    if [ "$current" = "Draft" ]; then
        basename_f=$(basename "$file")
        old_draft_id=$(echo "$basename_f" | sed 's/^\(D[0-9]*\)-.*/\1/')
        slug=$(echo "$basename_f" | sed 's/^[A-Z]*[0-9]*-//; s/\.md$//')

        # Assign next available permanent number.
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
        num=$(printf "%03d" "$next_num")
        new_basename="${num}-${slug}.md"
        new_file="docs/rfd/${new_basename}"

        # Rewrite heading and status.
        sed \
            -e "s/^# RFD [A-Z]*[0-9]*:/# RFD ${num}:/" \
            -e "s/^- \*\*Status\*\*: Draft/- **Status**: Discussion/" \
            "$file" > "$new_file"
        rm "$file"

        # Update cross-references in other RFDs: replace `RFD DNN` with
        # `RFD NNN` in prose, and `DNN-slug.md` with the correct relative
        # path to `NNN-slug.md` in link targets. Drafts under `drafts/`
        # need a `../` prefix because the promoted file moved up a
        # directory.
        updated=0
        for other in docs/rfd/*.md docs/rfd/drafts/*.md; do
            [ -f "$other" ] || continue
            [ "$other" = "$new_file" ] && continue
            if ! grep -q -e "RFD ${old_draft_id}" -e "${basename_f}" "$other"; then
                continue
            fi
            if [ "$(dirname "$other")" = "docs/rfd/drafts" ]; then
                link_replacement="../${new_basename}"
            else
                link_replacement="${new_basename}"
            fi
            sed \
                -e "s|RFD ${old_draft_id}|RFD ${num}|g" \
                -e "s|${basename_f}|${link_replacement}|g" \
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
            echo "Updated ${updated} cross-reference(s) in other RFDs."
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
        echo "${file}: Accepted -> Implemented"
    fi

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

# List RFDs, optionally filtered by category.
[group('rfd')]
rfd-list *CATEGORY:
    #!/usr/bin/env sh
    set -eu

    filter="{{CATEGORY}}"

    for file in docs/rfd/[0-9][0-9][0-9]-*.md docs/rfd/drafts/D[0-9][0-9]-*.md; do
        [ -f "$file" ] || continue

        num=$(basename "$file" | sed 's/-.*//')

        # Skip templates.
        [ "$num" = "000" ] && continue
        status=$(sed -n 's/^- \*\*Status\*\*: \(.*\)/\1/p' "$file" | head -1)
        category=$(sed -n 's/^- \*\*Category\*\*: \(.*\)/\1/p' "$file" | head -1)
        title=$(sed -n 's/^# RFD [0-9A-Z]*: \(.*\)/\1/p' "$file" | head -1)

        # Append the superseding RFD number to the status.
        if [ "$status" = "Superseded" ]; then
            by=$(sed -n 's/^- \*\*Superseded by\*\*: \[RFD \([0-9]*\)\].*/\1/p' "$file" | head -1)
            [ -n "$by" ] && status="Superseded (${by})"
        fi

        # Filter by category if specified.
        if [ -n "$filter" ]; then
            match=$(echo "$category" | tr '[:upper:]' '[:lower:]')
            want=$(echo "$filter" | tr '[:upper:]' '[:lower:]')
            [ "$match" = "$want" ] || continue
        fi

        printf "%s  %-16s %-12s %s\n" "$num" "$status" "$category" "$title"
    done

# Locally develop the documentation, with hot-reloading.
[group('docs')]
develop-docs *FLAGS="--open": rfd-summaries
    just _docs "dev" {{FLAGS}}

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
[group('plugins')]
plugin-registry-fetch:
    #!/usr/bin/env sh
    set -eu
    curl -fL https://raw.githubusercontent.com/dcdpr/jp/plugin-registry/plugins.json \
        -o docs/registry/plugins.json

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
    cargo clippy --workspace --all-targets --all-features --no-deps --profile=lint -- --deny warnings

# Check code formatting on CI.
[group('ci')]
fmt-ci: (_rustup_component "rustfmt") _install_ci_matchers
    cargo fmt --all --check

# Test the code on CI.
[group('ci')]
test-ci: (_install "cargo-nextest@" + nextest_version) _install_ci_matchers
    @just test --workspace --no-fail-fast

# Generate documentation on CI.
[group('ci')]
docs-ci: _install_ci_matchers
    #!/usr/bin/env sh
    export RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links -D rustdoc::private-intra-doc-links -D rustdoc::invalid-codeblock-attributes -D rustdoc::invalid-html-tags -D rustdoc::invalid-rust-codeblocks -D rustdoc::bare-urls -D rustdoc::unescaped-backticks -D rustdoc::redundant-explicit-links"
    cargo doc --workspace --profile=docs --all-features --keep-going --document-private-items --no-deps

# Generate code coverage on CI.
[group('ci')]
coverage-ci: _coverage-setup _install_ci_matchers
    cargo llvm-cov --no-cfg-coverage --no-cfg-coverage-nightly --cargo-profile=coverage --no-report nextest
    cargo llvm-cov --no-cfg-coverage --no-cfg-coverage-nightly --profile=coverage --no-report --doc
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

@_install-jp *args:
    cargo install {{quiet_flag}} --locked --path crates/jp_cli {{args}}

@_install-binstall:
    command -v cargo-binstall >/dev/null 2>&1 || { \
        curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | BINSTALL_VERSION={{binstall_version}} sh; \
    }

[working-directory: 'docs']
@_docs-install:
    yarn install --immutable

@_rustup_component +COMPONENTS:
    rustup component add {{COMPONENTS}}
