use super::*;

/// Parse a TOML scalar as a `print_stderr` value.
fn parse(toml: &str) -> PrintStderr {
    #[derive(Deserialize)]
    struct Wrapper {
        print_stderr: PrintStderr,
    }

    toml::from_str::<Wrapper>(&format!("print_stderr = {toml}"))
        .expect("valid print_stderr")
        .print_stderr
}

#[test]
fn a_bool_selects_between_off_and_auto() {
    assert_eq!(parse("false"), PrintStderr::Off);
    assert_eq!(parse("true"), PrintStderr::Auto);
}

#[test]
fn a_number_is_a_row_count() {
    assert_eq!(parse("4"), PrintStderr::Rows(StderrRows { rows: 4 }));
}

#[test]
fn zero_rows_means_off() {
    // `0` and `false` are the same request, so they produce the same value
    // rather than a window nothing can render into.
    assert_eq!(parse("0"), PrintStderr::Off);
}

#[test]
fn the_keywords_parse_too() {
    assert_eq!(parse(r#""off""#), PrintStderr::Off);
    assert_eq!(parse(r#""auto""#), PrintStderr::Auto);
    assert_eq!(parse(r#""6""#), PrintStderr::Rows(StderrRows { rows: 6 }));
}

#[test]
fn an_unknown_keyword_is_rejected() {
    #[derive(Deserialize)]
    struct Wrapper {
        #[expect(dead_code)]
        print_stderr: PrintStderr,
    }

    assert!(toml::from_str::<Wrapper>(r#"print_stderr = "loads""#).is_err());
}

#[test]
fn off_is_the_only_disabled_value() {
    assert!(!PrintStderr::Off.is_enabled());
    assert!(PrintStderr::Auto.is_enabled());
    assert!(PrintStderr::Rows(StderrRows { rows: 1 }).is_enabled());
}
