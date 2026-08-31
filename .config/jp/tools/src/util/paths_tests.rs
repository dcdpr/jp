use camino::Utf8Path;

use super::{shorten, shorten_within, shortenings_from};

/// The layout of a real machine, as the failing reports showed it.
fn fixture() -> Vec<super::Shortening> {
    shortenings_from(
        Utf8Path::new("/Users/jean/Projects/jp"),
        Some("/Users/jean"),
        None,
        None,
    )
}

/// The two shapes that leaked into a real summary.
#[test]
fn names_the_variable_a_dependency_lives_under() {
    let shortenings = fixture();

    assert_eq!(
        shorten(
            "/Users/jean/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/hashbrown-0.16.1/\
             src/raw/mod.rs",
            &shortenings
        ),
        "$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/hashbrown-0.16.1/src/raw/mod.rs"
    );

    assert_eq!(
        shorten(
            "/Users/jean/.rustup/toolchains/nightly-2026-03-26-aarch64-apple-darwin/lib/rustlib/\
             src/rust/library/core/src/ub_checks.rs",
            &shortenings
        ),
        "$RUSTUP_HOME/toolchains/nightly-2026-03-26-aarch64-apple-darwin/lib/rustlib/src/rust/\
         library/core/src/ub_checks.rs"
    );
}

/// A file in the repository needs no variable: relative is shorter and is what
/// every other report here prints.
#[test]
fn a_path_in_the_repository_becomes_relative() {
    assert_eq!(
        shorten(
            "/Users/jean/Projects/jp/crates/jp_config/src/conversation/tool/style.rs",
            &fixture()
        ),
        "crates/jp_config/src/conversation/tool/style.rs"
    );
}

/// Anything else under the home directory still must not name it.
#[test]
fn any_other_path_under_home_names_home() {
    assert_eq!(
        shorten("/Users/jean/scratch/notes.rs", &fixture()),
        "$HOME/scratch/notes.rs"
    );
}

/// Ordering, not luck: the registry sits under the home directory, so a
/// shortest-first pass would report every dependency as living under `$HOME`.
#[test]
fn the_most_specific_prefix_wins() {
    let shortenings = fixture();
    let shortened = shorten("/Users/jean/.cargo/registry/src/x.rs", &shortenings);

    assert!(
        shortened.starts_with("$CARGO_HOME/"),
        "unexpected shortening: {shortened}"
    );
}

/// The explicit variables take precedence over the defaults under home, because
/// a machine that sets them does not keep them there.
#[test]
fn an_explicit_variable_is_used_over_the_default() {
    let shortenings = shortenings_from(
        Utf8Path::new("/repo"),
        Some("/Users/jean"),
        Some("/opt/cargo"),
        Some("/opt/rustup"),
    );

    assert_eq!(
        shorten("/opt/cargo/registry/src/x.rs", &shortenings),
        "$CARGO_HOME/registry/src/x.rs"
    );
    assert_eq!(
        shorten("/opt/rustup/toolchains/x/lib.rs", &shortenings),
        "$RUSTUP_HOME/toolchains/x/lib.rs"
    );

    // The default location is no longer special, so it reads as what it is.
    assert_eq!(
        shorten("/Users/jean/.cargo/registry/src/x.rs", &shortenings),
        "$HOME/.cargo/registry/src/x.rs"
    );
}

/// A plain string prefix would rewrite this against a home of `/Users/jean`,
/// which is the kind of bug that only shows up on somebody else's machine.
#[test]
fn a_prefix_only_matches_whole_components() {
    let shortenings = fixture();

    assert_eq!(
        shorten("/Users/jeanne/src/main.rs", &shortenings),
        "/Users/jeanne/src/main.rs"
    );
    assert_eq!(
        shorten("/Users/jean/Projects/jp-other/src/main.rs", &shortenings),
        "$HOME/Projects/jp-other/src/main.rs"
    );
}

/// What rustc already remapped names no machine, and neither do the SDKs.
#[test]
fn a_path_under_nothing_known_is_left_alone() {
    let shortenings = fixture();

    assert_eq!(
        shorten(
            "/rustc/80d0e4be6f15899649ba31669077c59a986f96cc/library/core/src/str/validations.rs",
            &shortenings
        ),
        "/rustc/80d0e4be6f15899649ba31669077c59a986f96cc/library/core/src/str/validations.rs"
    );
    assert_eq!(
        shorten("/Applications/Xcode.app/Contents/Developer/x", &shortenings),
        "/Applications/Xcode.app/Contents/Developer/x"
    );
}

/// An unset home leaves the dependency prefixes with nothing to fall back to,
/// and the repository still works.
#[test]
fn nothing_to_go_on_leaves_paths_alone() {
    let shortenings = shortenings_from(Utf8Path::new("/repo"), None, None, None);

    assert_eq!(shorten("/repo/src/main.rs", &shortenings), "src/main.rs");
    assert_eq!(
        shorten("/Users/jean/.cargo/x.rs", &shortenings),
        "/Users/jean/.cargo/x.rs"
    );
}

/// A home of `/` would otherwise match every absolute path and rewrite the lot.
#[test]
fn a_root_valued_variable_is_dropped_rather_than_applied() {
    let shortenings = shortenings_from(Utf8Path::new("/repo"), Some("/"), None, None);

    assert_eq!(
        shorten("/etc/passwd", &shortenings),
        "/etc/passwd",
        "a root home must not rewrite unrelated paths"
    );
}

/// A report is prose with paths somewhere inside it, so the shortening has to
/// find them mid-string. dhat renders a source location inside the frame text,
/// and there is no point at which that substring is a path rather than prose.
#[test]
fn a_source_location_inside_a_stack_frame_is_shortened() {
    let report = shorten_within(
        "> jp_conversation::event::Event::deserialize \
         (/Users/jean/.cargo/registry/src/index.crates.io-1949/serde-1.0/src/de.rs:2025:9)\n",
        &fixture(),
    );

    assert_eq!(
        report,
        "> jp_conversation::event::Event::deserialize \
         ($CARGO_HOME/registry/src/index.crates.io-1949/serde-1.0/src/de.rs:2025:9)\n"
    );
}

/// Mid-string, the trailing boundary is what keeps `/Users/jean` off the front
/// of `/Users/jeanne`.
#[test]
fn a_longer_name_is_not_shortened_mid_string() {
    let report = shorten_within("cwd=/Users/jeanne/src\n", &fixture());

    assert_eq!(report, "cwd=/Users/jeanne/src\n");
}

/// The same, for the repository root, whose replacement is empty rather than a
/// variable name: a sibling checkout must not be reported as living inside it.
#[test]
fn a_sibling_of_the_root_is_not_made_relative() {
    let report = shorten_within("cwd=/Users/jean/Projects/jp-other/src\n", &fixture());

    assert_eq!(report, "cwd=$HOME/Projects/jp-other/src\n");
}

/// The same directory name under a mounted disk is a different file, and only
/// the check on what *precedes* the match knows it.
///
/// Without that check the home directory is replaced mid-path, leaving
/// `/Volumes/Backup$HOME/...` — which names neither the backup nor the home.
#[test]
fn a_path_under_a_mounted_copy_of_home_is_left_alone() {
    let report = shorten_within(
        "- Trace: `/Volumes/Backup/Users/jean/notes.md`\n",
        &fixture(),
    );

    assert_eq!(report, "- Trace: `/Volumes/Backup/Users/jean/notes.md`\n");
}

/// A separator does not continue a component, so a path behind a URL scheme is
/// still a path starting where it appears to start.
///
/// The pair with the test above: the naive fix for one breaks the other.
#[test]
fn a_path_after_a_scheme_is_still_shortened() {
    let report = shorten_within("opened file:///Users/jean/notes.md\n", &fixture());

    assert_eq!(report, "opened file://$HOME/notes.md\n");
}

/// The path that is exactly the root has no remainder to show.
#[test]
fn the_root_itself_shortens_to_a_dot() {
    assert_eq!(shorten("/Users/jean/Projects/jp", &fixture()), ".");
    assert_eq!(shorten("/Users/jean", &fixture()), "$HOME");
}
