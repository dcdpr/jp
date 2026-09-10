use super::*;

fn many(pairs: &[(&str, &str)]) -> ApiKeyEnv {
    ApiKeyEnv::Many(
        pairs
            .iter()
            .map(|(name, variable)| ((*name).to_owned(), (*variable).to_owned()))
            .collect(),
    )
}

/// The form every existing config is written in.
#[test]
fn a_single_variable_answers_a_bare_entry() {
    let keys = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());

    assert_eq!(keys.variable(None).unwrap(), "ANTHROPIC_API_KEY");
}

/// A lone variable has no name, so asking for one is asking for a key that is
/// not configured.
#[test]
fn a_single_variable_answers_to_no_name() {
    let keys = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());

    assert!(matches!(
        keys.variable(Some("work")),
        Err(ApiKeyEnvError::Unknown { .. })
    ));
}

#[test]
fn a_named_key_is_selected_by_name() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    assert_eq!(keys.variable(Some("work")).unwrap(), "WORK_KEY");
    assert_eq!(keys.variable(Some("personal")).unwrap(), "PERSONAL_KEY");
}

/// Which key pays is not a choice to make on the user's behalf.
#[test]
fn a_bare_entry_refuses_to_choose_between_several_keys() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    let error = keys.variable(None).unwrap_err();
    assert!(matches!(error, ApiKeyEnvError::Ambiguous { .. }));

    // The message has to name both candidates and how to pick one, or it
    // leaves the user guessing at their own config.
    let message = error.to_string();
    assert!(message.contains("personal"), "{message}");
    assert!(message.contains("work"), "{message}");
    assert!(message.contains("api_key:<name>"), "{message}");
}

/// One named key is unambiguous, so a bare entry resolves it.
#[test]
fn a_bare_entry_resolves_a_sole_named_key() {
    let keys = many(&[("work", "WORK_KEY")]);

    assert_eq!(keys.variable(None).unwrap(), "WORK_KEY");
}

/// An unknown name reports what is configured, so the typo is visible.
#[test]
fn an_unknown_name_reports_the_configured_ones() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    let message = keys.variable(Some("persnoal")).unwrap_err().to_string();
    assert!(message.contains("persnoal"), "{message}");
    assert!(message.contains("personal"), "{message}");
}

/// Both forms have to survive a round trip, since one of them is what every
/// config written so far uses.
#[test]
fn both_forms_round_trip() {
    let one = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());
    let json = serde_json::to_string(&one).unwrap();
    assert_eq!(json, r#""ANTHROPIC_API_KEY""#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), one);

    let many = many(&[("work", "WORK_KEY")]);
    let json = serde_json::to_string(&many).unwrap();
    assert_eq!(json, r#"{"work":"WORK_KEY"}"#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), many);
}
