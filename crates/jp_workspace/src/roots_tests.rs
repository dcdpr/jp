use std::{fs, str::FromStr as _};

use camino::Utf8Path;
use camino_tempfile::tempdir;
use datetime_literal::datetime;
use test_log::test;

use super::*;

const STORAGE_DIR: &str = ".jp";

fn wid(s: &str) -> Id {
    Id::from_str(s).unwrap()
}

/// Create a checkout at `path` whose storage directory holds `id`.
fn write_checkout(path: &Utf8Path, id: &Id) {
    let storage = path.join(STORAGE_DIR);
    fs::create_dir_all(&storage).unwrap();
    id.store(&storage).unwrap();
}

/// Write a registry entry with a fixed `last_used`, bypassing the fresh
/// timestamp `upsert_root` records.
fn write_entry(dir: &Utf8Path, root: &Utf8Path, last_used: DateTime<Utc>) {
    let path = root.canonicalize_utf8().unwrap();
    let file = dir
        .join(ROOTS_DIR)
        .join(format!("{}.json", root_key(&path)));
    write_json(&file, &RootEntry { path, last_used }).unwrap();
}

#[test]
fn upsert_writes_one_stable_file_per_checkout() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("dir");
    let id = wid("ws123");
    let root = tmp.path().join("checkout");
    write_checkout(&root, &id);

    upsert_root(&dir, &root).unwrap();
    upsert_root(&dir, &root).unwrap();

    let files = registry_files(&dir.join(ROOTS_DIR));
    assert_eq!(files.len(), 1, "repeated upserts reuse the checkout's file");

    let entry: RootEntry = read_json(&files[0]).unwrap();
    assert_eq!(entry.path, root.canonicalize_utf8().unwrap());
}

#[test]
fn upsert_leaves_a_fresh_entry_untouched() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("dir");
    let id = wid("ws123");
    let root = tmp.path().join("checkout");
    write_checkout(&root, &id);

    let recent = Utc::now() - Duration::minutes(1);
    write_entry(&dir, &root, recent);

    upsert_root(&dir, &root).unwrap();

    let files = registry_files(&dir.join(ROOTS_DIR));
    let entry: RootEntry = read_json(&files[0]).unwrap();
    assert_eq!(
        entry.last_used, recent,
        "an entry fresher than the refresh granularity is not rewritten"
    );
}

#[test]
fn upsert_refreshes_a_stale_entry() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("dir");
    let id = wid("ws123");
    let root = tmp.path().join("checkout");
    write_checkout(&root, &id);

    let stale = Utc::now() - Duration::hours(2);
    write_entry(&dir, &root, stale);

    upsert_root(&dir, &root).unwrap();

    let files = registry_files(&dir.join(ROOTS_DIR));
    let entry: RootEntry = read_json(&files[0]).unwrap();
    assert!(
        entry.last_used > stale,
        "a stale entry gets a fresh timestamp"
    );
}

#[test]
fn upsert_heals_a_future_timestamp() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("dir");
    let id = wid("ws123");
    let root = tmp.path().join("checkout");
    write_checkout(&root, &id);

    // A timestamp ahead of the clock would otherwise pin recency ordering
    // until the clock catches up; the upsert rewrites it to now.
    let future = Utc::now() + Duration::days(1);
    write_entry(&dir, &root, future);

    upsert_root(&dir, &root).unwrap();

    let files = registry_files(&dir.join(ROOTS_DIR));
    let entry: RootEntry = read_json(&files[0]).unwrap();
    assert!(
        entry.last_used < future,
        "a future-stamped entry is rewritten to the present"
    );
}

#[cfg(unix)]
#[test]
fn upsert_records_canonical_path_for_aliased_checkout() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path().join("dir");
    let id = wid("ws123");
    let root = tmp.path().join("checkout");
    write_checkout(&root, &id);

    let alias = tmp.path().join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();

    upsert_root(&dir, &root).unwrap();
    upsert_root(&dir, &alias).unwrap();

    assert_eq!(
        registry_files(&dir.join(ROOTS_DIR)).len(),
        1,
        "an aliased path resolves to the same registry file"
    );
}

#[test]
fn resolve_returns_live_roots_and_prunes_dead_ones() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");
    let dir = workspaces_dir.join("proj-ws123");

    // One live checkout, one deleted, and one re-initialized under a
    // different workspace ID.
    let live = tmp.path().join("live");
    write_checkout(&live, &id);
    upsert_root(&dir, &live).unwrap();

    let deleted = tmp.path().join("deleted");
    write_checkout(&deleted, &id);
    upsert_root(&dir, &deleted).unwrap();
    fs::remove_dir_all(&deleted).unwrap();

    let mismatched = tmp.path().join("mismatched");
    write_checkout(&mismatched, &wid("zz999"));
    upsert_root(&dir, &mismatched).unwrap();

    let roots = resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR);

    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].path, live.canonicalize_utf8().unwrap());
    assert_eq!(
        registry_files(&dir.join(ROOTS_DIR)).len(),
        1,
        "dead and mismatched entries are pruned from disk"
    );
}

#[test]
fn resolve_orders_roots_most_recently_used_first() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");
    let dir = workspaces_dir.join("ws123");

    let older = tmp.path().join("older");
    write_checkout(&older, &id);
    let newer = tmp.path().join("newer");
    write_checkout(&newer, &id);

    write_entry(&dir, &older, datetime!(2026-01-01 10:00:00 Z));
    write_entry(&dir, &newer, datetime!(2026-06-01 10:00:00 Z));

    let roots = resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR);
    let paths: Vec<_> = roots.into_iter().map(|entry| entry.path).collect();
    assert_eq!(paths, vec![
        newer.canonicalize_utf8().unwrap(),
        older.canonicalize_utf8().unwrap(),
    ]);
}

#[test]
fn resolve_merges_roots_across_legacy_sibling_dirs() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");

    let a = tmp.path().join("checkout-a");
    write_checkout(&a, &id);
    upsert_root(&workspaces_dir.join("main-ws123"), &a).unwrap();

    let b = tmp.path().join("checkout-b");
    write_checkout(&b, &id);
    upsert_root(&workspaces_dir.join("feature-ws123"), &b).unwrap();

    assert_eq!(
        resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR).len(),
        2
    );
}

#[test]
fn resolve_prunes_unreadable_registry_files() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");
    let roots_dir = workspaces_dir.join("ws123").join(ROOTS_DIR);

    fs::create_dir_all(&roots_dir).unwrap();
    fs::write(roots_dir.join("bogus.json"), "not json").unwrap();

    assert!(resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR).is_empty());
    assert!(
        !roots_dir.join("bogus.json").exists(),
        "corrupt entry removed"
    );
}

#[test]
fn deleted_root_nested_in_same_id_workspace_is_not_live() {
    // A registry entry for `<parent>/worktree` whose directory is gone must
    // not count as live just because `<parent>` is a checkout of the same
    // workspace: liveness is a direct check, not a walk-up discovery.
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");
    let dir = workspaces_dir.join("ws123");

    let parent = tmp.path().join("parent");
    write_checkout(&parent, &id);
    let nested = parent.join("worktree");
    write_checkout(&nested, &id);
    upsert_root(&dir, &nested).unwrap();
    fs::remove_dir_all(&nested).unwrap();

    assert!(resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR).is_empty());
}

#[cfg(unix)]
mod legacy_symlink {
    use test_log::test;

    use super::*;

    #[test]
    fn seeds_registry_from_live_target_and_removes_link() {
        let tmp = tempdir().unwrap();
        let workspaces_dir = tmp.path().join("workspace");
        let id = wid("ws123");
        let dir = workspaces_dir.join("proj-ws123");
        fs::create_dir_all(&dir).unwrap();

        let checkout = tmp.path().join("checkout");
        write_checkout(&checkout, &id);

        // The legacy link points at the checkout's storage directory, not at
        // the checkout root itself.
        std::os::unix::fs::symlink(checkout.join(STORAGE_DIR), dir.join("storage")).unwrap();

        let roots = resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR);

        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].path, checkout.canonicalize_utf8().unwrap());
        assert!(
            !dir.join("storage").is_symlink(),
            "legacy link removed after seeding"
        );
    }

    #[test]
    fn drops_link_with_dead_target_without_seeding() {
        let tmp = tempdir().unwrap();
        let workspaces_dir = tmp.path().join("workspace");
        let id = wid("ws123");
        let dir = workspaces_dir.join("proj-ws123");
        fs::create_dir_all(&dir).unwrap();

        std::os::unix::fs::symlink(tmp.path().join("gone"), dir.join("storage")).unwrap();

        assert!(resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR).is_empty());
        assert!(!dir.join("storage").is_symlink(), "dead link removed");
        assert!(registry_files(&dir.join(ROOTS_DIR)).is_empty());
    }

    #[test]
    fn drops_link_with_mismatched_workspace_id_without_seeding() {
        let tmp = tempdir().unwrap();
        let workspaces_dir = tmp.path().join("workspace");
        let id = wid("ws123");
        let dir = workspaces_dir.join("proj-ws123");
        fs::create_dir_all(&dir).unwrap();

        // The link target is a live workspace, but not *this* workspace.
        let other = tmp.path().join("other");
        write_checkout(&other, &wid("zz999"));
        std::os::unix::fs::symlink(other.join(STORAGE_DIR), dir.join("storage")).unwrap();

        assert!(resolve_live_roots(&workspaces_dir, &id, STORAGE_DIR).is_empty());
        assert!(!dir.join("storage").is_symlink(), "mismatched link removed");
        assert!(registry_files(&dir.join(ROOTS_DIR)).is_empty());
    }
}

#[test]
fn user_workspace_dir_names_parse_to_id_and_optional_slug() {
    assert_eq!(
        user_workspace_id_and_slug("ws123"),
        Some((wid("ws123"), None))
    );
    assert_eq!(
        user_workspace_id_and_slug("proj-ws123"),
        Some((wid("ws123"), Some("proj".to_owned())))
    );

    // A multi-segment slug keeps everything before the final `-`.
    assert_eq!(
        user_workspace_id_and_slug("my-app-ws123"),
        Some((wid("ws123"), Some("my-app".to_owned())))
    );

    // Not valid names: bad ID length, bad characters, empty slug.
    assert_eq!(user_workspace_id_and_slug("notanid"), None);
    assert_eq!(user_workspace_id_and_slug("proj-WS123"), None);
    assert_eq!(user_workspace_id_and_slug("-ws123"), None);
    assert_eq!(user_workspace_id_and_slug(""), None);
}

#[test]
fn known_workspaces_dedupes_dirs_and_prefers_named_slug() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");
    let id = wid("ws123");

    // A legacy layout: one bare and one named directory for the same ID,
    // each holding a live checkout.
    let first = tmp.path().join("first");
    write_checkout(&first, &id);
    upsert_root(&workspaces_dir.join("ws123"), &first).unwrap();

    let second = tmp.path().join("second");
    write_checkout(&second, &id);
    upsert_root(&workspaces_dir.join("proj-ws123"), &second).unwrap();

    let known = known_workspaces(&workspaces_dir, STORAGE_DIR);

    assert_eq!(
        known.len(),
        1,
        "directories for one ID collapse to one workspace"
    );
    assert_eq!(known[0].id, id);
    assert_eq!(known[0].slug.as_deref(), Some("proj"));
    assert_eq!(
        known[0].roots.len(),
        2,
        "roots union across both directories"
    );
}

#[test]
fn known_workspaces_orders_by_recency_rootless_last() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");

    let recent = tmp.path().join("recent");
    write_checkout(&recent, &wid("aaaaa"));
    write_entry(
        &workspaces_dir.join("aaaaa"),
        &recent,
        datetime!(2026-06-01 10:00:00 Z),
    );

    let stale = tmp.path().join("stale");
    write_checkout(&stale, &wid("bbbbb"));
    write_entry(
        &workspaces_dir.join("bbbbb"),
        &stale,
        datetime!(2026-01-01 10:00:00 Z),
    );

    // Two rootless directories, to exercise the stable ID tie-break.
    fs::create_dir_all(workspaces_dir.join("ddddd")).unwrap();
    fs::create_dir_all(workspaces_dir.join("ccccc")).unwrap();

    let known = known_workspaces(&workspaces_dir, STORAGE_DIR);
    let ids: Vec<String> = known.iter().map(|w| w.id.to_string()).collect();

    assert_eq!(ids, ["aaaaa", "bbbbb", "ccccc", "ddddd"]);
    assert!(known[2].roots.is_empty());
    assert!(known[3].roots.is_empty());
}

#[test]
fn known_workspaces_skips_non_workspace_entries() {
    let tmp = tempdir().unwrap();
    let workspaces_dir = tmp.path().join("workspace");

    let checkout = tmp.path().join("checkout");
    write_checkout(&checkout, &wid("ws123"));
    upsert_root(&workspaces_dir.join("proj-ws123"), &checkout).unwrap();

    // Not user-workspace directories: a directory whose name is no ID, and a
    // plain file whose name would otherwise qualify.
    fs::create_dir_all(workspaces_dir.join("not-a-workspace")).unwrap();
    fs::write(workspaces_dir.join("zzzzz"), b"").unwrap();

    let known = known_workspaces(&workspaces_dir, STORAGE_DIR);

    assert_eq!(known.len(), 1);
    assert_eq!(known[0].id, wid("ws123"));
}

#[test]
fn known_workspaces_missing_dir_is_empty() {
    let tmp = tempdir().unwrap();
    assert!(known_workspaces(&tmp.path().join("absent"), STORAGE_DIR).is_empty());
}
