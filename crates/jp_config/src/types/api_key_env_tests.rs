use schematic::{SchemaBuilder, SchemaType};

use super::*;

fn many(pairs: &[(&str, &str)]) -> ApiKeyEnv {
    ApiKeyEnv::Many(
        pairs
            .iter()
            .map(|(name, variable)| ((*name).to_owned(), (*variable).into()))
            .collect(),
    )
}

fn named_list(name: &str, variables: &[&str]) -> ApiKeyEnv {
    ApiKeyEnv::Many(BTreeMap::from([(
        name.to_owned(),
        KeyVariables::FirstOf(variables.iter().map(|v| (*v).to_owned()).collect()),
    )]))
}

fn first_of(variables: &[&str]) -> ApiKeyEnv {
    ApiKeyEnv::FirstOf(variables.iter().map(|v| (*v).to_owned()).collect())
}

/// The form every existing config is written in.
#[test]
fn a_single_variable_answers_a_bare_entry() {
    let keys = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());

    assert_eq!(keys.variables(None).unwrap(), ["ANTHROPIC_API_KEY"]);
}

/// The caller reads the variables in the order the user wrote them, so the
/// order is the contract.
#[test]
fn a_list_answers_a_bare_entry_in_order() {
    let keys = first_of(&["WORK_KEY", "USER_KEY"]);

    assert_eq!(keys.variables(None).unwrap(), ["WORK_KEY", "USER_KEY"]);
}

/// A list is one key read from several places, so it has no names to select.
#[test]
fn a_list_answers_to_no_name() {
    let keys = first_of(&["WORK_KEY", "USER_KEY"]);

    assert!(matches!(
        keys.variables(Some("work")),
        Err(ApiKeyEnvError::Unknown { ref available, .. }) if available.is_empty()
    ));
    assert!(keys.names().is_empty());
}

/// A named key can itself be read from several variables, in order.
#[test]
fn a_named_key_can_be_a_list() {
    let keys = ApiKeyEnv::Many(BTreeMap::from([
        (
            "work".to_owned(),
            KeyVariables::FirstOf(vec!["WORK_KEY".to_owned(), "WORK_KEY_OLD".to_owned()]),
        ),
        ("personal".to_owned(), "PERSONAL_KEY".into()),
    ]));

    assert_eq!(keys.variables(Some("work")).unwrap(), [
        "WORK_KEY",
        "WORK_KEY_OLD"
    ]);
    assert_eq!(keys.variables(Some("personal")).unwrap(), ["PERSONAL_KEY"]);
    assert_eq!(keys.names(), ["personal", "work"]);
}

/// A sole named key answers a bare entry, list or not.
#[test]
fn a_bare_entry_resolves_a_sole_named_list() {
    let keys = named_list("work", &["WORK_KEY", "USER_KEY"]);

    assert_eq!(keys.variables(None).unwrap(), ["WORK_KEY", "USER_KEY"]);
}

/// A named key with no variables is a config mistake that names the key, not a
/// variable lookup that reports nothing.
#[test]
fn a_named_empty_list_reports_the_name() {
    let keys = named_list("work", &[]);

    let error = keys.variables(Some("work")).unwrap_err();
    assert!(matches!(error, ApiKeyEnvError::NoVariables { ref name } if name == "work"));
    assert_eq!(
        error.to_string(),
        "API key `work` names no environment variables"
    );
}

#[test]
fn an_empty_list_configures_no_key() {
    let keys = first_of(&[]);

    assert!(matches!(keys.variables(None), Err(ApiKeyEnvError::Empty)));
}

/// A lone variable has no name, so asking for one is asking for a key that is
/// not configured.
#[test]
fn a_single_variable_answers_to_no_name() {
    let keys = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());

    assert!(matches!(
        keys.variables(Some("work")),
        Err(ApiKeyEnvError::Unknown { .. })
    ));
}

#[test]
fn a_named_key_is_selected_by_name() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    assert_eq!(keys.variables(Some("work")).unwrap(), ["WORK_KEY"]);
    assert_eq!(keys.variables(Some("personal")).unwrap(), ["PERSONAL_KEY"]);
}

/// Which key pays is not a choice to make on the user's behalf.
#[test]
fn a_bare_entry_refuses_to_choose_between_several_keys() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    let error = keys.variables(None).unwrap_err();
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

    assert_eq!(keys.variables(None).unwrap(), ["WORK_KEY"]);
}

/// An unknown name reports what is configured, so the typo is visible.
#[test]
fn an_unknown_name_reports_the_configured_ones() {
    let keys = many(&[("work", "WORK_KEY"), ("personal", "PERSONAL_KEY")]);

    let message = keys.variables(Some("persnoal")).unwrap_err().to_string();
    assert!(message.contains("persnoal"), "{message}");
    assert!(message.contains("personal"), "{message}");
}

/// The schema describes every form `Deserialize` accepts, so a validator or an
/// editor does not reject the list or the named-key map.
#[test]
fn the_schema_accepts_a_variable_a_list_or_a_map_of_variables() {
    let schema = SchemaBuilder::build_root::<ApiKeyEnv>();
    let SchemaType::Union(union) = schema.ty else {
        panic!("expected a union, got {:?}", schema.ty)
    };

    let mut has_string = false;
    let mut has_list = false;
    let mut has_map = false;

    for variant in union.variants_types {
        match variant.ty {
            SchemaType::String(_) => has_string = true,
            SchemaType::Array(array) => {
                assert!(matches!(array.items_type.ty, SchemaType::String(_)));
                has_list = true;
            }
            SchemaType::Object(object) => {
                assert!(matches!(object.key_type.ty, SchemaType::String(_)));

                // Each named key is itself a variable or a list of them.
                let SchemaType::Union(value) = object.value_type.ty else {
                    panic!("expected a union, got {:?}", object.value_type.ty)
                };
                assert_eq!(value.variants_types.len(), 2);
                assert!(matches!(value.variants_types[0].ty, SchemaType::String(_)));
                let SchemaType::Array(array) = &value.variants_types[1].ty else {
                    panic!("expected an array, got {:?}", value.variants_types[1].ty)
                };
                assert!(matches!(array.items_type.ty, SchemaType::String(_)));

                has_map = true;
            }
            ty => panic!("unexpected variant: {ty:?}"),
        }
    }

    assert!(has_string, "`api_key_env = \"KEY\"` must be described");
    assert!(has_list, "`api_key_env = [\"KEY\"]` must be described");
    assert!(
        has_map,
        "`api_key_env = {{ work = \"KEY\" }}` must be described"
    );
}

/// Every form has to survive a round trip, since a resolved config is written
/// back into the conversation stream.
#[test]
fn every_form_round_trips() {
    let one = ApiKeyEnv::One("ANTHROPIC_API_KEY".to_owned());
    let json = serde_json::to_string(&one).unwrap();
    assert_eq!(json, r#""ANTHROPIC_API_KEY""#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), one);

    let many = many(&[("work", "WORK_KEY")]);
    let json = serde_json::to_string(&many).unwrap();
    assert_eq!(json, r#"{"work":"WORK_KEY"}"#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), many);

    let list = first_of(&["WORK_KEY", "USER_KEY"]);
    let json = serde_json::to_string(&list).unwrap();
    assert_eq!(json, r#"["WORK_KEY","USER_KEY"]"#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), list);

    let named = named_list("work", &["WORK_KEY", "USER_KEY"]);
    let json = serde_json::to_string(&named).unwrap();
    assert_eq!(json, r#"{"work":["WORK_KEY","USER_KEY"]}"#);
    assert_eq!(serde_json::from_str::<ApiKeyEnv>(&json).unwrap(), named);
}

#[test]
fn a_map_mixing_variables_and_lists_parses_from_toml() {
    #[derive(Deserialize)]
    struct Doc {
        api_key_env: ApiKeyEnv,
    }

    let doc: Doc = toml::from_str(
        r#"api_key_env = { work = ["WORK_KEY", "USER_KEY"], personal = "PERSONAL_KEY" }"#,
    )
    .unwrap();

    assert_eq!(
        doc.api_key_env,
        ApiKeyEnv::Many(BTreeMap::from([
            (
                "work".to_owned(),
                KeyVariables::FirstOf(vec!["WORK_KEY".to_owned(), "USER_KEY".to_owned()]),
            ),
            (
                "personal".to_owned(),
                KeyVariables::One("PERSONAL_KEY".to_owned())
            ),
        ]))
    );
}

#[test]
fn a_list_parses_from_toml() {
    #[derive(Deserialize)]
    struct Doc {
        api_key_env: ApiKeyEnv,
    }

    let doc: Doc = toml::from_str(r#"api_key_env = ["WORK_KEY", "USER_KEY"]"#).unwrap();

    assert_eq!(doc.api_key_env, first_of(&["WORK_KEY", "USER_KEY"]));
}

#[test]
fn a_list_displays_as_a_list() {
    assert_eq!(
        first_of(&["WORK_KEY", "USER_KEY"]).to_string(),
        "[WORK_KEY, USER_KEY]"
    );

    let keys = ApiKeyEnv::Many(BTreeMap::from([
        (
            "work".to_owned(),
            KeyVariables::FirstOf(vec!["WORK_KEY".to_owned(), "USER_KEY".to_owned()]),
        ),
        ("personal".to_owned(), "PERSONAL_KEY".into()),
    ]));
    assert_eq!(
        keys.to_string(),
        "{ personal = PERSONAL_KEY, work = [WORK_KEY, USER_KEY] }"
    );
}
