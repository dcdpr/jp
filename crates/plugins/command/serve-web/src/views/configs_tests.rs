use pretty_assertions::assert_eq;

use super::*;

/// Build an entry the way the host does: the namespace is everything before the
/// last separator, the name what follows it.
fn entry(segment: &str) -> ConfigEntry {
    let (namespace, name) = segment.rsplit_once('/').map_or_else(
        || (String::new(), segment.to_owned()),
        |(namespace, name)| (namespace.to_owned(), name.to_owned()),
    );

    ConfigEntry {
        segment: segment.to_owned(),
        namespace,
        name,
    }
}

/// The legends the chooser renders, in order.
fn legends(configs: &[ConfigEntry]) -> Vec<String> {
    group_by_namespace(configs)
        .into_keys()
        .map(|namespace| label(namespace).to_owned())
        .collect()
}

/// A workspace whose load paths nest, which is what `.jp/config` plus
/// `.jp/config/personas` produces: every persona is selectable under both its
/// bare name and its directory.
fn nested_load_paths() -> Vec<ConfigEntry> {
    // The order the host sends, which is a `BTreeSet` of the segments.
    [
        "architect",
        "committer",
        "default",
        "dev",
        "knowledge/testing",
        "knowledge/voice",
        "personas/architect",
        "personas/dev",
        "po",
        "pr-reviewer",
        "skill/rfd",
        "stager",
        "writer",
    ]
    .iter()
    .map(|segment| entry(segment))
    .collect()
}

/// One heading per directory, however the segments sort against each other.
///
/// Sorting by full segment scatters a directory's entries: `personas/dev` sorts
/// between `default` and `po`, and `skill/rfd` between `pr-reviewer` and
/// `stager`.
/// Grouping by adjacency opens a fresh block at each gap, so the root entries
/// arrive under three separate `General` headings.
#[test]
fn each_directory_gets_one_heading() {
    assert_eq!(legends(&nested_load_paths()), [
        "General",
        "knowledge",
        "personas",
        "skill"
    ]);
}

/// Root entries stay together under one heading, in the order the host sent
/// them.
#[test]
fn root_entries_are_one_group() {
    let configs = nested_load_paths();
    let groups = group_by_namespace(&configs);

    let names: Vec<&str> = groups[""].iter().map(|entry| entry.name.as_str()).collect();

    assert_eq!(names, [
        "architect",
        "committer",
        "default",
        "dev",
        "po",
        "pr-reviewer",
        "stager",
        "writer"
    ]);
}

/// Entries at a load path's root have no directory to name.
#[test]
fn the_root_group_is_labelled_generically() {
    assert_eq!(label(""), "General");
    assert_eq!(label("personas"), "personas");
}

/// A load path with nothing in it says so rather than rendering an empty
/// chooser.
#[test]
fn an_empty_list_says_there_is_nothing_to_choose() {
    let markup = chooser(&[], &[], &[]).into_string();

    assert!(
        markup.contains("No configurations found on the load paths."),
        "expected the empty note, got: {markup}"
    );
}

/// What a previous submission chose comes back ticked and filled in, so a
/// refused message does not cost the choices made with it.
#[test]
fn a_previous_choice_is_restored() {
    let configs = [entry("personas/dev")];
    let selected = ["personas/dev".to_owned()];
    let pairs = [("assistant.model.id".to_owned(), "opus".to_owned())];

    let markup = chooser(&configs, &selected, &pairs).into_string();

    let expected = concat!(
        r#"<div class="config-pairs">"#,
        r#"<p class="config-note">Set a value directly, as <code>--cfg key=value</code> does. "#,
        r#"Applied after the configurations below.</p>"#,
        r#"<div class="config-pair">"#,
        r#"<input type="text" name="cfg_key" value="assistant.model.id" "#,
        r#"placeholder="assistant.model.id" autocomplete="off" autocapitalize="off" "#,
        r#"spellcheck="false" aria-label="Configuration key">"#,
        r#"<input type="text" name="cfg_value" value="opus" placeholder="value" "#,
        r#"autocomplete="off" autocapitalize="off" spellcheck="false" "#,
        r#"aria-label="Configuration value">"#,
        r#"<button type="button" class="config-add" title="Add another" "#,
        r#"aria-label="Add another assignment">+</button>"#,
        r#"</div>"#,
        r#"<div class="config-pair">"#,
        r#"<input type="text" name="cfg_key" value="" placeholder="assistant.model.id" "#,
        r#"autocomplete="off" autocapitalize="off" spellcheck="false" "#,
        r#"aria-label="Configuration key">"#,
        r#"<input type="text" name="cfg_value" value="" placeholder="value" "#,
        r#"autocomplete="off" autocapitalize="off" spellcheck="false" "#,
        r#"aria-label="Configuration value">"#,
        r#"<button type="button" class="config-add" title="Add another" "#,
        r#"aria-label="Add another assignment">+</button>"#,
        r#"</div>"#,
        r#"</div>"#,
        r#"<div class="config-named">"#,
        r#"<fieldset><legend>personas</legend>"#,
        r#"<label class="config-option">"#,
        r#"<input type="checkbox" name="cfg" value="personas/dev" checked>"#,
        r#"<span>dev</span>"#,
        r#"</label>"#,
        r#"</fieldset>"#,
        r#"</div>"#,
    );

    assert_eq!(markup, expected);
}

/// A row left blank is not carried back as an assignment.
#[test]
fn blank_assignments_are_dropped() {
    let markup = chooser(&[], &[], &[(String::new(), "orphan".to_owned())]).into_string();

    assert!(
        !markup.contains("orphan"),
        "a value with no key must not survive, got: {markup}"
    );
}
