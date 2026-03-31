use super::*;

#[test]
fn parse_ask() {
    assert_eq!(
        "ask".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Ask
    );
}

#[test]
fn parse_last_aliases() {
    assert_eq!(
        "last".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
    assert_eq!(
        "last-activated".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
    assert_eq!(
        "last_activated".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastActivated
    );
}

#[test]
fn parse_last_created() {
    assert_eq!(
        "last-created".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastCreated
    );
    assert_eq!(
        "last_created".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::LastCreated
    );
}

#[test]
fn parse_previous_aliases() {
    assert_eq!(
        "previous".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Previous
    );
    assert_eq!(
        "prev".parse::<DefaultConversationId>().unwrap(),
        DefaultConversationId::Previous
    );
}

#[test]
fn parse_conversation_id_fallback() {
    let result = "jp-c17528832001".parse::<DefaultConversationId>().unwrap();
    assert_eq!(result, DefaultConversationId::Id("jp-c17528832001".into()));
}

#[test]
fn deserialize_from_toml() {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        id: DefaultConversationId,
    }

    let w: Wrapper = toml::from_str("id = \"last\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::LastActivated);

    let w: Wrapper = toml::from_str("id = \"ask\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::Ask);

    let w: Wrapper = toml::from_str("id = \"jp-c17528832001\"").unwrap();
    assert_eq!(w.id, DefaultConversationId::Id("jp-c17528832001".into()));
}

#[test]
fn default_is_ask() {
    assert!(DefaultConversationId::default().is_ask());
}
