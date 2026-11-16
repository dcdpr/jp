use std::collections::BTreeMap;

use camino_tempfile::Utf8TempDir;

use super::*;

#[test]
fn test_a_value_survives_a_restart() {
    let dir = Utf8TempDir::new().unwrap();
    let file = dir.path().join("jp/dash/repo.jp.json");
    let value = BTreeMap::from([("/repo/a".to_owned(), 7_u32)]);

    store(&file, &value).unwrap();

    assert_eq!(load::<BTreeMap<String, u32>>(&file), value);
}

#[test]
fn test_a_missing_or_broken_file_is_empty() {
    let dir = Utf8TempDir::new().unwrap();
    let file = dir.path().join("repo.jp.json");
    assert_eq!(load::<BTreeMap<String, u32>>(&file), BTreeMap::new());

    fs::write(&file, "{ not json").unwrap();
    assert_eq!(load::<BTreeMap<String, u32>>(&file), BTreeMap::new());
}

#[test]
fn test_home_follows_xdg() {
    let user = Some("/Users/jean".to_owned());
    // A temporary directory is absolute on every platform; `/tmp/cache` has no
    // drive prefix, so Windows treats it as relative.
    let xdg = Utf8TempDir::new().unwrap();

    assert_eq!(
        home(Some(xdg.path().to_string()), user.clone()),
        Some(xdg.path().to_owned())
    );
    // A relative `XDG_CACHE_HOME` is invalid and ignored.
    assert_eq!(
        home(Some("cache".to_owned()), user.clone()),
        Some(Utf8PathBuf::from("/Users/jean/.cache"))
    );
    assert_eq!(
        home(None, user),
        Some(Utf8PathBuf::from("/Users/jean/.cache"))
    );
    assert_eq!(home(None, None), None);
}

#[test]
fn test_path_names_the_file_after_the_repository_and_the_source() {
    assert_eq!(
        path(
            Utf8Path::new("/Users/jean/.cache"),
            Utf8Path::new("/Users/jean/Projects/Personal/jp.git/.bare"),
            "github"
        ),
        Utf8PathBuf::from(
            "/Users/jean/.cache/jp/dash/Users-jean-Projects-Personal-jp-git--bare.github.json"
        )
    );
}
