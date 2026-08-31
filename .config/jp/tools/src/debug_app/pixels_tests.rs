use camino::Utf8Path;
use serde_json::{Map, Value, json};

use super::{Args, Axis, Run, Scan, Window, report};
use crate::Tool;

fn scan(runs: Vec<Run>) -> Scan {
    Scan {
        width: 1800,
        height: 900,
        color_space: "kCGColorSpaceSRGB".to_owned(),
        scan: "row".to_owned(),
        at: 100,
        runs,
    }
}

fn args(axis: Axis) -> Args {
    Args {
        scan: axis,
        at: 100,
        from: None,
        to: None,
        image: None,
    }
}

fn run(start: u32, count: u32, color: &str) -> Run {
    Run {
        start,
        count,
        color: color.to_owned(),
    }
}

/// The whole point of the report: an edge, and the offset it sits at.
#[test]
fn tabulates_the_runs_along_the_scan() {
    let report = report(
        Utf8Path::new("/repo"),
        Utf8Path::new("/repo/tmp/debug-app/test/shot-1730000000123.png"),
        Some(&Window {
            id: 7412,
            title: Some("mac-app".to_owned()),
            width: 900,
            height: 450,
        }),
        &args(Axis::Row),
        &scan(vec![
            run(0, 560, "#FFFFFF"),
            run(560, 2, "#DBDBDB"),
            run(562, 1238, "#FFFFFF"),
        ]),
    );

    assert_eq!(
        report,
        "Scanned row 100 of a 1800x900 pixel image, in kCGColorSpaceSRGB.\n\nThe window is \
         900x450 points, so the image is 2x: one point is 2 pixels.\n\n| x | count | colour |\n| \
         --- | --- | --- |\n| 0 | 560 | `#FFFFFF` |\n| 560 | 2 | `#DBDBDB` |\n| 562 | 1238 | \
         `#FFFFFF` |\n\nThe capture is at `tmp/debug-app/test/shot-1730000000123.png`. Pass it as \
         `image` to scan another line of the same picture without disturbing the app.\n"
    );
}

/// A column's offsets measure down the image, not across it, and a reader
/// comparing them against a frame needs to know which.
#[test]
fn names_the_axis_the_offsets_measure() {
    let report = report(
        Utf8Path::new("/repo"),
        Utf8Path::new("/repo/shot.png"),
        None,
        &args(Axis::Column),
        &scan(vec![run(0, 900, "#1D1E20")]),
    );

    assert!(report.contains("| y | count | colour |"), "{report}");
}

/// Nothing about the retina factor is claimed when there is no window to
/// compare against: an image passed in by path may be a crop, a scaled copy, or
/// from another machine.
#[test]
fn claims_no_scale_for_an_image_it_did_not_capture() {
    let report = report(
        Utf8Path::new("/repo"),
        Utf8Path::new("/repo/shot.png"),
        None,
        &args(Axis::Row),
        &scan(vec![run(0, 1800, "#FFFFFF")]),
    );

    assert!(!report.contains("one point is"), "{report}");
}

/// An empty range is a question with an answer, and a table with no rows reads
/// as a broken report rather than as "there is nothing there".
#[test]
fn says_so_when_the_range_holds_nothing() {
    let report = report(
        Utf8Path::new("/repo"),
        Utf8Path::new("/repo/shot.png"),
        None,
        &args(Axis::Row),
        &scan(vec![]),
    );

    assert!(report.contains("*nothing in range*"), "{report}");
}

/// The exact document `jpdrive pixels` writes.
/// Nothing else checks that the two sides agree on the key names.
#[test]
fn reads_the_document_the_driver_writes() {
    let written = r##"{
      "at" : 560,
      "color_space" : "kCGColorSpaceDisplayP3",
      "height" : 900,
      "runs" : [
        {
          "color" : "#DBDBDB",
          "count" : 2,
          "start" : 560
        }
      ],
      "scan" : "column",
      "width" : 1800
    }"##;

    let scan: Scan = serde_json::from_str(written).unwrap();

    assert_eq!(scan.width, 1800);
    assert_eq!(scan.height, 900);
    assert_eq!(scan.color_space, "kCGColorSpaceDisplayP3");
    assert_eq!(scan.scan, "column");
    assert_eq!(scan.at, 560);
    assert_eq!(scan.runs.len(), 1);
    assert_eq!(scan.runs[0].start, 560);
    assert_eq!(scan.runs[0].count, 2);
    assert_eq!(scan.runs[0].color, "#DBDBDB");
}

/// A tool call carrying `arguments`, as the dispatcher hands one over.
fn called_with(arguments: Value) -> Tool {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };

    Tool {
        name: "debug_app_pixels".to_owned(),
        arguments,
        answers: Map::new(),
        options: Map::new(),
    }
}

/// The arguments arrive as JSON from the assistant, so the spellings are part
/// of the tool's contract.
#[test]
fn reads_the_arguments_the_tool_is_called_with() {
    let args = Args::from_tool(&called_with(
        json!({"scan": "column", "at": 560, "from": 0, "to": 40}),
    ))
    .unwrap();

    assert_eq!(args.scan, Axis::Column);
    assert_eq!(args.at, 560);
    assert_eq!(args.from, Some(0));
    assert_eq!(args.to, Some(40));
    assert_eq!(args.image, None);
}

#[test]
fn defaults_the_range_to_the_whole_line() {
    let args = Args::from_tool(&called_with(json!({"scan": "row", "at": 0}))).unwrap();

    assert_eq!(args.from, None);
    assert_eq!(args.to, None);
}

/// A scan with no line to read is a mistake worth reporting before anything is
/// captured.
#[test]
fn refuses_a_call_with_no_line_to_read() {
    let error = Args::from_tool(&called_with(json!({"scan": "row"})))
        .unwrap_err()
        .to_string();

    assert!(error.contains("Missing argument 'at'"), "{error}");
}
