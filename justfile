# Open a commit message in the editor, using Jean-Pierre.
commit args="Give me a commit message": _install-jp
    #!/usr/bin/env sh
    if message=$(jp query --no-persist --new --context=commit --hide-reasoning --no-tool '{{args}}'); then
        echo "$message" | sed -e 's/\x1b\[[0-9;]*[mGKHF]//g' | git commit --edit --file=-
    fi

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
check: (_install "bacon@^3.15")
    @bacon

# Run all ci tasks.
[group('ci')]
ci: build-ci lint-ci fmt-ci test-ci docs-ci coverage-ci deny-ci

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
test-ci: (_install "cargo-nextest@^0.9") _install_ci_matchers
    cargo nextest run --workspace --all-targets --no-fail-fast

# Generate documentation on CI.
[group('ci')]
docs-ci: _install_ci_matchers
    #!/usr/bin/env sh
    export RUSTDOCFLAGS="-D rustdoc::broken-intra-doc-links -D rustdoc::private-intra-doc-links -D rustdoc::invalid-codeblock-attributes -D rustdoc::invalid-html-tags -D rustdoc::invalid-rust-codeblocks -D rustdoc::bare-urls -D rustdoc::unescaped-backticks -D rustdoc::redundant-explicit-links"
    cargo doc --workspace --all-features --keep-going --document-private-items --no-deps

# Generate code coverage on CI.
[group('ci')]
coverage-ci: (_rustup_component "llvm-tools-preview") (_install "cargo-llvm-cov@^0.6 cargo-nextest@^0.9") _install_ci_matchers
    cargo llvm-cov --no-report nextest
    cargo llvm-cov --no-report --doc
    cargo llvm-cov report --doctests --lcov --output-path lcov.info

deny-ci: (_install "cargo-deny@^0.18") _install_ci_matchers
    cargo deny check -A index-failure --hide-inclusion-graph

@_install_ci_matchers:
    echo "::add-matcher::.github/matchers.json"

[working-directory: 'docs']
@_docs CMD="dev" *FLAGS: _docs-install
    yarn vitepress {{CMD}} {{FLAGS}}

@_install +CRATES: _install-binstall
    cargo binstall --locked --quiet --disable-telemetry --no-confirm --only-signed {{CRATES}}

@_install-jp *args:
    cargo install --locked --path crates/jp_cli {{args}}

@_install-binstall:
    cargo install --locked --quiet --version ^1.12 cargo-binstall

[working-directory: 'docs']
@_docs-install:
    yarn install --immutable

@_rustup_component +COMPONENTS:
    rustup component add {{COMPONENTS}}
