use std::{fs, process::Command};

use camino_tempfile::Utf8TempDir;
use doc::format::Formatting;
use indoc::indoc;
use jp_tool::FsRule;

use super::*;

const REPOSITORY: &str = "https://github.com/o/r";

/// Run git in `dir`, failing the test when it does.
fn git(dir: &Utf8Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "user.name=Tester", "-c", "user.email=t@example.com"])
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

/// A repository whose default branch changed `crates/x.rs` from `one` to `two`,
/// then changed another file, with `origin/HEAD` on that last commit.
///
/// Returns the repository and the three commits, oldest first.
fn repository() -> (Utf8TempDir, [String; 3]) {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path();
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    fs::create_dir_all(dir.join("crates")).unwrap();

    let mut commits = vec![];
    for (path, text) in [
        ("crates/x.rs", "one\n"),
        ("crates/x.rs", "two\n"),
        ("crates/y.rs", "y\n"),
    ] {
        fs::write(dir.join(path), text).unwrap();
        git(dir, &["add", "."]);
        git(dir, &["commit", "--quiet", "-m", path]);
        commits.push(git(dir, &["rev-parse", "HEAD"]));
    }
    git(dir, &["update-ref", "refs/remotes/origin/main", "HEAD"]);
    git(dir, &[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);

    let [a, b, c] = <[String; 3]>::try_from(commits).unwrap();
    (root, [a, b, c])
}

fn paths(root: &Utf8Path) -> Paths {
    Paths {
        root: root.to_path_buf(),
        wiki: root.join("docs/wiki"),
        rfds: root.join("docs/rfd"),
        tickets: root.join("docs/ticket"),
        formatting: Formatting::default(),
    }
}

const PAGE: &str = indoc! {"
    # Notes

    See [`x`] and [the y file](../../crates/y.rs#12-14), and [a page](0b0b0b0-other.md).

    ```markdown
    [`x`]: ../../crates/x.rs#L1
    ```

    [`x`]: ../../crates/x.rs#L1
"};

#[test]
fn a_link_pins_to_the_newest_commit_holding_the_file() {
    let (root, [_, _, head]) = repository();
    let page = root.path().join("docs/wiki/0a1b2c3-notes.md");

    let (text, report) = pin(
        PAGE,
        &page,
        &paths(root.path()),
        Some(REPOSITORY),
        &Access::Unrestricted,
    );

    assert_eq!(report.left, []);
    assert_eq!(report.pinned.len(), 2);
    assert!(
        text.ends_with(&format!(
            "```markdown\n[`x`]: ../../crates/x.rs#L1\n```\n\n[`x`]: \
             {REPOSITORY}/blob/{head}/crates/x.rs#L1\n"
        )),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "[the y file]({REPOSITORY}/blob/{head}/crates/y.rs#L12-L14)"
        )),
        "{text}"
    );
    assert!(text.contains("[a page](0b0b0b0-other.md)"), "{text}");
}

/// A file the work tree holds as an older commit did pins to the newest commit
/// still holding that copy, not the one that wrote it.
#[test]
fn an_older_copy_pins_to_the_last_commit_holding_it() {
    let (root, [first, ..]) = repository();
    fs::write(root.path().join("crates/x.rs"), "one\n").unwrap();
    let page = root.path().join("docs/wiki/0a1b2c3-notes.md");

    let (text, report) = pin(
        "[x](../../crates/x.rs)\n",
        &page,
        &paths(root.path()),
        Some(REPOSITORY),
        &Access::Unrestricted,
    );

    assert_eq!(report.left, []);
    assert_eq!(
        text,
        format!("[x]({REPOSITORY}/blob/{first}/crates/x.rs)\n")
    );
}

#[test]
fn a_change_no_commit_holds_is_left_and_reported() {
    let (root, _) = repository();
    fs::write(root.path().join("crates/x.rs"), "three\n").unwrap();
    let page = root.path().join("docs/wiki/0a1b2c3-notes.md");
    let source = "[x](../../crates/x.rs)\n";

    let (text, report) = pin(
        source,
        &page,
        &paths(root.path()),
        Some(REPOSITORY),
        &Access::Unrestricted,
    );

    assert_eq!(text, source);
    assert_eq!(report.pinned, []);
    assert_eq!(report.left.len(), 1);
    assert!(report.left[0].1.contains("no commit"), "{report:?}");
}

/// Telling which commit holds a file means reading it, so a file the policy
/// keeps from the call stays unpinned, with the refusal.
#[test]
fn a_file_the_policy_hides_is_left_and_reported() {
    let (root, _) = repository();
    let page = root.path().join("docs/wiki/0a1b2c3-notes.md");
    let source = "[x](../../crates/x.rs)\n";
    let access = Access::granting(root.path(), vec![FsRule::new("docs").with_read(true)]);

    let (text, report) = pin(
        source,
        &page,
        &paths(root.path()),
        Some(REPOSITORY),
        &access,
    );

    assert_eq!(text, source);
    assert_eq!(report.left, [(
        "../../crates/x.rs".to_owned(),
        "access denied: read on crates/x.rs".to_owned()
    )]);
}

#[test]
fn without_a_repository_nothing_is_pinned() {
    let root = Utf8TempDir::new().unwrap();
    let page = root.path().join("docs/wiki/0a1b2c3-notes.md");

    let (_, report) = pin(
        "[x](../../crates/x.rs)\n",
        &page,
        &paths(root.path()),
        None,
        &Access::Unrestricted,
    );

    assert_eq!(report.left.len(), 1);
    assert!(
        report.left[0].1.contains("repository address"),
        "{report:?}"
    );
}

#[test]
fn a_permalink_reads_back() {
    let commit = "9a43a5e2dc077909a9b8fca9e58abe541b3d8cda";

    assert_eq!(
        permalink(
            &format!("{REPOSITORY}/blob/{commit}/crates/a%20b.rs#L88-L125"),
            REPOSITORY
        ),
        Some(Permalink {
            commit: commit.to_owned(),
            path: "crates/a b.rs".to_owned(),
            fragment: Some("L88-L125".to_owned()),
        })
    );
    assert_eq!(
        permalink(&format!("{REPOSITORY}/blob/main/crates/x.rs"), REPOSITORY),
        None
    );
    assert_eq!(
        permalink(
            &format!("https://github.com/o/other/blob/{commit}/x.rs"),
            REPOSITORY
        ),
        None
    );
    assert_eq!(
        permalink(&format!("{REPOSITORY}/tree/{commit}/crates/"), REPOSITORY).map(|p| p.path),
        Some("crates".to_owned())
    );
}
