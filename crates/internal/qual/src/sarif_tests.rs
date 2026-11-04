use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;

/// The shape rustqual 1.8.2 writes: a located result, a workspace-wide one, and
/// the rules table.
const REPORT: &str = indoc! {r#"
    {
      "$schema": "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
      "version": "2.1.0",
      "runs": [{
        "tool": {
          "driver": {
            "name": "rustqual",
            "informationUri": "https://github.com/SaschaOnTour/rustqual",
            "rules": [
              { "id": "CX-001", "shortDescription": { "text": "Cognitive complexity" } },
              { "id": "CP-001", "shortDescription": { "text": "Circular dependency" } },
              { "id": "architecture/pattern/forbid_x" }
            ]
          }
        },
        "results": [
          {
            "ruleId": "CX-001",
            "level": "warning",
            "message": { "text": "Cognitive complexity 31 in Parser::run" },
            "locations": [{
              "physicalLocation": {
                "artifactLocation": { "uri": "crates/a/src/lib.rs" },
                "region": { "startLine": 12 }
              }
            }]
          },
          {
            "ruleId": "CP-001",
            "level": "warning",
            "message": { "text": "a -> b -> a" },
            "locations": []
          }
        ]
      }]
    }
"#};

#[test]
fn reads_located_and_workspace_wide_results() {
    let sarif = parse(REPORT).unwrap();

    assert_eq!(sarif.findings, vec![
        RawFinding {
            rule: "CX-001".to_owned(),
            file: "crates/a/src/lib.rs".to_owned(),
            line: 12,
            message: "Cognitive complexity 31 in Parser::run".to_owned(),
        },
        RawFinding {
            rule: "CP-001".to_owned(),
            file: String::new(),
            line: 0,
            message: "a -> b -> a".to_owned(),
        },
    ]);
}

#[test]
fn reads_rule_titles_and_skips_rules_without_one() {
    let sarif = parse(REPORT).unwrap();

    assert_eq!(
        sarif.titles,
        BTreeMap::from([
            ("CP-001".to_owned(), "Circular dependency".to_owned()),
            ("CX-001".to_owned(), "Cognitive complexity".to_owned()),
        ])
    );
}

#[test]
fn reads_a_report_without_results() {
    let input =
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"rustqual"}},"results":[]}]}"#;

    assert_eq!(parse(input).unwrap(), Sarif::default());
}

#[test]
fn rejects_output_that_is_not_json() {
    let error = parse("No Rust source files found in .").unwrap_err();

    assert!(matches!(error, SarifError::Json(_)), "{error:?}");
}

#[test]
fn rejects_empty_output() {
    let error = parse("").unwrap_err();

    assert!(matches!(error, SarifError::Json(_)), "{error:?}");
}

#[test]
fn rejects_a_report_without_results() {
    let input = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"rustqual"}}}]}"#;

    let error = parse(input).unwrap_err();

    assert!(matches!(error, SarifError::Json(_)), "{error:?}");
}

#[test]
fn rejects_a_result_without_a_rule_id() {
    let input = r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"rustqual"}},
        "results":[{"message":{"text":"x"},"locations":[]}]}]}"#;

    let error = parse(input).unwrap_err();

    assert!(matches!(error, SarifError::Json(_)), "{error:?}");
}

#[test]
fn rejects_another_sarif_version() {
    let input = r#"{"version":"2.0.0","runs":[]}"#;

    let error = parse(input).unwrap_err();

    assert_eq!(
        error.to_string(),
        r#"SARIF version "2.0.0" is not supported; expected 2.1.0"#
    );
}

#[test]
fn rejects_a_report_with_no_run() {
    let input = r#"{"version":"2.1.0","runs":[]}"#;

    let error = parse(input).unwrap_err();

    assert_eq!(
        error.to_string(),
        "expected one run in the SARIF report, found 0"
    );
}

#[test]
fn rejects_a_report_from_another_tool() {
    let input =
        r#"{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"clippy"}},"results":[]}]}"#;

    let error = parse(input).unwrap_err();

    assert_eq!(
        error.to_string(),
        r#"the SARIF report was written by "clippy", not rustqual"#
    );
}
