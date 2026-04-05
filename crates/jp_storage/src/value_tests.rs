use std::fs;

use camino_tempfile::tempdir;
use serde_json::json;

use super::*;

#[test]
fn write_json_creates_file() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("out.json");

    write_json(&path, &json!({"key": "value"})).unwrap();

    let content = fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed, json!({"key": "value"}));
}

#[test]
fn write_json_no_tmp_file_left_on_success() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("out.json");
    let tmp_path = Utf8PathBuf::from(format!("{path}{TMP_SUFFIX}"));

    write_json(&path, &json!(42)).unwrap();

    assert!(path.is_file());
    assert!(
        !tmp_path.exists(),
        ".tmp file should be cleaned up after rename"
    );
}

#[test]
fn write_json_preserves_original_on_write_failure() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("out.json");

    write_json(&path, &json!({"original": true})).unwrap();

    // Place a directory at the .tmp path. File::create fails on all platforms
    // when the target is an existing directory.
    let blocker = Utf8PathBuf::from(format!("{path}{TMP_SUFFIX}"));
    fs::create_dir(&blocker).unwrap();

    let result = write_json(&path, &json!({"new": true}));
    assert!(
        result.is_err(),
        "write should fail when .tmp path is a directory"
    );

    let content: serde_json::Value = read_json(&path).unwrap();
    assert_eq!(content, json!({"original": true}));
}

#[test]
fn write_json_overwrites_existing_file() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("out.json");

    write_json(&path, &json!({"v": 1})).unwrap();
    write_json(&path, &json!({"v": 2})).unwrap();

    let content: serde_json::Value = read_json(&path).unwrap();
    assert_eq!(content, json!({"v": 2}));
}

#[test]
fn write_json_creates_parent_dirs() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("a").join("b").join("out.json");

    write_json(&path, &json!("nested")).unwrap();

    let content: serde_json::Value = read_json(&path).unwrap();
    assert_eq!(content, json!("nested"));
}

#[test]
fn cleanup_tmp_files_removes_orphaned_temps() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // Create some normal files and some .tmp files.
    fs::write(dir.join("metadata.json"), "{}").unwrap();
    fs::write(dir.join("events.json"), "[]").unwrap();
    fs::write(dir.join("metadata.json.tmp"), "partial").unwrap();
    fs::write(dir.join("events.json.tmp"), "partial").unwrap();

    cleanup_tmp_files(dir);

    assert!(dir.join("metadata.json").is_file());
    assert!(dir.join("events.json").is_file());
    assert!(!dir.join("metadata.json.tmp").exists());
    assert!(!dir.join("events.json.tmp").exists());
}

#[test]
fn cleanup_tmp_files_ignores_non_tmp() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    fs::write(dir.join("data.json"), "ok").unwrap();
    fs::write(dir.join("notes.txt"), "ok").unwrap();

    cleanup_tmp_files(dir);

    assert!(dir.join("data.json").is_file());
    assert!(dir.join("notes.txt").is_file());
}

#[test]
fn cleanup_tmp_files_ignores_tmp_directories() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();

    // A directory ending in .tmp should not be removed.
    fs::create_dir(dir.join("subdir.tmp")).unwrap();

    cleanup_tmp_files(dir);

    assert!(dir.join("subdir.tmp").is_dir());
}

#[test]
fn cleanup_tmp_files_handles_nonexistent_dir() {
    let bogus = Utf8Path::new("/tmp/jp_test_nonexistent_dir_abc123");
    // Should not panic.
    cleanup_tmp_files(bogus);
}
