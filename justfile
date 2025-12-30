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

[private]
default:
  just --list

[group('build')]
install:
    @just quiet_flag="" _install-jp

[group('jp')]
issue-bug +ARGS="Please create a bug report for the following:\n\n": _install-jp
    jp query --new --tmp --cfg=personas/product-owner --hide-reasoning --edit=true {{ARGS}}

# Create a feature request issue.
[group('jp')]
issue-feat +ARGS="Please create a feature request for the following:\n\n": _install-jp
    jp query --new --tmp --cfg=personas/product-owner --hide-reasoning --edit=true {{ARGS}}

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

    jp query --new --tmp --cfg=personas/committer $args || exit 1
    git commit --amend

[group('jp')]
[positional-arguments]
stage *ARGS: _install-jp
    #!/usr/bin/env sh
    args="$@"
    msg="Find related changes in the git diff and stage ONE set of changes in preparation for a \
    commit using the 'git_stage' tool. Follow your prompt instructions carefully."

    starts_with() { case $2 in "$1"*) true;; *) false;; esac; }
    contains() { case $2 in *"$1"*) true;; *) false;; esac; }
    if starts_with "--" "$@"; then
    elif starts_with "-" "$@" && ! contains "--" "$@"; then
        args="$* -- $msg"
    elif [ -z "$args" ]; then
        args="$msg"
    fi

    jp query --new --tmp --cfg=personas/stager $args

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

_coverage-ci-setup: (_rustup_component "llvm-tools-preview") (_install "cargo-llvm-cov@" + llvm_cov_version + " cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version) _install_ci_matchers

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
