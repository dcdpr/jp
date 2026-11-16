use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;
use crate::NewFinding;

fn finding(rule: &str, file: &str, line: usize, symbol: &str, message: &str) -> Finding {
    Finding {
        file: file.to_owned(),
        line,
        rule: rule.to_owned(),
        symbol: symbol.to_owned(),
        message: message.to_owned(),
        pass: "main".to_owned(),
    }
}

fn titles() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("CX-004".to_owned(), "Function length".to_owned()),
        ("DRY-004".to_owned(), "Wildcard import".to_owned()),
    ])
}

fn guidance() -> Guidance {
    Guidance::from_iter([(
        "CX-004".to_owned(),
        "Return early.\nExtract a helper only when it has a name worth reading.\n".to_owned(),
    )])
}

/// Two new findings, one sharing its key with a base finding, and one finding
/// in an enforced rule.
fn failing() -> Verdict {
    Verdict {
        new: vec![
            NewFinding {
                finding: finding("CX-003", "src/a.rs", 12, "run", "Magic number 42 in run"),
                shared: Some(Shared { new: 1, of: 2 }),
            },
            NewFinding {
                finding: finding(
                    "CX-004",
                    "src/a.rs",
                    40,
                    "parse",
                    "Function length 72 in parse",
                ),
                shared: None,
            },
        ],
        enforced: vec![finding(
            "DRY-004",
            "src/b.rs",
            3,
            "",
            "wildcard import: std::io",
        )],
        base: Some(BTreeMap::from([("main".to_owned(), 1)])),
        head: BTreeMap::from([("main".to_owned(), 3)]),
        titles: titles(),
    }
}

#[test]
fn text_groups_findings_by_rule_with_guidance() {
    let out = verdict(&failing(), &guidance(), Format::Text);

    assert_eq!(out, indoc! {"
        New rustqual findings: 2

        CX-003
          src/a.rs:12 (run): Magic number 42 in run
            2 findings share this rule and symbol, 1 more than at the base.

        CX-004: Function length
          src/a.rs:40 (parse): Function length 72 in parse

          Return early.
          Extract a helper only when it has a name worth reading.

        rustqual findings in enforced rules: 1

        DRY-004: Wildcard import
          src/b.rs:3: wildcard import: std::io
    "});
}

#[test]
fn github_annotates_each_finding_with_its_guidance() {
    let out = verdict(&failing(), &guidance(), Format::Github);

    assert_eq!(out, indoc! {"
        ::error file=src/a.rs,line=12,title=CX-003::Magic number 42 in run%0A%0A2 findings share this rule and symbol, 1 more than at the base.
        ::error file=src/a.rs,line=40,title=CX-004%3A Function length::Function length 72 in parse%0A%0AReturn early.%0AExtract a helper only when it has a name worth reading.
        ::error file=src/b.rs,line=3,title=DRY-004%3A Wildcard import::wildcard import: std::io
    "});
}

#[test]
fn markdown_tabulates_findings_and_folds_guidance() {
    let out = verdict(&failing(), &guidance(), Format::Markdown);

    assert_eq!(out, indoc! {"
        ### New rustqual findings (2)

        | Location | Rule | Symbol | Message |
        | --- | --- | --- | --- |
        | `src/a.rs:12` | CX-003 | `run` | Magic number 42 in run (2 findings share this rule and symbol, 1 more than at the base.) |
        | `src/a.rs:40` | CX-004 | `parse` | Function length 72 in parse |

        <details>
        <summary>CX-004: Function length</summary>

        Return early.
        Extract a helper only when it has a name worth reading.

        </details>

        ### rustqual findings in enforced rules (1)

        | Location | Rule | Symbol | Message |
        | --- | --- | --- | --- |
        | `src/b.rs:3` | DRY-004 |  | wildcard import: std::io |
    "});
}

#[test]
fn ai_lists_findings_under_their_rule_and_guidance() {
    let out = verdict(&failing(), &guidance(), Format::Ai);

    assert_eq!(out, indoc! {"
        # New rustqual findings (2)

        ## CX-003

        - `src/a.rs:12` in `run`: Magic number 42 in run
          2 findings share this rule and symbol, 1 more than at the base.

        ## CX-004: Function length

        - `src/a.rs:40` in `parse`: Function length 72 in parse

        Return early.
        Extract a helper only when it has a name worth reading.

        # rustqual findings in enforced rules (1)

        ## DRY-004: Wildcard import

        - `src/b.rs:3`: wildcard import: std::io
    "});
}

/// The `cargo_check` tool in `.config/jp/tools` reads this output, and its
/// tests parse the same fixture.
#[test]
fn json_carries_the_findings_counts_and_rules() {
    let out = verdict(&failing(), &guidance(), Format::Json);

    assert_eq!(out, include_str!("../fixtures/verdict.json"));
}

#[test]
fn json_names_only_the_rules_with_findings() {
    let passing = Verdict {
        base: Some(BTreeMap::from([("main".to_owned(), 1)])),
        head: BTreeMap::from([("main".to_owned(), 1)]),
        titles: titles(),
        ..Verdict::default()
    };

    let out = verdict(&passing, &guidance(), Format::Json);

    assert_eq!(out, indoc! {r#"
        {
          "new": [],
          "enforced": [],
          "base": {
            "main": 1
          },
          "head": {
            "main": 1
          },
          "rules": {}
        }
    "#});
}

#[test]
fn a_passing_verdict_prints_the_counts_per_pass() {
    let passing = Verdict {
        base: Some(BTreeMap::from([
            ("dry".to_owned(), 3),
            ("main".to_owned(), 10),
        ])),
        head: BTreeMap::from([("dry".to_owned(), 3), ("main".to_owned(), 9)]),
        ..Verdict::default()
    };

    assert_eq!(
        verdict(&passing, &Guidance::default(), Format::Text),
        "No new rustqual findings. Findings per pass, base -> head: dry 3 -> 3, main 10 -> 9.\n"
    );
    assert_eq!(
        verdict(&passing, &Guidance::default(), Format::Github),
        "::notice::No new rustqual findings. Findings per pass, base -> head: dry 3 -> 3, main 10 \
         -> 9.\n"
    );
}

#[test]
fn a_passing_verdict_without_a_base_prints_the_head_counts() {
    let passing = Verdict {
        base: None,
        head: BTreeMap::from([("dry".to_owned(), 3), ("main".to_owned(), 9)]),
        ..Verdict::default()
    };

    assert_eq!(
        verdict(&passing, &Guidance::default(), Format::Text),
        "No base revision to compare against, and no rustqual findings in enforced rules. \
         Findings per pass: dry 3, main 9.\n"
    );
    assert_eq!(
        verdict(&passing, &Guidance::default(), Format::Github),
        "::notice::No base revision to compare against, and no rustqual findings in enforced \
         rules. Findings per pass: dry 3, main 9.\n"
    );
}

#[test]
fn a_report_lists_findings_as_warnings() {
    let report_ = Report {
        findings: vec![finding("CP-001", "", 0, "", "a -> b -> a")],
        titles: BTreeMap::new(),
    };

    assert_eq!(
        report(&report_, &Guidance::default(), Format::Github),
        "::warning title=CP-001::a -> b -> a\n"
    );
    assert_eq!(
        report(&report_, &Guidance::default(), Format::Text),
        indoc! {"
            rustqual findings: 1

            CP-001
              <workspace>: a -> b -> a
        "}
    );
}

#[test]
fn an_empty_report_says_so() {
    assert_eq!(
        report(&Report::default(), &Guidance::default(), Format::Text),
        "No rustqual findings.\n"
    );
}

#[test]
fn escapes_workflow_command_values() {
    assert_eq!(escape_data("50%\r\nok: a,b"), "50%25%0D%0Aok: a,b");
    assert_eq!(escape_property("50%\nok: a,b"), "50%25%0Aok%3A a%2Cb");
}

#[test]
fn escapes_table_cells() {
    assert_eq!(cell("a | b\nc"), "a \\| b c");
}

#[test]
fn names_guidance_files_after_their_rule() {
    assert_eq!(Guidance::file_name("CX-001"), "CX-001.md");
    assert_eq!(Guidance::file_name("iosp/violation"), "iosp_violation.md");
}
