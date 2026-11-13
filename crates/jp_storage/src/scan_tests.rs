use std::{fs, time::Duration};

use camino_tempfile::tempdir;
use chrono::{TimeZone as _, Utc};

use super::*;

fn stamp(size: u64, modified_secs: u64) -> FileStamp {
    FileStamp {
        size,
        modified_ns: modified_secs * 1_000_000_000,
    }
}

fn scanned(modified_secs: u64) -> ScannedDir {
    ScannedDir {
        metadata: stamp(10, modified_secs),
        base_config: Some(stamp(20, modified_secs)),
        events: stamp(30, modified_secs),
        summary: EventsSummary {
            count: 2,
            last_event_at: None,
        },
    }
}

fn stamps_of(scanned: &ScannedDir) -> DirStamps {
    DirStamps {
        metadata: Some(scanned.metadata),
        base_config: scanned.base_config,
        events: Some(scanned.events),
    }
}

#[test]
fn parse_counts_events_and_reads_the_last_timestamp() {
    let body = br#"[
        {"timestamp": "2026-01-01T10:00:00Z", "type": "config_delta", "delta": {}},
        {"timestamp": "2026-01-02T11:30:00Z", "type": "chat_request", "content": "hi"}
    ]"#;

    let summary = EventsSummary::parse(body).unwrap();

    assert_eq!(summary, EventsSummary {
        count: 2,
        last_event_at: Some(Utc.with_ymd_and_hms(2026, 1, 2, 11, 30, 0).unwrap()),
    });
}

#[test]
fn parse_of_an_empty_array_has_no_last_timestamp() {
    assert_eq!(
        EventsSummary::parse(b"[]").unwrap(),
        EventsSummary::default()
    );
}

#[test]
fn parse_keeps_an_unparseable_last_timestamp_as_none() {
    let body = br#"[{"timestamp": "yesterday", "type": "chat_request"}]"#;

    assert_eq!(EventsSummary::parse(body).unwrap(), EventsSummary {
        count: 1,
        last_event_at: None,
    });
}

#[test]
fn parse_rejects_an_event_without_a_type() {
    let body = br#"[{"timestamp": "2026-01-01T10:00:00Z"}]"#;

    assert!(EventsSummary::parse(body).is_err());
}

#[test]
fn parse_rejects_an_event_without_a_timestamp() {
    let body = br#"[{"type": "chat_request"}]"#;

    assert!(EventsSummary::parse(body).is_err());
}

#[test]
fn parse_rejects_a_non_array() {
    assert!(EventsSummary::parse(br#"{"timestamp": "x", "type": "y"}"#).is_err());
}

#[test]
fn get_requires_every_stamp_to_match() {
    let cache = ScanCache::default();
    let dir = Utf8PathBuf::from("/ws/conversations/a");
    let entry = scanned(100);
    cache.extend(
        [(dir.clone(), entry)],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    assert_eq!(cache.get(&dir, &stamps_of(&entry)), Some(entry));

    let mut events_changed = stamps_of(&entry);
    events_changed.events = Some(stamp(31, 100));
    assert_eq!(cache.get(&dir, &events_changed), None);

    let mut base_config_added = stamps_of(&entry);
    base_config_added.base_config = None;
    assert_eq!(cache.get(&dir, &base_config_added), None);

    let mut metadata_touched = stamps_of(&entry);
    metadata_touched.metadata = Some(stamp(10, 101));
    assert_eq!(cache.get(&dir, &metadata_touched), None);
}

#[test]
fn summary_only_requires_the_events_stamp_to_match() {
    let cache = ScanCache::default();
    let dir = Utf8PathBuf::from("/ws/conversations/a");
    let entry = scanned(100);
    cache.extend(
        [(dir.clone(), entry)],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    assert_eq!(cache.summary(&dir, stamp(30, 100)), Some(entry.summary));
    assert_eq!(cache.summary(&dir, stamp(30, 101)), None);
    assert_eq!(cache.summary(&dir, stamp(29, 100)), None);
}

#[test]
fn extend_leaves_out_entries_modified_inside_the_granularity_window() {
    // A file modified one second before the scan started could be rewritten
    // within the same timestamp tick on a coarse filesystem, leaving its stamp
    // unchanged, so the scan must not be cached.
    let cache = ScanCache::default();
    let old = Utf8PathBuf::from("/ws/conversations/old");
    let recent = Utf8PathBuf::from("/ws/conversations/recent");

    cache.extend(
        [(old.clone(), scanned(100)), (recent.clone(), scanned(199))],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    assert_eq!(
        cache.get(&old, &stamps_of(&scanned(100))),
        Some(scanned(100))
    );
    assert_eq!(cache.get(&recent, &stamps_of(&scanned(199))), None);
    assert_eq!(cache.summary(&recent, stamp(30, 199)), None);
}

#[test]
fn saved_entries_load_into_a_fresh_cache() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let dir = Utf8PathBuf::from("/ws/conversations/a");
    let entry = scanned(100);

    ScanCache::default().save(
        &path,
        [(dir.clone(), entry)],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    let fresh = ScanCache::default();
    fresh.load(&path);
    assert_eq!(fresh.get(&dir, &stamps_of(&entry)), Some(entry));
}

#[test]
fn save_leaves_out_entries_modified_inside_the_granularity_window() {
    // A file modified one second before the scan started could be rewritten
    // within the same timestamp tick on a coarse filesystem, so only the older
    // entry may reach disk.
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let old = Utf8PathBuf::from("/ws/conversations/old");
    let recent = Utf8PathBuf::from("/ws/conversations/recent");

    ScanCache::default().save(
        &path,
        [(old.clone(), scanned(100)), (recent.clone(), scanned(199))],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    let fresh = ScanCache::default();
    fresh.load(&path);
    assert_eq!(
        fresh.get(&old, &stamps_of(&scanned(100))),
        Some(scanned(100))
    );
    assert_eq!(fresh.get(&recent, &stamps_of(&scanned(199))), None);
}

#[test]
fn save_leaves_out_relative_directories() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let relative = Utf8PathBuf::from("ws/conversations/a");

    ScanCache::default().save(
        &path,
        [(relative.clone(), scanned(100))],
        UNIX_EPOCH + Duration::from_secs(200),
    );

    let fresh = ScanCache::default();
    fresh.load(&path);
    assert_eq!(fresh.get(&relative, &stamps_of(&scanned(100))), None);
}

#[test]
fn save_does_not_rewrite_a_file_holding_the_same_entries() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let dir = Utf8PathBuf::from("/ws/conversations/a");
    let started = UNIX_EPOCH + Duration::from_secs(200);
    let cache = ScanCache::default();

    cache.save(&path, [(dir.clone(), scanned(100))], started);
    // A sentinel only survives if the second save skips the write.
    fs::write(&path, "sentinel").unwrap();
    cache.save(&path, [(dir, scanned(100))], started);

    assert_eq!(fs::read_to_string(&path).unwrap(), "sentinel");
}

#[test]
fn save_drops_directories_missing_from_the_scan() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let kept = Utf8PathBuf::from("/ws/conversations/kept");
    let removed = Utf8PathBuf::from("/ws/conversations/removed");
    let started = UNIX_EPOCH + Duration::from_secs(200);
    let cache = ScanCache::default();

    cache.save(
        &path,
        [
            (kept.clone(), scanned(100)),
            (removed.clone(), scanned(100)),
        ],
        started,
    );
    cache.save(&path, [(kept.clone(), scanned(100))], started);

    let fresh = ScanCache::default();
    fresh.load(&path);
    assert_eq!(
        fresh.get(&kept, &stamps_of(&scanned(100))),
        Some(scanned(100))
    );
    assert_eq!(fresh.get(&removed, &stamps_of(&scanned(100))), None);
}

#[test]
fn load_ignores_a_corrupt_file() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    fs::write(&path, "{not json").unwrap();

    let cache = ScanCache::default();
    cache.load(&path);

    assert_eq!(format!("{cache:?}"), "ScanCache { dirs: 0 }");
}

#[test]
fn load_ignores_a_file_of_another_version() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join(SCAN_CACHE_FILE);
    let dir = Utf8PathBuf::from("/ws/conversations/a");
    let persisted = Persisted {
        version: SCAN_CACHE_VERSION + 1,
        dirs: BTreeMap::from([(dir.clone(), scanned(100))]),
    };
    fs::write(&path, serde_json::to_vec(&persisted).unwrap()).unwrap();

    let cache = ScanCache::default();
    cache.load(&path);

    assert_eq!(cache.get(&dir, &stamps_of(&scanned(100))), None);
}

#[test]
fn file_stamp_tracks_size_and_modification_time() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("events.json");
    fs::write(&path, "[]").unwrap();
    let pinned = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(pinned)
        .unwrap();

    assert_eq!(
        FileStamp::of(&path),
        Some(FileStamp {
            size: 2,
            modified_ns: 1_700_000_000_000_000_000,
        })
    );
    assert_eq!(FileStamp::of(&tmp.path().join("missing.json")), None);
    assert_eq!(FileStamp::of(tmp.path()), None, "a directory has no stamp");
}
