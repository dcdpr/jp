use std::time::Duration;

use super::*;
use crate::debug_jp::util::profile_heap_parse::{ProgramPoint, Profile};

fn fixture_profile() -> Profile {
    Profile {
        elapsed_units: 5_000_000,
        time_unit: "instrs".into(),
        total_bytes: 5_120,
        total_blocks: 15,
        peak_bytes: 1_280,
        peak_blocks: 4,
        end_bytes: 100,
        end_blocks: 1,
        program_points: vec![
            ProgramPoint {
                total_bytes: 4_096,
                total_blocks: 10,
                peak_bytes: 1_024,
                peak_blocks: 3,
                end_bytes: 0,
                end_blocks: 0,
                frames: vec![
                    "PartialAppConfig::clone".into(),
                    "ConversationStream::extend".into(),
                    "Fork::run".into(),
                ],
            },
            ProgramPoint {
                total_bytes: 1_024,
                total_blocks: 5,
                peak_bytes: 256,
                peak_blocks: 1,
                end_bytes: 100,
                end_blocks: 1,
                frames: vec!["String::clone".into(), "config::load".into()],
            },
        ],
    }
}

fn fixture_launch() -> LaunchResult {
    LaunchResult {
        exit_code: Some(0),
        stdout: String::new(),
        stderr: String::new(),
        wall_duration: Duration::from_millis(1234),
    }
}

#[test]
fn render_includes_headline() {
    let report = render(&fixture_profile(), &fixture_launch(), &["c".into(), "fork".into()], "tmp/heap.json");
    assert!(report.contains("# jp profile · heap (dhat)"));
    assert!(report.contains("`jp c fork`"));
    assert!(report.contains("**Total allocations:** 15 blocks"));
    assert!(report.contains("**At global peak:**"));
}

#[test]
fn render_includes_hot_leaves_table() {
    let report = render(&fixture_profile(), &fixture_launch(), &["x".into()], "x.json");
    assert!(report.contains("## Hot leaves"));
    assert!(report.contains("`PartialAppConfig::clone`"));
    // 10 blocks for the bigger PP, 5 for the smaller.
    assert!(report.contains("| 10 |"));
    assert!(report.contains("| 5 |"));
}

#[test]
fn render_includes_top_stacks_with_leaf_marker() {
    let report = render(&fixture_profile(), &fixture_launch(), &["x".into()], "x.json");
    assert!(report.contains("## Hot stacks"));
    assert!(report.contains("> PartialAppConfig::clone"));
    // Non-leaf frames are not marked.
    assert!(report.contains("  ConversationStream::extend"));
}

#[test]
fn render_marks_interesting_leaf_skipping_allocator_prefix() {
    // A stack where the literal leaf is allocator plumbing but a deeper
    // frame is jp-code. The report must mark the jp frame with `>` and
    // note the skipped frames.
    let profile = Profile {
        elapsed_units: 0,
        time_unit: "instrs".into(),
        total_bytes: 100,
        total_blocks: 5,
        peak_bytes: 0,
        peak_blocks: 0,
        end_bytes: 0,
        end_blocks: 0,
        program_points: vec![ProgramPoint {
            total_bytes: 100,
            total_blocks: 5,
            peak_bytes: 0,
            peak_blocks: 0,
            end_bytes: 0,
            end_blocks: 0,
            frames: vec![
                "<alloc::alloc::Global as core::alloc::Allocator>::allocate".into(),
                "<alloc::vec::Vec<u8> as core::clone::Clone>::clone".into(),
                "<jp_config::PartialAppConfig as core::clone::Clone>::clone".into(),
                "jp_conversation::stream::ConversationStream::extend".into(),
            ],
        }],
    };
    let report = render(&profile, &fixture_launch(), &["x".into()], "x.json");

    // The jp_config frame is the interesting leaf and must be marked.
    assert!(
        report.contains("> <jp_config::PartialAppConfig as core::clone::Clone>::clone"),
        "expected jp_config frame marked as leaf; report:\n{report}"
    );
    // The allocator and Vec frames precede the jp leaf and must be skipped.
    assert!(
        report.contains("skipped 2 allocator/stdlib frames"),
        "expected skip note for 2 frames; report:\n{report}"
    );
}

#[test]
fn fmt_count_chooses_unit() {
    assert_eq!(fmt_count(42), "42");
    assert_eq!(fmt_count(9_999), "9999");
    assert_eq!(fmt_count(12_345), "12.3K");
    assert_eq!(fmt_count(2_500_000), "2.50M");
    assert_eq!(fmt_count(3_500_000_000), "3.50B");
}

#[test]
fn fmt_bytes_chooses_unit() {
    assert_eq!(fmt_bytes(512), "512 B");
    assert_eq!(fmt_bytes(2_048), "2.0 KiB");
    assert_eq!(fmt_bytes(5_242_880), "5.0 MiB");
    assert_eq!(fmt_bytes(2_147_483_648), "2.00 GiB");
}
