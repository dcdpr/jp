use camino_tempfile::Utf8TempDir;
use jp_tool::FsRule;

use super::*;

/// A wiki holding an open page and a private one.
fn wiki() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    let dir = root.path().join("docs/wiki");
    fs::create_dir_all(&dir).unwrap();
    for (name, title) in [
        ("0000001-open.md", "Open"),
        ("0000002-private.md", "Private"),
    ] {
        fs::write(
            dir.join(name),
            format!("# {title}\n\n- **Status**: Reference\n"),
        )
        .unwrap();
    }

    root
}

fn id(text: &str) -> PageId {
    text.parse().unwrap()
}

#[test]
fn a_command_the_user_runs_reads_every_page() {
    let root = wiki();

    let loaded = Wiki::load(&root.path().join("docs/wiki"), &Access::Unrestricted).unwrap();

    assert_eq!(loaded.pages.len(), 2);
    assert_eq!(loaded.hidden, []);
}

/// A page the policy keeps from the call is known to exist, and nothing more.
#[test]
fn a_page_the_policy_hides_is_known_by_its_file_alone() {
    let root = wiki();
    let access = Access::granting(root.path(), vec![
        FsRule::new("docs/wiki").with_read(true),
        FsRule::new("docs/wiki/0000002-private.md").with_read(false),
    ]);

    let loaded = Wiki::load(&root.path().join("docs/wiki"), &access).unwrap();

    assert_eq!(loaded.pages.len(), 1);
    assert_eq!(loaded.hidden.len(), 1);
    assert!(loaded.exists(id("W-0000002")));
    assert_eq!(
        loaded.name_of(id("W-0000002")).as_deref(),
        Some("0000002-private")
    );
    assert_eq!(
        loaded.find(id("W-0000002")).unwrap_err(),
        "W-0000002 can't be read: access denied: read on docs/wiki/0000002-private.md."
    );
    assert_eq!(loaded.find(id("W-0000001")).unwrap().name(), "Open");
    assert_eq!(
        loaded.find(id("W-0000009")).unwrap_err(),
        "No wiki page W-0000009."
    );
}

#[test]
fn a_directory_the_policy_hides_is_an_error() {
    let root = wiki();
    let access = Access::granting(root.path(), vec![FsRule::new("docs/rfd").with_read(true)]);

    let error = Wiki::load(&root.path().join("docs/wiki"), &access).unwrap_err();

    assert_eq!(
        error,
        "The wiki directory can't be read: access denied: read on docs/wiki."
    );
}
