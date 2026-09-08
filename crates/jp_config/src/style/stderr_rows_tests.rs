use super::*;

/// Parse a TOML scalar as a `stderr_rows` value.
fn parse(toml: &str) -> StderrRows {
    #[derive(Deserialize)]
    struct Wrapper {
        stderr_rows: StderrRows,
    }

    toml::from_str::<Wrapper>(&format!("stderr_rows = {toml}"))
        .expect("valid stderr_rows")
        .stderr_rows
}

#[test]
fn a_bool_selects_between_off_and_auto() {
    assert_eq!(parse("false"), StderrRows::Off);
    assert_eq!(parse("true"), StderrRows::Auto);
}

#[test]
fn a_number_is_a_row_count() {
    assert_eq!(parse("4"), StderrRows::Fixed(RowCount { rows: 4 }));
}

#[test]
fn zero_rows_means_off() {
    // `0` and `false` are the same request, so they produce the same value
    // rather than a window nothing can render into.
    assert_eq!(parse("0"), StderrRows::Off);
}

#[test]
fn the_keywords_parse_too() {
    assert_eq!(parse(r#""off""#), StderrRows::Off);
    assert_eq!(parse(r#""auto""#), StderrRows::Auto);
    assert_eq!(parse(r#""6""#), StderrRows::Fixed(RowCount { rows: 6 }));
}

/// `--cfg style.mcp_startup.stderr_rows=false` arrives as a string, so the
/// boolean spellings have to survive the string path as well as the bool one.
#[test]
fn the_boolean_spellings_parse_as_strings() {
    assert_eq!(parse(r#""false""#), StderrRows::Off);
    assert_eq!(parse(r#""true""#), StderrRows::Auto);
}

#[test]
fn an_unknown_keyword_is_rejected() {
    #[derive(Deserialize)]
    struct Wrapper {
        #[expect(dead_code)]
        stderr_rows: StderrRows,
    }

    assert!(toml::from_str::<Wrapper>(r#"stderr_rows = "loads""#).is_err());
}

#[test]
fn off_is_the_only_disabled_value() {
    assert!(!StderrRows::Off.is_enabled());
    assert!(StderrRows::Auto.is_enabled());
    assert!(StderrRows::Fixed(RowCount { rows: 1 }).is_enabled());
}
