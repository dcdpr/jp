use super::*;

/// The literal values a rule resolves to, for readable assertions.
fn values(config: &LabelConfig) -> Vec<&str> {
    match config.value() {
        LabelValueRef::Values(values) => values.iter().map(String::as_str).collect(),
        LabelValueRef::Command(_) => panic!("expected literal values"),
    }
}

#[test]
fn valid_keys_are_accepted() {
    for key in ["a", "branch", "team-name", "team_name", "v2", "A-1_b"] {
        assert!(validate_key(key).is_ok(), "expected '{key}' to be valid");
    }
}

#[test]
fn invalid_keys_are_rejected() {
    // Every rejected character is significant somewhere else: `.` in dotted
    // config paths, `=`/`,` in CLI parsing, `:` as the alias marker.
    for key in [
        "",
        "team.platform",
        "team=x",
        "a,b",
        ":branch",
        "a b",
        "café",
    ] {
        assert!(validate_key(key).is_err(), "expected '{key}' to be invalid");
    }
}

#[test]
fn static_shorthand_deserializes() {
    let config: LabelConfig = serde_json::from_str(r#""platform""#).unwrap();

    assert_eq!(values(&config), ["platform"]);
    assert_eq!(config.apply_on(), ApplyOn {
        new: true,
        fork: false
    });
    assert_eq!(config.run(), LabelRunMode::Ask);
}

/// A list is shorthand for `value`, the way a bare string already is, so it
/// takes the same defaults.
#[test]
fn list_shorthand_deserializes() {
    let config: LabelConfig = serde_json::from_str(r#"["jp_config","jp_llm"]"#).unwrap();

    assert_eq!(values(&config), ["jp_config", "jp_llm"]);
    assert_eq!(config.apply_on(), ApplyOn {
        new: true,
        fork: false
    });
    assert_eq!(config.run(), LabelRunMode::Ask);
}

#[test]
fn list_value_deserializes() {
    let json = r#"{"value":["jp_config","jp_llm"],"apply_on":{"fork":true}}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    assert_eq!(values(&config), ["jp_config", "jp_llm"]);
    assert!(config.apply_on().fork);
}

/// An empty list is a rule that produces no label, which is how a rule is
/// turned off without deleting it.
#[test]
fn an_empty_list_resolves_to_no_values() {
    for json in [r"[]", r#"{"value":[]}"#] {
        let config: LabelConfig = serde_json::from_str(json).unwrap();

        assert!(values(&config).is_empty(), "got: {json}");
    }
}

#[test]
fn object_form_deserializes() {
    let json = r#"{"value":"review","apply_on":{"new":false,"fork":true},"run":"unattended"}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    assert_eq!(values(&config), ["review"]);
    assert_eq!(config.apply_on(), ApplyOn {
        new: false,
        fork: true
    });
    assert_eq!(config.run(), LabelRunMode::Unattended);
}

/// A rule is required unless it says otherwise, so an unmarked rule reports its
/// failures.
#[test]
fn rules_are_required_by_default() {
    for json in [r#""platform""#, r#"["a"]"#, r#"{"value":"x"}"#] {
        let config: LabelConfig = serde_json::from_str(json).unwrap();

        assert!(!config.optional(), "got: {json}");
    }
}

#[test]
fn optional_deserializes() {
    let json = r#"{"value":{"cmd":"git branch --show-current"},"optional":true}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    assert!(config.optional());
}

#[test]
fn command_shorthand_value_deserializes() {
    let json = r#"{"value":{"cmd":"git rev-parse --abbrev-ref HEAD"},"run":"unattended"}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    let LabelValueRef::Command(cmd) = config.value() else {
        panic!("expected a command value, got {:?}", config.value());
    };
    let cmd = cmd.clone().command();
    assert_eq!(cmd.program, "git");
    assert_eq!(cmd.args, ["rev-parse", "--abbrev-ref", "HEAD"]);
    assert!(!cmd.shell);
    assert_eq!(config.run(), LabelRunMode::Unattended);
}

#[test]
fn structured_command_value_deserializes() {
    let json = r#"{"value":{"cmd":{"program":"hostname","args":["-s"],"shell":true}}}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    let LabelValueRef::Command(cmd) = config.value() else {
        panic!("expected a command value");
    };
    let cmd = cmd.clone().command();
    assert_eq!(cmd.program, "hostname");
    assert_eq!(cmd.args, ["-s"]);
    assert!(cmd.shell);

    // An unset `run` defaults to prompting, not to running unattended.
    assert_eq!(config.run(), LabelRunMode::Ask);
}
