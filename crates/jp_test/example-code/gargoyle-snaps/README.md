# gargoyle-snaps

Always-on snapshot testing utilities with path normalization filters.

## Overview

gargoyle-snaps wraps [insta](https://insta.rs) snapshot testing with:

- **Standardized documentation** - Every snapshot requires `desc`, `test_purpose`, and `expected_output` parameters
- **Path normalization filters** - Timestamps and temp paths are replaced with placeholders for reproducible snapshots
- **Fixture concurrency checking** - Detect read/write conflicts when multiple tests share fixtures

## The Independent Verification Principle

**Accepting a snapshot must be an independent verification.**

The `expected_output` parameter must be a human-written statement of what correctness looks like, based on your understanding of the requirements—NOT derived from the code being tested.

Why this matters: If `expected_output` is generated from the code (e.g., by echoing computed results), and the code has a bug, then the guidance will also be wrong. The reviewer has no independent reference point.

## Usage

```rust
use gargoyle_snaps::{assert_snapshot_json, assert_snapshot_text, assert_snapshot_debug};

#[test]
fn test_issue_parsing() {
    let issue = parse_issue(input);

    assert_snapshot_json!(
        "parsed_issue",                              // snapshot name
        "Parse issue #42 from markdown.",            // desc: WHAT is this test about?
        "Section boundaries must be respected.",     // test_purpose: WHY does this test exist?
        "issue_id=42, description has 2 items",      // expected_output: WHAT to verify
        &issue,
    );
}
```

## Macros

### `assert_snapshot_json!`
For types implementing `Serialize`. Produces JSON snapshots.

### `assert_snapshot_text!`
For string content. Produces plain text snapshots.

### `assert_snapshot_debug!`
For types implementing `Debug` but not `Serialize`. Produces Debug-formatted snapshots.

## Filters

### `timestamp_filters()`
Normalizes various timestamp formats:
- ISO 8601 (`2025-01-17T14:30:00Z`)
- SQL datetime (`2025-01-17 14:30:00`)
- Issue timestamps (`[claude 20250117 14:30 UTC]`)

### `temp_path_filters()`
Normalizes temporary directory paths:
- macOS: `/var/folders/...` and `/private/var/folders/...`
- Linux: `/tmp.*/`
- Claude Code sandbox: `/tmp/claude/...`

### `usual_filters()`
Combines all filters for typical usage.

## Concurrency Checking

Declare file access patterns for fixture-sharing tests:

```rust
use gargoyle_snaps::{ConcurrencyInfo, Fixture, TestName, CommitHashShort, AccessMode};

inventory::submit! {
    ConcurrencyInfo {
        test_name: TestName("test_read_only"),
        fixture: Fixture {
            name: "shared_fixture",
            commit: CommitHashShort("abc1234"),
            path: fixture_path.clone(),
        },
        files: vec![
            (PathBuf::from("config.toml"), AccessMode::Read),
        ],
    }
}
```

Then verify no conflicts:

```rust
#[test]
fn verify_fixture_safety() {
    gargoyle_snaps::verify_no_conflicts().unwrap();
}
```

## Snapshot Storage

Snapshots are stored in `$CARGO_MANIFEST_DIR/snapshots/` with names derived from the test file:

```
test file: tests/integration/issue_test.rs
snapshot name: "parsed"
result: snapshots/issue__parsed.snap
```

## License

MPL-2.0
