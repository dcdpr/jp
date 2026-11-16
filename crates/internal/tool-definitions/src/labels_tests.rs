use super::*;

/// A vocabulary with a retired value and a bare key, which is every shape the
/// roles treat differently.
fn vocabulary() -> Vocabulary {
    Vocabulary::parse(
        r#"{
            "client": {"description": "The client.", "values": ["cli", "macos"]},
            "package": {"description": "The crate.", "values": ["jp_cli"], "retired": ["jp_legacy"]},
            "triaged": {"description": "Someone has looked at it."}
        }"#,
    )
    .unwrap()
}

/// A write is offered what it may add; a filter also gets retired values and a
/// bare key per facet.
#[test]
fn a_filter_is_offered_more_than_a_write() {
    assert_eq!(
        render("t", "labels", &vocabulary(), "ticket", Role::Add),
        "\n[conversation.tools.t.parameters.labels.items]\nenum = [\n  \"client=cli\",\n  \
         \"client=macos\",\n  \"package=jp_cli\",\n  \"triaged\",\n]\n"
    );
    assert_eq!(
        render("t", "labels", &vocabulary(), "ticket", Role::Filter),
        "\n[conversation.tools.t.parameters.labels.items]\nenum = [\n  \"client\",\n  \
         \"client=cli\",\n  \"client=macos\",\n  \"package\",\n  \"package=jp_cli\",\n  \
         \"package=jp_legacy\",\n  \"triaged\",\n]\n"
    );
}

/// A bounded key is described beside the lists that write an item's labels, and
/// a key an item needs makes `create`'s labels required, so the assistant knows
/// before its first call.
/// A filter is held to neither.
#[test]
fn the_bounds_are_described_to_the_tools_that_write_labels() {
    let vocabulary = Vocabulary::parse(
        r#"{
            "type": {"values": ["bug"], "min": 1, "max": 1},
            "team": {"values": ["core"], "max": 1}
        }"#,
    )
    .unwrap();

    assert_eq!(
        render("t", "labels", &vocabulary, "page", Role::Create),
        "\n[conversation.tools.t.parameters.labels]\nrequired = \
         true\n\n[conversation.tools.t.parameters.labels.items]\nsummary = \"One `key=value` \
         label. A page carries at most one label under `team` and exactly one label under \
         `type`.\"\nenum = [\n  \"team=core\",\n  \"type=bug\",\n]\n"
    );
    assert_eq!(
        render("t", "add_labels", &vocabulary, "page", Role::Add),
        "\n[conversation.tools.t.parameters.add_labels.items]\nsummary = \"One `key=value` label. \
         A page carries at most one label under `team` and exactly one label under \
         `type`.\"\nenum = [\n  \"team=core\",\n  \"type=bug\",\n]\n"
    );
    assert_eq!(
        render("t", "labels", &vocabulary, "page", Role::Filter),
        "\n[conversation.tools.t.parameters.labels.items]\nenum = [\n  \"team\",\n  \
         \"team=core\",\n  \"type\",\n  \"type=bug\",\n]\n"
    );
}

/// A key taking any value can't be listed, so a vocabulary with one gets no
/// lists: its labels are described in words instead, along with the keys one
/// item holds on its own.
#[test]
fn open_and_unique_keys_are_described_in_words() {
    let vocabulary = Vocabulary::parse(
        r#"{
            "rfd": {"values": [], "unique": "key-value"},
            "type": {"values": ["bug"]}
        }"#,
    )
    .unwrap();

    assert_eq!(
        render("t", "labels", &vocabulary, "ticket", Role::Create),
        "\n[conversation.tools.t.parameters.labels.items]\nsummary = \"One `key=value` label: \
         type=bug, or rfd=<value> with any value. Only one ticket carries each `rfd` value.\"\n"
    );
    assert_eq!(
        render("t", "labels", &vocabulary, "ticket", Role::Filter),
        "\n[conversation.tools.t.parameters.labels.items]\nsummary = \"One `key=value` label, or \
         a bare `key` for any value under it: rfd, type, type=bug, or rfd=<value> with any \
         value.\"\n"
    );
}

/// Write tools describe the expected value format; filter tools also list
/// comparison operators.
#[test]
fn a_typed_key_is_described_with_its_type() {
    let vocabulary =
        Vocabulary::parse(r#"{"due": {"values": [], "max": 1, "type": "date"}}"#).unwrap();

    assert_eq!(
        render("t", "labels", &vocabulary, "ticket", Role::Add),
        "\n[conversation.tools.t.parameters.labels.items]\nsummary = \"One `key=value` label: \
         due=<value>, with any value. A ticket carries at most one label under `due`. `due` takes \
         a date, written YYYY-MM-DD.\"\n"
    );
    assert_eq!(
        render("t", "labels", &vocabulary, "ticket", Role::Filter),
        "\n[conversation.tools.t.parameters.labels.items]\nsummary = \"One `key=value` label, or \
         a bare `key` for any value under it: due, or due=<value> with any value. `due` holds a \
         date, written YYYY-MM-DD; compare it with `<`, `<=`, `>`, or `>=`, as in \
         `due<2026-12-01`.\"\n"
    );
}

/// An empty `enum` accepts nothing, which some providers refuse outright, so a
/// vocabulary without labels writes nothing at all.
#[test]
fn an_empty_vocabulary_writes_nothing() {
    assert_eq!(
        render(
            "t",
            "labels",
            &Vocabulary::default(),
            "ticket",
            Role::Create
        ),
        ""
    );
}
