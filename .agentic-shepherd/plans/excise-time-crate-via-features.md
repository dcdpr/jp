# Plan: Excise `bat` and make `octocrab` an optional feature

## Context

The `time` crate has a security advisory (RUSTSEC-2026-0009) and is pulled in transitively by both `bat` (via `plist`) and `octocrab` (via `jsonwebtoken` → `simple_asn1`).

## Part 1: Replace `bat` with `syntect` — DONE

Rather than making `bat` an optional feature, we replaced it entirely with `syntect` (the library bat itself uses internally). This eliminates bat, plist, onig, and their transitive `time` dependency unconditionally.

syntect is configured with `default-features = false` and only the features needed for pure-Rust terminal highlighting: `default-syntaxes`, `default-themes`, `parsing`, `regex-fancy`. The `regex-fancy` feature uses `fancy-regex` (pure Rust) instead of `regex-onig` (Oniguruma C library). The `plist-load` feature is excluded, which is what pulled in `plist` → `time`.

### Changes made

**`Cargo.toml` (workspace root)**
- Added `syntect` to `[workspace.dependencies]` with features: `default-syntaxes`, `default-themes`, `parsing`, `regex-fancy`
- `bat` entry remains but is now unused — should be removed

**`crates/jp_term/Cargo.toml`**
- Removed `bat` dependency
- Added `syntect` with workspace features

**`crates/jp_term/src/code.rs`**
- Rewrote `format()` using syntect's `HighlightLines`, `SyntaxSet::load_defaults_newlines()`, `ThemeSet::load_defaults()`, `as_24_bit_terminal_escaped()`
- `find_syntax_by_token(lang)` replaces bat's `language()` — returns `Ok(false)` for unknown syntaxes (same behavior)
- When theme is `None`, content is pushed unformatted (same as bat's `colored_output(false)`)
- When theme name is not found in syntect's bundled themes, falls back to unformatted output
- Return type changed from `Result<bool, bat::error::Error>` to `Result<bool, syntect::Error>`

**`crates/jp_cli/Cargo.toml`**
- Removed `bat` dependency
- Added `syntect = { workspace = true }` (needed for the error type)

**`crates/jp_cli/src/error.rs`**
- `Bat(#[from] bat::error::Error)` → `SyntaxHighlight(#[from] syntect::Error)`

**`crates/jp_cli/src/cmd.rs`**
- `Bat(error)` match arm → `SyntaxHighlight(error)`
- `impl_from_error!(bat::error::Error, ...)` → `impl_from_error!(syntect::Error, ...)`

### Verified

- `cargo check --workspace` — compiles clean
- `cargo tree -p jp_cli | grep -E 'bat |plist|onig'` — all absent

**`crates/jp_config/src/style/code.rs`**
- Changed default theme from `"Monokai Extended"` to `"base16-mocha.dark"` (bundled in syntect)
- Updated doc comment to reference syntect instead of bat

**`crates/jp_config/src/snapshots/jp_config__tests__partial_app_config_default_values.snap`**
- Updated snapshot to reflect new default theme

**`Cargo.toml` (workspace root)**
- Removed `bat` from `[workspace.dependencies]`

## Part 2: Make `octocrab` an optional feature — TODO

All octocrab usage is isolated in `.config/jp/tools/src/github/`. The approach is to feature-gate it.

### Changes needed

**`.config/jp/tools/Cargo.toml`**
- Make `octocrab` optional, add a `github` feature (default-enabled):

```toml
[features]
default = ["github"]
github = ["dep:octocrab"]

[dependencies]
octocrab = { ..., optional = true }
```

**`.config/jp/tools/src/lib.rs`**
- cfg-gate the github module and add a fallback match arm:

```rust
#[cfg(feature = "github")]
mod github;
```

```rust
#[cfg(feature = "github")]
s if s.starts_with("github_") => github::run(ctx, t).await,
#[cfg(not(feature = "github"))]
s if s.starts_with("github_") => Err("GitHub tools require the `github` feature.".into()),
```

The `github.rs` module and all submodules need no changes — they are entirely compiled out by the module-level cfg gate.

### Verification

1. `cargo check --workspace` — compiles with defaults (github enabled)
2. `cargo check -p tools --no-default-features` — compiles without octocrab
3. `cargo tree -p tools --no-default-features | grep octocrab` — octocrab absent
