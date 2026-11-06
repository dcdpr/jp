use indoc::indoc;

use super::*;

const POLICY: &str = indoc! {r#"
    exempt = ["iosp/violation", "TQ-*"]
    enforced = ["DRY-004", "TQ-001"]

    [[pass]]
    name = "dry"
    config = "dry.toml"
    rules = ["DRY-*"]

    [[pass]]
    name = "main"
    config = "config.toml"
    rules = ["*"]
"#};

#[test]
fn a_rule_is_read_from_the_first_pass_that_lists_it() {
    let policy = Policy::parse(POLICY).unwrap();

    assert_eq!(
        policy.owner("DRY-002").map(|pass| pass.name.as_str()),
        Some("dry")
    );
    assert_eq!(
        policy.owner("CX-001").map(|pass| pass.name.as_str()),
        Some("main")
    );
    assert_eq!(
        policy
            .owner("iosp/violation")
            .map(|pass| pass.name.as_str()),
        Some("main")
    );
}

#[test]
fn a_rule_no_pass_lists_has_no_owner() {
    let input = indoc! {r#"
        [[pass]]
        name = "dry"
        config = "dry.toml"
        rules = ["DRY-*"]
    "#};
    let policy = Policy::parse(input).unwrap();

    assert_eq!(policy.owner("CX-001"), None);
}

#[test]
fn exempt_rules_do_not_gate() {
    let policy = Policy::parse(POLICY).unwrap();

    assert!(!policy.gates("iosp/violation"));
    assert!(!policy.gates("TQ-003"));
    assert!(policy.gates("CX-001"));
}

#[test]
fn an_enforced_rule_gates_even_when_an_exemption_matches_it() {
    let policy = Policy::parse(POLICY).unwrap();

    assert!(policy.is_enforced("TQ-001"));
    assert!(policy.gates("TQ-001"));
}

#[test]
fn a_pattern_without_a_star_matches_only_its_rule() {
    let policy = Policy::parse(POLICY).unwrap();

    assert!(policy.is_enforced("DRY-004"));
    assert!(!policy.is_enforced("DRY-0045"));
    assert!(!policy.is_enforced("DRY-00"));
}

#[test]
fn rejects_a_policy_without_passes() {
    let error = Policy::parse("exempt = []").unwrap_err();

    assert!(matches!(error, PolicyError::Toml(_)), "{error:?}");
}

#[test]
fn rejects_an_empty_list_of_passes() {
    let error = Policy::parse("pass = []").unwrap_err();

    assert_eq!(error.to_string(), "the policy names no pass");
}

#[test]
fn rejects_a_pass_named_twice() {
    let input = indoc! {r#"
        [[pass]]
        name = "main"
        config = "config.toml"
        rules = ["*"]

        [[pass]]
        name = "main"
        config = "dry.toml"
        rules = ["DRY-*"]
    "#};

    let error = Policy::parse(input).unwrap_err();

    assert_eq!(
        error.to_string(),
        r#"the policy names pass "main" more than once"#
    );
}

#[test]
fn rejects_a_pass_read_for_no_rule() {
    let input = indoc! {r#"
        [[pass]]
        name = "main"
        config = "config.toml"
        rules = []
    "#};

    let error = Policy::parse(input).unwrap_err();

    assert_eq!(error.to_string(), r#"pass "main" is read for no rule"#);
}

#[test]
fn rejects_an_unknown_key() {
    let input = indoc! {r#"
        exempted = ["CX-001"]

        [[pass]]
        name = "main"
        config = "config.toml"
        rules = ["*"]
    "#};

    let error = Policy::parse(input).unwrap_err();

    assert!(matches!(error, PolicyError::Toml(_)), "{error:?}");
}

#[test]
fn rejects_a_star_before_the_end_of_a_pattern() {
    let input = indoc! {r#"
        exempt = ["DRY-*-x"]

        [[pass]]
        name = "main"
        config = "config.toml"
        rules = ["*"]
    "#};

    let error = Policy::parse(input).unwrap_err();

    assert!(
        error
            .to_string()
            .contains(r#"`*` is only allowed at the end of a rule pattern: "DRY-*-x""#),
        "{error}"
    );
}

#[test]
fn rejects_an_empty_pattern() {
    let input = indoc! {r#"
        enforced = [""]

        [[pass]]
        name = "main"
        config = "config.toml"
        rules = ["*"]
    "#};

    let error = Policy::parse(input).unwrap_err();

    assert!(
        error.to_string().contains("a rule pattern can't be empty"),
        "{error}"
    );
}
