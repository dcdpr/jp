use schematic::SchemaBuilder;

use super::*;
use crate::schema_shape::render;

/// Parse a TOML scalar as a `stderr_rows` value.
fn parse(toml: &str) -> WindowRows {
    #[derive(Deserialize)]
    struct Wrapper {
        stderr_rows: WindowRows,
    }

    toml::from_str::<Wrapper>(&format!("stderr_rows = {toml}"))
        .expect("valid stderr_rows")
        .stderr_rows
}

#[test]
fn a_bool_selects_between_off_and_auto() {
    assert_eq!(parse("false"), WindowRows::Off);
    assert_eq!(parse("true"), WindowRows::Auto);
}

#[test]
fn a_number_is_a_row_count() {
    assert_eq!(parse("4"), WindowRows::Fixed(RowCount { rows: 4 }));
}

#[test]
fn zero_rows_means_off() {
    // `0` and `false` are the same request, so they produce the same value
    // rather than a window nothing can render into.
    assert_eq!(parse("0"), WindowRows::Off);
}

/// `--cfg style.mcp_startup.stderr_rows=0` reaches the type as a string, and
/// has to mean what `stderr_rows = 0` means in a file.
#[test]
fn zero_rows_from_a_string_means_off() {
    assert_eq!("0".parse::<WindowRows>().unwrap(), WindowRows::Off);
}

#[test]
fn the_keywords_parse_too() {
    assert_eq!(parse(r#""off""#), WindowRows::Off);
    assert_eq!(parse(r#""auto""#), WindowRows::Auto);
    assert_eq!(parse(r#""6""#), WindowRows::Fixed(RowCount { rows: 6 }));
}

/// `--cfg style.mcp_startup.stderr_rows=false` arrives as a string, so the
/// boolean spellings have to survive the string path as well as the bool one.
#[test]
fn the_boolean_spellings_parse_as_strings() {
    assert_eq!(parse(r#""false""#), WindowRows::Off);
    assert_eq!(parse(r#""true""#), WindowRows::Auto);
}

#[test]
fn an_unknown_keyword_is_rejected() {
    #[derive(Deserialize)]
    struct Wrapper {
        #[expect(dead_code)]
        stderr_rows: WindowRows,
    }

    assert!(toml::from_str::<Wrapper>(r#"stderr_rows = "loads""#).is_err());
}

#[test]
fn an_unknown_string_names_the_accepted_values() {
    let error = "loads".parse::<WindowRows>().unwrap_err();

    assert_eq!(
        error.to_string(),
        "invalid row count `loads`: expected `off`, `auto`, `false`, `true`, or a number of rows"
    );
}

/// A consumer validating a config file against the schema has to accept every
/// form the doc comments offer: `stderr_rows = true` and `stderr_rows = 4` as
/// much as `stderr_rows = "auto"`.
#[test]
fn the_schema_lists_the_keywords_a_bool_and_a_row_count() {
    let schema = SchemaBuilder::build_root::<WindowRows>();

    assert_eq!(
        render(&schema).trim_end(),
        r#""off" (aka false) | "auto" (aka true) | bool | int"#
    );
}

#[test]
fn off_is_the_only_disabled_value() {
    assert!(!WindowRows::Off.is_enabled());
    assert!(WindowRows::Auto.is_enabled());
    assert!(WindowRows::Fixed(RowCount { rows: 1 }).is_enabled());
}

/// Conversations store their config deltas as JSON, so the serialized form is
/// part of the on-disk format.
#[test]
fn serializes_to_the_stored_form() {
    assert_eq!(serde_json::to_string(&WindowRows::Off).unwrap(), r#""off""#);
    assert_eq!(
        serde_json::to_string(&WindowRows::Auto).unwrap(),
        r#""auto""#
    );
    assert_eq!(
        serde_json::to_string(&WindowRows::Fixed(RowCount { rows: 4 })).unwrap(),
        r#"{"fixed":{"rows":4}}"#
    );
}

#[test]
fn the_stored_form_reads_back() {
    assert_eq!(
        serde_json::from_str::<WindowRows>(r#""off""#).unwrap(),
        WindowRows::Off
    );
    assert_eq!(
        serde_json::from_str::<WindowRows>(r#""auto""#).unwrap(),
        WindowRows::Auto
    );
    assert_eq!(
        serde_json::from_str::<WindowRows>(r#"{"fixed":{"rows":4}}"#).unwrap(),
        WindowRows::Fixed(RowCount { rows: 4 })
    );
}
