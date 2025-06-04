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
