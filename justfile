bacon_version    := "3.18.0"
binstall_version := "1.15.3"
deny_version     := "0.18.4"
expand_version   := "1.0.115"
insta_version    := "1.43.1"
jilu_version     := "0.13.2"
llvm_cov_version := "0.6.18"
nextest_version  := "0.9.103"
shear_version    := "1.5.1"

quiet_flag := if env_var_or_default("CI", "") == "true" { "" } else { "--quiet" }

default:
  just --list

install:
    @just quiet_flag="" _install-jp

[group('issue')]
issue-bug +ARGS="Please create a bug report for the following:\n\n": _install-jp
    jp query --no-persist --new --cfg=personas/product-owner --hide-reasoning --edit=true {{ARGS}}

# Create a feature request issue.
[group('issue')]
issue-feat +ARGS="Please create a feature request for the following:\n\n": _install-jp
    jp query --no-persist --new --cfg=personas/product-owner --hide-reasoning --edit=true {{ARGS}}

# Open a commit message in the editor, using Jean-Pierre.
[group('git')]
[positional-arguments]
commit +ARGS="Give me a commit message": _install-jp
    #!/usr/bin/env sh
    if message=$(jp query --no-persist --new --cfg=personas/commit "$@"); then
        echo "$message" | sed -e 's/\x1b\[[0-9;]*[mGKHF]//g' | git commit --edit --file=-
    fi

# Generate changelog for the project.
build-changelog: (_install "jilu@" + jilu_version)
    @jilu

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
check *FLAGS:
    just bacon clippy {{FLAGS}}

test *FLAGS: (_install "cargo-nextest@" + nextest_version + " cargo-expand@" + expand_version)
    cargo nextest run --workspace --all-targets {{FLAGS}}

testw *FLAGS:
    just bacon test {{FLAGS}}

shear *FLAGS="--fix": (_install "cargo-shear@" + shear_version)
    cargo shear {{FLAGS}}

bacon CMD *FLAGS: (_install "bacon@" + bacon_version)
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
