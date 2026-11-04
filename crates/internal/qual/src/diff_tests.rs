use indoc::indoc;
use pretty_assertions::assert_eq;

use super::*;

const POLICY: &str = indoc! {r#"
    exempt = ["iosp/violation"]
    enforced = ["DRY-004"]

    [[pass]]
    name = "dry"
    config = "dry.toml"
    rules = ["DRY-*"]

    [[pass]]
    name = "main"
    config = "config.toml"
    rules = ["*"]
"#};

fn finding(rule: &str, file: &str, symbol: &str, line: usize) -> Finding {
    Finding {
        file: file.to_owned(),
        line,
        rule: rule.to_owned(),
        symbol: symbol.to_owned(),
        message: format!("{rule} in {symbol}"),
        pass: if rule.starts_with("DRY-") {
            "dry"
        } else {
            "main"
        }
        .to_owned(),
    }
}

fn report(findings: Vec<Finding>) -> Report {
    Report {
        findings,
        titles: BTreeMap::new(),
    }
}

fn compare(base: Vec<Finding>, head: Vec<Finding>) -> Verdict {
    let policy = Policy::parse(POLICY).unwrap();
    verdict(&report(base), &report(head), &Renames::default(), &policy)
}

#[test]
fn an_unchanged_tree_passes() {
    let findings = vec![finding("CX-001", "src/a.rs", "run", 10)];

    let verdict = compare(findings.clone(), findings);

    assert!(verdict.passed());
    assert_eq!(verdict.new, vec![]);
}

#[test]
fn a_finding_the_base_lacks_fails_the_gate() {
    let base = vec![finding("CX-001", "src/a.rs", "run", 10)];
    let head = vec![
        finding("CX-001", "src/a.rs", "run", 10),
        finding("CX-004", "src/a.rs", "parse", 40),
    ];

    let verdict = compare(base, head);

    assert!(!verdict.passed());
    assert_eq!(verdict.new, vec![NewFinding {
        finding: finding("CX-004", "src/a.rs", "parse", 40),
        shared: None,
    }]);
}

#[test]
fn a_finding_that_moved_lines_is_not_new() {
    let base = vec![finding("CX-001", "src/a.rs", "run", 10)];
    let head = vec![finding("CX-001", "src/a.rs", "run", 55)];

    let verdict = compare(base, head);

    assert!(verdict.passed());
}

#[test]
fn the_same_rule_in_another_symbol_is_new() {
    let base = vec![finding("CX-003", "src/a.rs", "run", 10)];
    let head = vec![finding("CX-003", "src/a.rs", "parse", 10)];

    let verdict = compare(base, head);

    assert_eq!(verdict.new, vec![NewFinding {
        finding: finding("CX-003", "src/a.rs", "parse", 10),
        shared: None,
    }]);
}

#[test]
fn findings_sharing_a_key_are_compared_by_count() {
    let base = vec![finding("CX-003", "src/a.rs", "run", 12)];
    let head = vec![
        finding("CX-003", "src/a.rs", "run", 12),
        finding("CX-003", "src/a.rs", "run", 14),
    ];

    let verdict = compare(base, head);

    assert_eq!(verdict.new, vec![
        NewFinding {
            finding: finding("CX-003", "src/a.rs", "run", 12),
            shared: Some(Shared { new: 1, of: 2 }),
        },
        NewFinding {
            finding: finding("CX-003", "src/a.rs", "run", 14),
            shared: Some(Shared { new: 1, of: 2 }),
        },
    ]);
}

#[test]
fn fewer_findings_sharing_a_key_pass() {
    let base = vec![
        finding("CX-003", "src/a.rs", "run", 12),
        finding("CX-003", "src/a.rs", "run", 14),
    ];
    let head = vec![finding("CX-003", "src/a.rs", "run", 12)];

    let verdict = compare(base, head);

    assert!(verdict.passed());
}

#[test]
fn a_finding_in_a_renamed_file_is_not_new() {
    let policy = Policy::parse(POLICY).unwrap();
    let base = report(vec![finding("CX-001", "src/old.rs", "run", 10)]);
    let head = report(vec![finding("CX-001", "src/new.rs", "run", 10)]);
    let renames = Renames::from_name_status("R100\0src/old.rs\0src/new.rs\0").unwrap();

    let verdict = verdict(&base, &head, &renames, &policy);

    assert!(verdict.passed());
}

#[test]
fn an_exempt_rule_neither_gates_nor_counts() {
    let head = vec![finding("iosp/violation", "src/a.rs", "run", 10)];

    let verdict = compare(vec![], head);

    assert!(verdict.passed());
    assert_eq!(
        verdict.head,
        BTreeMap::from([("dry".to_owned(), 0), ("main".to_owned(), 0)])
    );
}

#[test]
fn an_enforced_rule_fails_on_a_finding_the_base_already_had() {
    let findings = vec![finding("DRY-004", "src/a.rs", "", 3)];

    let verdict = compare(findings.clone(), findings);

    assert!(!verdict.passed());
    assert_eq!(verdict.new, vec![]);
    assert_eq!(verdict.enforced, vec![finding(
        "DRY-004", "src/a.rs", "", 3
    )]);
}

#[test]
fn counts_gating_findings_per_pass() {
    let base = vec![finding("CX-001", "src/a.rs", "run", 10)];
    let head = vec![
        finding("CX-001", "src/a.rs", "run", 10),
        finding("DRY-002", "src/a.rs", "unused", 20),
        finding("iosp/violation", "src/a.rs", "run", 10),
    ];

    let verdict = compare(base, head);

    assert_eq!(
        verdict.base,
        BTreeMap::from([("dry".to_owned(), 0), ("main".to_owned(), 1)])
    );
    assert_eq!(
        verdict.head,
        BTreeMap::from([("dry".to_owned(), 1), ("main".to_owned(), 1)])
    );
}

#[test]
fn lists_the_rules_in_the_verdict() {
    let head = vec![
        finding("CX-004", "src/a.rs", "parse", 40),
        finding("DRY-004", "src/a.rs", "", 3),
    ];

    let verdict = compare(vec![], head);

    assert_eq!(verdict.rules(), BTreeSet::from(["CX-004", "DRY-004"]));
}

#[test]
fn reads_renames_and_skips_other_statuses() {
    let output =
        "M\0src/a.rs\0R087\0src/old.rs\0src/new.rs\0C100\0src/b.rs\0src/c.rs\0A\0src/d.rs\0";

    let renames = Renames::from_name_status(output).unwrap();

    assert_eq!(
        renames,
        Renames(BTreeMap::from([(
            "src/old.rs".to_owned(),
            "src/new.rs".to_owned()
        )]))
    );
    assert_eq!(renames.new_path("src/old.rs"), "src/new.rs");
    assert_eq!(renames.new_path("src/a.rs"), "src/a.rs");
}

#[test]
fn reads_no_renames_from_empty_output() {
    assert_eq!(Renames::from_name_status("").unwrap(), Renames::default());
}

#[test]
fn rejects_a_rename_without_its_destination() {
    let error = Renames::from_name_status("R100\0src/old.rs\0").unwrap_err();

    assert_eq!(
        error.to_string(),
        r#"unexpected `git diff --name-status -z` output: status "R100" has no path"#
    );
}
