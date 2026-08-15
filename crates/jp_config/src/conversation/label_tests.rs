use super::*;

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

    assert_eq!(config.value(), LabelValueRef::Static("platform"));
    assert_eq!(config.apply_on(), ApplyOn {
        new: true,
        fork: false
    });
    assert_eq!(config.run(), LabelRunMode::Ask);
}

#[test]
fn object_form_deserializes() {
    let json = r#"{"value":"review","apply_on":{"new":false,"fork":true},"run":"unattended"}"#;

    let config: LabelConfig = serde_json::from_str(json).unwrap();

    assert_eq!(config.value(), LabelValueRef::Static("review"));
    assert_eq!(config.apply_on(), ApplyOn {
        new: false,
        fork: true
    });
    assert_eq!(config.run(), LabelRunMode::Unattended);
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
