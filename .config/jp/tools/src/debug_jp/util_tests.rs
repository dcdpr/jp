use camino::Utf8Path;

use super::shorten_paths;

/// The workspace root, as a report's artifact paths carry it.
const ROOT: &str = "/Users/jean/jp";

/// The artifact footer every one of these reports ends with.
#[test]
fn artifact_paths_in_the_footer_become_relative() {
    let report = shorten_paths(
        "- Trace: `/Users/jean/jp/tmp/profiling/trace-1.jsonl`\n",
        Utf8Path::new(ROOT),
    );

    assert_eq!(report, "- Trace: `tmp/profiling/trace-1.jsonl`\n");
}

/// The reason this runs over the whole report rather than over each path that
/// goes into it: a quoted stderr names whatever the subprocess was reading, and
/// nothing upstream knows those strings are paths.
#[test]
fn a_path_quoted_from_a_subprocess_is_shortened_too() {
    let report = shorten_paths(
        "  Error: failed to read /Users/jean/jp/.jp/config.toml\n",
        Utf8Path::new(ROOT),
    );

    assert_eq!(report, "  Error: failed to read .jp/config.toml\n");
}

/// Several paths on one line, which is the ordinary case for a jp trace event's
/// fields.
#[test]
fn every_path_on_a_line_is_shortened() {
    let report = shorten_paths(
        "INFO  config.load  path=/Users/jean/jp/.jp/config.toml \
         base=/Users/jean/jp/crates/jp_config\n",
        Utf8Path::new(ROOT),
    );

    assert_eq!(
        report,
        "INFO  config.load  path=.jp/config.toml base=crates/jp_config\n"
    );
}

/// A sandbox lives under the workspace, so its long timestamped path collapses
/// rather than being reported in full on every line that mentions it.
#[test]
fn the_sandbox_path_collapses() {
    let report = shorten_paths(
        "cwd=/Users/jean/jp/tmp/jp-sandbox-1785754546/crates\n",
        Utf8Path::new(ROOT),
    );

    assert_eq!(report, "cwd=tmp/jp-sandbox-1785754546/crates\n");
}

/// The root on its own still has to render as something: a field whose value
/// vanished reads as a bug in the tool rather than as the workspace root.
#[test]
fn the_root_on_its_own_becomes_a_dot() {
    let report = shorten_paths("cwd=/Users/jean/jp\n", Utf8Path::new(ROOT));

    assert_eq!(report, "cwd=.\n");
}

/// A path under nothing known is left alone: the system temp directory is where
/// a sandbox artifact can land, and a reader still has to be able to find it.
#[test]
fn a_path_outside_everything_known_is_left_alone() {
    let absolute = "/var/folders/ny/T/.tmpXYZ/trace.jsonl";
    let report = shorten_paths(&format!("- Trace: `{absolute}`\n"), Utf8Path::new(ROOT));

    assert_eq!(report, format!("- Trace: `{absolute}`\n"));
}

/// The report is markdown, and shortening must not disturb anything that is not
/// a path.
#[test]
fn text_with_no_paths_is_returned_unchanged() {
    let report = "## Hot code\n\n| Self | Share | Symbol |\n| 12 | 4.0% | `core::ptr::drop` |\n";

    assert_eq!(shorten_paths(report, Utf8Path::new(ROOT)), report);
}
