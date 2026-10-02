use camino_tempfile::{Utf8TempDir, tempdir};
use pretty_assertions::assert_eq;

use super::*;

const TITLES: &str = "#!/bin/sh\n# jp-plugin/v1 \
                      {\"protocol\":1,\"description\":\"Titles\",\"command\":[\"titles\"]}\n";

/// The file name a plugin named `name` has on this platform.
///
/// These files are only scanned, never run, so a script's text serves on
/// Windows too.
fn plugin_file(name: &str) -> String {
    format!("jp-{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Write an executable file.
fn script(dir: &Utf8Path, file: &str, content: &str) -> Utf8PathBuf {
    let path = dir.join(file);
    fs::write(&path, content).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    path
}

fn titles() -> Manifest {
    Manifest {
        protocol: 1,
        description: "Titles".to_owned(),
        command: vec!["titles".to_owned()],
    }
}

fn dirs() -> (Utf8TempDir, Utf8PathBuf, Utf8PathBuf) {
    let tmp = tempdir().unwrap();
    let install = tmp.path().join("install");
    let path = tmp.path().join("bin");
    fs::create_dir_all(&install).unwrap();
    fs::create_dir_all(&path).unwrap();
    (tmp, install, path)
}

#[test]
fn a_plugin_is_found_with_its_manifest() {
    let (_tmp, install, bin) = dirs();
    let path = script(&bin, &plugin_file("titles"), TITLES);

    let found = discover_in(Some(&install), &[bin]);

    assert_eq!(found, vec![LocalPlugin {
        name: "titles".to_owned(),
        path,
        location: Location::Path,
        manifest: ManifestState::Valid(titles()),
    }]);
}

#[test]
fn a_binary_without_a_manifest_is_found_and_says_so() {
    let (_tmp, install, bin) = dirs();
    script(&bin, &plugin_file("tools"), "#!/bin/sh\necho hi\n");

    let found = discover_in(Some(&install), &[bin]);

    assert_eq!(found[0].manifest, ManifestState::Missing);
    assert_eq!(
        found[0].manifest.problem().as_deref(),
        Some("no plugin manifest")
    );
}

#[test]
fn files_that_are_not_plugins_are_skipped() {
    let (_tmp, install, bin) = dirs();
    script(&bin, "titles", TITLES);
    script(&bin, "jp-", TITLES);
    fs::create_dir(bin.join("jp-dir")).unwrap();

    assert_eq!(discover_in(Some(&install), &[bin]), vec![]);
}

#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_run_is_skipped() {
    let (_tmp, install, bin) = dirs();
    fs::write(bin.join("jp-titles"), TITLES).unwrap();

    assert_eq!(discover_in(Some(&install), &[bin]), vec![]);
}

#[test]
fn the_install_directory_comes_first() {
    let (_tmp, install, bin) = dirs();
    script(&bin, &plugin_file("a"), TITLES);
    script(&install, &plugin_file("b"), TITLES);

    let found = discover_in(Some(&install), &[bin]);
    let names: Vec<_> = found
        .iter()
        .map(|p| (p.name.as_str(), p.location))
        .collect();

    assert_eq!(names, [("b", Location::InstallDir), ("a", Location::Path)]);
}

/// Two copies with one name would share one configuration and one approval, so
/// both are reported, for the router to refuse.
#[test]
fn two_files_with_one_name_are_both_found() {
    let (_tmp, install, bin) = dirs();
    script(&install, &plugin_file("titles"), TITLES);
    script(&bin, &plugin_file("titles"), TITLES);

    let found = discover_in(Some(&install), &[bin]);

    assert_eq!(found.len(), 2);
}

/// A relative `$PATH` entry is resolved against the current directory, so the
/// path names the same file however the plugin's working directory is set
/// later.
///
/// Unix only: the relative spelling climbs to the root with `..`, and on
/// Windows the temporary directory can sit on another drive than the current
/// one, where no relative path reaches it.
#[cfg(unix)]
#[test]
fn a_relative_directory_yields_an_absolute_path() {
    let (_tmp, install, bin) = dirs();
    let written = script(&bin, &plugin_file("titles"), TITLES);

    let cwd = Utf8PathBuf::try_from(std::env::current_dir().unwrap()).unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative = Utf8PathBuf::from(format!("{up}{}", bin.as_str().trim_start_matches('/')));
    assert!(relative.is_relative());

    let found = discover_in(Some(&install), &[relative]);

    assert_eq!(found.len(), 1);
    assert!(found[0].path.is_absolute(), "{}", found[0].path);
    assert_eq!(
        found[0].path.canonicalize_utf8().unwrap(),
        written.canonicalize_utf8().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn a_symlink_to_a_file_already_found_is_the_same_binary() {
    let (tmp, install, bin) = dirs();
    let store = tmp.path().join("store");
    fs::create_dir(&store).unwrap();
    let target = script(&store, "jp-titles", TITLES);
    std::os::unix::fs::symlink(&target, bin.join("jp-titles")).unwrap();

    let found = discover_in(Some(&install), &[bin.clone(), bin, store]);

    assert_eq!(found.len(), 1);
}
