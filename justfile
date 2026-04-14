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

    starts_with() { case $2 in "$1"*) true;; *) false;; esac; }
    contains() { case $2 in *"$1"*) true;; *) false;; esac; }
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

    starts_with() { case $2 in "$1"*) true;; *) false;; esac; }
    contains() { case $2 in *"$1"*) true;; *) false;; esac; }
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

    starts_with() { case $2 in "$1"*) true;; *) false;; esac; }
    contains() { case $2 in *"$1"*) true;; *) false;; esac; }
    if starts_with "-- " "$@"; then
    elif starts_with "-" "$@" && ! contains "-- " "$@"; then
        args="$* -- $msg"
    elif [ -n "$args" ]; then
        args="$msg\n\n Here is additional context: $args"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --cfg=skill/rfd $args

# Create a new RFD draft. CATEGORY is 'design', 'decision', 'guide', or 'process'.
# Drafts are created as DNN-slug.md — a permanent number is assigned at Discussion.
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
        if ! ls docs/rfd/${draft_id}-*.md >/dev/null 2>&1; then
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
    slug=$(echo "{{TITLE}}" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9-')
    file="docs/rfd/${draft_id}-${slug}.md"

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
[group('rfd')]
rfd-extend NNN MMM:
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

    # Validate that the extended RFD is in Discussion or later status.
    old_status=$(sed -n 's/^- \*\*Status\*\*: \(.*\)/\1/p' "$old_file" | head -1)
    case "$old_status" in
        Discussion|Accepted|Implemented) ;;
        *)
            echo "Cannot extend RFD ${old_num} (status: '${old_status}')." >&2
            echo "Only Discussion, Accepted or Implemented RFDs can be extended." >&2
            exit 1 ;;
    esac

    # Resolve basenames for relative markdown links.
    new_basename=$(basename "$new_file")
    old_basename=$(basename "$old_file")

    # Add "Extended by: [RFD MMM](...)" to the older RFD (NNN).
    existing_eb=$(sed -n 's/^- \*\*Extended by\*\*: \(.*\)/\1/p' "$old_file" | head -1)
    new_link="[RFD ${new_num}](${new_basename})"
    if echo "$existing_eb" | grep -q "RFD ${new_num}"; then
        echo "${old_file}: already extended by RFD ${new_num}"
    elif [ -n "$existing_eb" ]; then
        sed "s/^- \*\*Extended by\*\*: .*/&, ${new_link}/" "$old_file" > "${old_file}.tmp"
        mv "${old_file}.tmp" "$old_file"
        echo "${old_file}: Extended by ${existing_eb}, RFD ${new_num}"
    else
        first_heading=$(grep -n '^## ' "$old_file" | head -1 | cut -d: -f1)
        last_meta=$(head -n "${first_heading:-9999}" "$old_file" | grep -n '^- \*\*' | tail -1 | cut -d: -f1)
        awk -v ln="$last_meta" -v eb="- **Extended by**: ${new_link}" '
            NR == ln { print; print eb; next }
            { print }
        ' "$old_file" > "${old_file}.tmp"
        mv "${old_file}.tmp" "$old_file"
        echo "${old_file}: Extended by RFD ${new_num}"
    fi

    # Add "Extends: [RFD NNN](...)" to the newer RFD (MMM).
    existing_ex=$(sed -n 's/^- \*\*Extends\*\*: \(.*\)/\1/p' "$new_file" | head -1)
    old_link="[RFD ${old_num}](${old_basename})"
    if echo "$existing_ex" | grep -q "RFD ${old_num}"; then
        echo "${new_file}: already extends RFD ${old_num}"
    elif [ -n "$existing_ex" ]; then
        sed "s/^- \*\*Extends\*\*: .*/&, ${old_link}/" "$new_file" > "${new_file}.tmp"
        mv "${new_file}.tmp" "$new_file"
        echo "${new_file}: Extends ${existing_ex}, RFD ${old_num}"
    else
        first_heading=$(grep -n '^## ' "$new_file" | head -1 | cut -d: -f1)
        last_meta=$(head -n "${first_heading:-9999}" "$new_file" | grep -n '^- \*\*' | tail -1 | cut -d: -f1)
        awk -v ln="$last_meta" -v ex="- **Extends**: ${old_link}" '
            NR == ln { print; print ex; next }
            { print }
        ' "$new_file" > "${new_file}.tmp"
        mv "${new_file}.tmp" "$new_file"
        echo "${new_file}: Extends RFD ${old_num}"
    fi

# Advance an RFD's status: Draft -> Discussion -> Accepted -> Implemented.
#
# For drafts (DNN-prefixed files), assigns the next available permanent number
# and renames the file. When promoting to Accepted, creates a GitHub tracking
# issue via `jp` and injects the link into the metadata.
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
        file=$(ls docs/rfd/${draft_id}-*.md 2>/dev/null | head -1)
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

    # --- Draft -> Discussion: assign permanent number, rename file ---
    if [ "$current" = "Draft" ]; then
        basename_f=$(basename "$file")
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
        new_file="docs/rfd/${num}-${slug}.md"

        # Rewrite heading and status.
        sed \
            -e "s/^# RFD [A-Z]*[0-9]*:/# RFD ${num}:/" \
            -e "s/^- \*\*Status\*\*: Draft/- **Status**: Discussion/" \
            "$file" > "$new_file"
        rm "$file"

        echo "${new_file}: Draft -> Discussion (assigned ${num})"

    # --- Discussion -> Accepted: create tracking issue via jp ---
    elif [ "$current" = "Discussion" ]; then
        sed "s/^- \*\*Status\*\*: Discussion/- **Status**: Accepted/" "$file" > "${file}.tmp"
        mv "${file}.tmp" "$file"

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

    for file in docs/rfd/[0-9][0-9][0-9]-*.md docs/rfd/D[0-9][0-9]-*.md; do
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

_coverage-setup: (_rustup_component "llvm-tools") (_install "cargo-llvm-cov@" + llvm_cov_version + " cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version)

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
