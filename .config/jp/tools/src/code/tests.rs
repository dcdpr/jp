use std::path::Path;

use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

#[path = "fixture.rs"]
pub(super) mod fixture;

fn items() -> Vec<Item> {
    outline::flatten(&fixture::stream_symbols())
}

fn item(label: &str, start: usize, end: usize) -> GroupItem {
    GroupItem {
        label: label.to_owned(),
        start,
        end,
    }
}

/// Selection ranges, innermost first, as zero-based `(line, character)` pairs.
fn ranges(spans: &[(u32, u32, u32, u32)]) -> Vec<Range> {
    spans
        .iter()
        .map(|&(line, character, end_line, end_character)| Range {
            start: Position { line, character },
            end: Position {
                line: end_line,
                character: end_character,
            },
        })
        .collect()
}

#[test]
fn a_reference_runs_to_the_end_of_the_statement_it_starts() {
    // `ConversationEvent::now(ToolCallRequest {` in `event_builder.rs`, with
    // the ranges rust-analyzer nested around it: the struct literal and the
    // call around it end four lines down.
    let chain = ranges(&[
        (309, 39, 309, 39),
        (309, 39, 309, 54),
        (309, 39, 313, 17),
        (309, 38, 313, 18),
        (309, 16, 313, 18),
        (293, 17, 314, 13),
        (288, 12, 314, 13),
    ]);

    assert_eq!(statement_end(&chain, 309), 313);
}

#[test]
fn a_reference_in_a_one_line_statement_keeps_its_line() {
    // `request: ToolCallRequest,`, a parameter of a multi-line signature.
    let chain = ranges(&[
        (923, 17, 923, 17),
        (923, 17, 923, 32),
        (923, 8, 923, 32),
        (923, 8, 923, 33),
        (921, 22, 924, 5),
        (908, 4, 945, 5),
    ]);

    assert_eq!(statement_end(&chain, 923), 923);
}

#[test]
fn a_statement_past_the_cap_falls_back_to_its_largest_part_within_it() {
    // An `impl` header without a doc comment starts on the reference's line
    // and runs 30 lines; the part of it within the cap is taken instead.
    let chain = ranges(&[(5, 16, 5, 23), (5, 10, 9, 3), (5, 0, 35, 1), (0, 0, 60, 0)]);
    assert_eq!(statement_end(&chain, 5), 9);

    let chain = ranges(&[(5, 16, 5, 23), (5, 0, 35, 1), (0, 0, 60, 0)]);
    assert_eq!(statement_end(&chain, 5), 5);
}

#[test]
fn groups_references_by_their_enclosing_item() {
    let items = items();
    let lines = fixture::stream_lines();

    // `use crate::Render;`, both references on line 21, `total`, and the test.
    let groups = group_lines(&items, &lines, &[0, 21, 21, 27, 37], false);

    assert_eq!(groups, vec![
        Group {
            item: None,
            lines: vec![0],
            ends: vec![0],
            matches: vec![],
            count: 1,
            test: false,
        },
        Group {
            item: Some(item("fn <Stream as Render>::render", 20, 23)),
            lines: vec![21],
            ends: vec![21],
            matches: vec![],
            count: 2,
            test: false,
        },
        Group {
            item: Some(item("fn total", 26, 28)),
            lines: vec![27],
            ends: vec![27],
            matches: vec![],
            count: 1,
            test: false,
        },
        Group {
            item: Some(item("fn tests::counts", 35, 38)),
            lines: vec![37],
            ends: vec![37],
            matches: vec![],
            count: 1,
            test: true,
        },
    ]);
}

#[test]
fn every_reference_in_a_test_file_is_a_test_reference() {
    let items = items();
    let lines = fixture::stream_lines();

    let groups = group_lines(&items, &lines, &[27], true);

    assert_eq!(
        groups.iter().map(|group| group.test).collect::<Vec<_>>(),
        vec![true]
    );
}

#[test]
fn a_file_with_test_and_other_references_counts_toward_both() {
    let items = items();
    let lines = fixture::stream_lines();
    let mut references = Hits::default();

    tally(
        &mut references,
        &group_lines(&items, &lines, &[21, 27, 37], false),
    );
    tally(&mut references, &group_lines(&items, &lines, &[27], true));

    assert_eq!(references.outside_tests, render::Tally {
        count: 2,
        files: 1
    });
    assert_eq!(references.in_tests, render::Tally { count: 2, files: 2 });
}

#[test]
fn an_impl_block_is_labelled_by_its_own_name() {
    let items = items();
    let lines = fixture::stream_lines();

    // `impl Render for Stream {`, a reference to `Render` in an impl header.
    let groups = group_lines(&items, &lines, &[19], false);

    assert_eq!(
        groups[0].item.as_ref().map(|item| item.label.as_str()),
        Some("impl Render for Stream")
    );
}

/// Verify that the `show` enum in `symbol.toml` matches `Show` variants.
#[test]
fn tool_definition_show_enum_matches_the_code() {
    let path = Path::new("../../../.jp/mcp/tools/code/symbol.toml");
    let content = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let value: toml::Value = toml::from_str(&content).expect("valid TOML");

    let advertised = value["conversation"]["tools"]["code_symbol"]["parameters"]["show"]["items"]
        ["enum"]
        .as_array()
        .expect("`show` items have an `enum` array")
        .iter()
        .map(|value| serde_json::from_value::<Show>(json!(value.as_str().unwrap())).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(advertised, vec![
        Show::Definition,
        Show::References,
        Show::Implementations,
        Show::Members,
        Show::Callers,
        Show::Calls,
    ]);
}

fn edit(line: u32, start: u32, end: u32, new_text: &str) -> TextEdit {
    TextEdit {
        range: Range {
            start: Position {
                line,
                character: start,
            },
            end: Position {
                line,
                character: end,
            },
        },
        new_text: new_text.to_owned(),
    }
}

#[test]
fn edits_apply_from_the_end_whatever_their_order() {
    let text = "fn iter() {}\nlet a = s.iter();\nlet b = s.iter();\n";
    let edits = [
        edit(0, 3, 7, "values"),
        edit(2, 10, 14, "values"),
        edit(1, 10, 14, "values"),
    ];

    assert_eq!(
        apply_edits(text, &edits, PositionEncoding::Utf8, "iter").unwrap(),
        "fn values() {}\nlet a = s.values();\nlet b = s.values();\n"
    );
}

#[test]
fn edit_positions_count_in_the_server_encoding() {
    // `é` is two bytes and one UTF-16 unit, so `iter` starts at byte 11 and
    // UTF-16 offset 10.
    let text = "let é = s.iter();";

    assert_eq!(
        apply_edits(
            text,
            &[edit(0, 10, 14, "values")],
            PositionEncoding::Utf16,
            "iter"
        )
        .unwrap(),
        "let é = s.values();"
    );
    assert_eq!(
        apply_edits(
            text,
            &[edit(0, 11, 15, "values")],
            PositionEncoding::Utf8,
            "iter"
        )
        .unwrap(),
        "let é = s.values();"
    );
}

#[test]
fn a_shorthand_field_edit_keeps_the_old_name() {
    // Renaming field `x` to `z` turns `Self { x }` into `Self { z: x }`.
    let text = "Self { x }";

    assert_eq!(
        apply_edits(text, &[edit(0, 7, 8, "z: x")], PositionEncoding::Utf8, "x").unwrap(),
        "Self { z: x }"
    );
}

#[test]
fn an_edit_over_other_text_means_the_file_changed() {
    let text = "let a = s.next();";

    let error = apply_edits(
        text,
        &[edit(0, 10, 14, "values")],
        PositionEncoding::Utf8,
        "iter",
    )
    .unwrap_err();

    assert_eq!(
        error,
        "line 1 reads `next` where the rename expected `iter`; the file changed after \
         rust-analyzer read it. Nothing was renamed."
    );
}

#[test]
fn overlapping_edits_and_positions_past_a_line_are_refused() {
    let text = "s.iter();";

    assert_eq!(
        apply_edits(
            text,
            &[edit(0, 2, 6, "a"), edit(0, 4, 8, "b")],
            PositionEncoding::Utf8,
            "",
        )
        .unwrap_err(),
        "two edits overlap. Nothing was renamed."
    );
    assert_eq!(
        apply_edits(text, &[edit(0, 2, 40, "a")], PositionEncoding::Utf8, "iter").unwrap_err(),
        "line 1 has no character 40"
    );
}

fn request(name: &str, new_name: &str) -> RenameRequest {
    RenameRequest {
        name: name.to_owned(),
        path: None,
        line: None,
        occurrence: None,
        new_name: new_name.to_owned(),
    }
}

#[test]
fn a_batch_whose_result_depends_on_its_order_is_refused() {
    assert_eq!(
        check_renames(&[request("a", "c"), request("b", "c")]).unwrap_err(),
        "Two renames give the name `c`. Rename them in separate calls."
    );
    assert_eq!(
        check_renames(&[request("Stream::a", "b"), request("b", "a")]).unwrap_err(),
        "Renaming `b` gives it the name `a`, which renaming `Stream::a` takes away; the result \
         would depend on the order. Rename them in separate calls."
    );
    assert_eq!(
        check_renames(&[request("a", "")]).unwrap_err(),
        "`new_name` for `a` is empty. Give the new identifier."
    );
    assert_eq!(
        check_renames(&[]).unwrap_err(),
        "`renames` is empty. Give at least one rename."
    );

    assert_eq!(
        check_renames(&[request("a", "x"), request("b", "y")]),
        Ok(())
    );
}

#[test]
fn a_later_target_moves_with_the_edits_before_it() {
    // `x` at character 7 and `y` at character 15.
    let before = "fn new(x: u32, y: u32) {}\n";
    let y = Position {
        line: 0,
        character: 15,
    };

    let edits = [edit(0, 7, 8, "ax")];
    let spans = edit_spans(before, &edits, PositionEncoding::Utf8, "x").unwrap();
    let after = splice(before, &spans);
    assert_eq!(
        shift_position(before, &after, &spans, y, PositionEncoding::Utf8),
        Some(Position {
            line: 0,
            character: 16,
        })
    );

    // An edit that breaks the line moves the target to the next one.
    let edits = [edit(0, 7, 8, "a\nb")];
    let spans = edit_spans(before, &edits, PositionEncoding::Utf8, "x").unwrap();
    let after = splice(before, &spans);
    assert_eq!(
        shift_position(before, &after, &spans, y, PositionEncoding::Utf8),
        Some(Position {
            line: 1,
            character: 8,
        })
    );
}

#[test]
fn a_target_an_earlier_edit_rewrote_cannot_be_followed() {
    let before = "fn new(x: u32) {}\n";
    let edits = [edit(0, 7, 8, "ax")];
    let spans = edit_spans(before, &edits, PositionEncoding::Utf8, "x").unwrap();
    let after = splice(before, &spans);

    assert_eq!(
        shift_position(
            before,
            &after,
            &spans,
            Position {
                line: 0,
                character: 7,
            },
            PositionEncoding::Utf8,
        ),
        None
    );
}

#[test]
fn settings_merge_recursively() {
    let mut settings = rust_analyzer::settings();

    merge(
        &mut settings,
        &json!({ "cargo": { "targetDir": true }, "references": { "excludeTests": true } }),
    );

    assert_eq!(settings["cargo"], json!({ "targetDir": true }));
    assert_eq!(
        settings["references"],
        json!({ "excludeImports": true, "excludeTests": true })
    );
    assert_eq!(settings["checkOnSave"], json!(false));
}

#[test]
fn a_file_is_relative_to_the_root_it_sits_under() {
    let root = Utf8Path::new("/work");

    assert_eq!(
        relative_to(Utf8Path::new("/work/src/lib.rs"), root).as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(
        relative_to(Utf8Path::new("/elsewhere/src/lib.rs"), root),
        None
    );
}

#[cfg(windows)]
#[test]
fn a_windows_file_is_relative_to_its_verbatim_root() {
    let root = Utf8Path::new(r"\\?\C:\Users\runner\work");

    assert_eq!(
        relative_to(Utf8Path::new(r"c:\Users\runner\work\src\lib.rs"), root).as_deref(),
        Some("src/lib.rs")
    );
    assert_eq!(
        relative_to(Utf8Path::new(r"c:\Users\runner\other\src\lib.rs"), root),
        None
    );
}

fn apply_edits(
    text: &str,
    edits: &[TextEdit],
    encoding: PositionEncoding,
    old: &str,
) -> Result<String, String> {
    Ok(splice(text, &edit_spans(text, edits, encoding, old)?))
}
