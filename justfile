bacon_version    := "3.20.1"
binstall_version := "1.15.7"
deny_version     := "0.18.9"
expand_version   := "1.0.118"
insta_version    := "1.43.2"
jilu_version     := "0.13.2"
llvm_cov_version := "0.6.21"
nextest_version  := "0.9.108"
shear_version    := "1.6.0"

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
  just --list

[group('build')]
[no-cd]
run *ARGS:
    cargo run --package jp_cli -- {{ARGS}}

# Install the `jp` binary from your local checkout.
[group('build')]
install:
    @just quiet_flag="" _install-jp

[group('jp')]
issue-bug +ARGS="Please create a bug report for the following:\n\n": _install-jp
    jp query --new-local --tmp --cfg=personas/po --hide-reasoning --edit=true {{ARGS}}

# Create a feature request issue.
[group('jp')]
issue-feat +ARGS="Please create a feature request for the following:\n\n": _install-jp
    jp query --new-local --tmp --cfg=personas/po --hide-reasoning --edit=true {{ARGS}}

# Open a commit message in the editor, using Jean-Pierre.
[group('jp')]
[positional-arguments]
commit *ARGS: _install-jp
    #!/usr/bin/env sh
    args="$@"
    msg="Give me a commit message"

    starts_with() { case $2 in "$1"*) true;; *) false;; esac; }
    if starts_with "--" "$@"; then
    elif starts_with "-" "$@"; then
        args="$* -- $msg"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --new-local --tmp --cfg=personas/committer $args || exit 1
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
    if starts_with "--" "$@"; then
    elif starts_with "-" "$@" && ! contains "--" "$@"; then
        args="$* -- $msg"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --new-local --tmp --cfg=personas/stager $args

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

# Create a new RFD draft. STYLE is 'rfd' (design proposal) or 'adr' (decision record).
[group('rfd')]
rfd-draft STYLE +TITLE:
    #!/usr/bin/env sh
    set -eu

    style="{{STYLE}}"

    # Validate the style argument.
    case "$style" in
        rfd|adr) ;;
        *) echo "Unknown style '$style'. Use 'rfd' or 'adr'." >&2; exit 1 ;;
    esac

    # Find the next available RFD number.
    last=$(ls docs/rfd/[0-9][0-9][0-9]-*.md 2>/dev/null \
        | sed 's|.*/||; s|-.*||' \
        | sort -n \
        | tail -1 \
        || echo "000")

    # Strip leading zeros to avoid octal interpretation in POSIX sh.
    last=$(echo "$last" | sed 's/^0*//')
    last=${last:-0}
    next=$(printf "%03d" $((last + 1)))

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

    # Build the filename slug from the title.
    slug=$(echo "{{TITLE}}" | tr '[:upper:]' '[:lower:]' | tr ' ' '-' | tr -cd 'a-z0-9-')
    file="docs/rfd/${next}-${slug}.md"

    # Copy the template and fill in metadata.
    sed \
        -e "s/RFD NNN: TITLE/RFD ${next}: {{TITLE}}/" \
        -e "s/AUTHOR/${author}/" \
        -e "s/DATE/$(date +%Y-%m-%d)/" \
        "docs/rfd/000-${style}-template.md" > "$file"

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
    current=$(sed -n 's/^- \*\*Status\*\*: \([A-Za-z]*\).*/\1/p' "$old_file")
    case "$current" in
        Accepted|Implemented) ;;
        *)
            echo "Cannot supersede from '${current}'." >&2
            echo "Only Accepted or Implemented RFDs can be superseded." >&2
            exit 1 ;;
    esac

    # Update old RFD: status -> Superseded, add/update "Superseded by" link.
    awk -v new="RFD ${new_num}" '
        /^- \*\*Status\*\*:/ { print "- **Status**: Superseded"; next }
        /^- \*\*Superseded by\*\*:/ { next }
        /^- \*\*Date\*\*:/ { print; print "- **Superseded by**: " new; next }
        { print }
    ' "$old_file" > "${old_file}.tmp"
    mv "${old_file}.tmp" "$old_file"

    # Update new RFD: add/update "Supersedes" link.
    awk -v old="RFD ${old_num}" '
        /^- \*\*Supersedes\*\*:/ { next }
        /^- \*\*Date\*\*:/ { print; print "- **Supersedes**: " old; next }
        { print }
    ' "$new_file" > "${new_file}.tmp"
    mv "${new_file}.tmp" "$new_file"

    echo "${old_file}: Superseded by RFD ${new_num}"
    echo "${new_file}: Supersedes RFD ${old_num}"

# Locally develop the documentation, with hot-reloading.
[group('docs')]
develop-docs: (_docs "dev" "--open")

# Build the statically built documentation.
[group('docs')]
build-docs: (_docs "build")

# Preview the statically built documentation.
[group('docs')]
preview-docs: (_docs "preview")

# Live-check the code, using Clippy and Bacon.
[group('check')]
check *FLAGS:
    just _bacon clippy {{FLAGS}}

# Run tests, using nextest.
[group('check')]
test *FLAGS="--workspace": (_install "cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version)
    cargo nextest run --all-targets {{FLAGS}}

# Continuously run tests, using Bacon.
[group('check')]
testw *FLAGS:
    just _bacon test {{FLAGS}}

# Check for unused dependencies.
[group('check')]
shear *FLAGS="--fix": (_install "cargo-shear@" + shear_version)
    cargo shear {{FLAGS}}

_bacon CMD *FLAGS: (_install "bacon@" + bacon_version)
    @bacon {{CMD}} -- {{FLAGS}}

[group('tools')]
install-tools:
    cargo install --locked --path .config/jp/tools --debug

[group('tools')]
serve-tools CONTEXT TOOL:
    @jp-tools {{quote(CONTEXT)}} {{quote(TOOL)}}

# Run all ci tasks.
[group('ci')]
ci: build-ci lint-ci fmt-ci test-ci docs-ci coverage-ci deny-ci insta-ci shear-ci

# Build the code on CI.
[group('ci')]
build-ci: _install_ci_matchers
    cargo build --workspace --all-targets --keep-going --locked --future-incompat-report

# Lint the code on CI.
[group('ci')]
lint-ci: (_rustup_component "clippy") _install_ci_matchers
    cargo clippy --workspace --all-targets --no-deps -- --deny warnings

# Check code formatting on CI.
[group('ci')]
fmt-ci: (_rustup_component "rustfmt") _install_ci_matchers
    cargo fmt --all --check

# Test the code on CI.
[group('ci')]
test-ci: (_install "cargo-nextest@" + nextest_version) _install_ci_matchers
    @just test --no-fail-fast

# Generate documentation on CI.
[group('ci')]
docs-ci: _install_ci_matchers
    #!/usr/bin/env sh
    export RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links -D rustdoc::private-intra-doc-links -D rustdoc::invalid-codeblock-attributes -D rustdoc::invalid-html-tags -D rustdoc::invalid-rust-codeblocks -D rustdoc::bare-urls -D rustdoc::unescaped-backticks -D rustdoc::redundant-explicit-links"
    cargo doc --workspace --all-features --keep-going --document-private-items --no-deps

# Generate code coverage on CI.
[group('ci')]
coverage-ci: _coverage-ci-setup
    cargo llvm-cov --no-cfg-coverage --no-cfg-coverage-nightly --no-report nextest
    cargo llvm-cov --no-cfg-coverage --no-cfg-coverage-nightly --no-report --doc
    cargo llvm-cov report --doctests --lcov --output-path lcov.info

_coverage-ci-setup: (_rustup_component "llvm-tools") (_install "cargo-llvm-cov@" + llvm_cov_version + " cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version) _install_ci_matchers

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
    cargo install {{quiet_flag}} --locked --version {{binstall_version}} cargo-binstall

[working-directory: 'docs']
@_docs-install:
    yarn install --immutable

@_rustup_component +COMPONENTS:
    rustup component add {{COMPONENTS}}
