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

/// The group labels the picker renders, in order.
fn labels(configs: &[ConfigEntry]) -> Vec<String> {
    group_by_namespace(configs)
        .into_keys()
        .map(|namespace| label(namespace).to_owned())
        .collect()
}

/// The rows the chooser holds, without the templates it clones them from.
fn rows(markup: &str) -> &str {
    let opened = markup
        .split_once(r#"<ol class="config-items">"#)
        .expect("the chooser renders a list")
        .1;

    opened.split_once("</ol>").expect("the list is closed").0
}

/// The row a `+` button clones for the given kind.
fn template(markup: &str, kind: &str) -> String {
    let opened = markup
        .split_once(&format!(
            r#"<template class="config-template" data-kind="{kind}">"#
        ))
        .unwrap_or_else(|| panic!("the chooser renders a {kind} template, got: {markup}"))
        .1;

    opened
        .split_once("</template>")
        .expect("the template is closed")
        .0
        .to_owned()
}

/// The grip every row is dragged by.
const GRIP: &str = concat!(
    r#"<button type="button" class="config-grip" title="Drag to reorder" "#,
    r#"aria-label="Reorder this row; the arrow keys move it too">⠿</button>"#,
);

/// The button every row is dropped by.
const REMOVE: &str = concat!(
    r#"<button type="button" class="config-remove" title="Remove" "#,
    r#"aria-label="Remove">×</button>"#,
);

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

/// One group per directory, however the segments sort against each other.
///
/// Sorting by full segment scatters a directory's entries: `personas/dev` sorts
/// between `default` and `po`, and `skill/rfd` between `pr-reviewer` and
/// `stager`.
/// Grouping by adjacency opens a fresh group at each gap, so the root entries
/// arrive under three separate `General` labels.
#[test]
fn each_directory_gets_one_group() {
    assert_eq!(labels(&nested_load_paths()), [
        "General",
        "knowledge",
        "personas",
        "skill"
    ]);
}

/// Root entries stay together under one label, in the order the host sent them.
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

/// A load path with nothing in it says so, and offers no way to open a picker
/// with nothing in it either.
#[test]
fn an_empty_list_says_there_is_nothing_to_choose() {
    let markup = chooser(&[], &[]).into_string();

    assert!(
        markup.contains("No configurations found on the load paths."),
        "expected the empty note, got: {markup}"
    );
    assert!(
        markup.contains(r#"<button type="button" class="config-add" data-kind="named" disabled>"#),
        "expected the configuration button to be disabled, got: {markup}"
    );
    assert!(
        !markup.contains("config-picker"),
        "expected no picker to open, got: {markup}"
    );
}

/// The picker opens with the filter focused, so the set is gathered by typing.
#[test]
fn the_picker_opens_on_its_filter() {
    let markup = chooser(&[entry("dev")], &[]).into_string();

    let opened = markup
        .split_once(r#"<dialog class="config-modal config-picker">"#)
        .expect("the chooser renders a picker")
        .1;

    assert_eq!(
        opened
            .split_once(r#"<div class="config-picker-groups">"#)
            .expect("the picker renders its groups")
            .0,
        concat!(
            r#"<div class="config-picker-body">"#,
            r#"<h2>Add configurations</h2>"#,
            r#"<p class="config-note">Added in the order they are ticked. "#,
            r#"Enter ticks the first match, ⌘ Enter adds them.</p>"#,
            r#"<input type="text" class="config-filter" placeholder="Filter…" "#,
            r#"autocomplete="off" autocapitalize="off" spellcheck="false" "#,
            r#"aria-label="Filter configurations" autofocus>"#,
        )
    );
}

/// The picker offers every configuration as a box to tick, grouped by the
/// directory it lives in.
///
/// The boxes carry no `name`: they build the list, and it is the list the form
/// posts.
#[test]
fn the_picker_offers_every_configuration_as_a_box() {
    let markup = chooser(&[entry("knowledge/voice"), entry("dev")], &[]).into_string();

    let opened = markup
        .split_once(r#"<div class="config-picker-groups">"#)
        .expect("the chooser renders a picker")
        .1;
    let groups = opened
        .split_once("</div>")
        .expect("the groups are closed")
        .0;

    assert_eq!(
        groups,
        concat!(
            r#"<fieldset><legend>General</legend>"#,
            r#"<label class="config-option">"#,
            r#"<input type="checkbox" data-segment="dev">"#,
            r#"<span>dev</span>"#,
            r#"<span class="config-order"></span>"#,
            r#"</label>"#,
            r#"</fieldset>"#,
            r#"<fieldset><legend>knowledge</legend>"#,
            r#"<label class="config-option">"#,
            r#"<input type="checkbox" data-segment="knowledge/voice">"#,
            r#"<span>voice</span>"#,
            r#"<span class="config-order"></span>"#,
            r#"</label>"#,
            r#"</fieldset>"#,
        )
    );
}

/// Nothing is chosen until a row is added, so the list starts empty.
#[test]
fn nothing_is_chosen_to_begin_with() {
    let markup = chooser(&[entry("personas/dev")], &[]).into_string();

    assert_eq!(rows(&markup), "");
}

/// What a previous submission chose comes back in the order it was arranged, so
/// a refused message does not cost the choices made with it.
///
/// An argument naming a configuration comes back as a picker with that option
/// selected; anything else comes back as text.
#[test]
fn a_previous_choice_is_restored_in_order() {
    let configs = [entry("personas/dev")];
    let chosen = [
        "assistant.model.id=opus".to_owned(),
        "personas/dev".to_owned(),
    ];

    let markup = chooser(&configs, &chosen).into_string();

    let expected = format!(
        concat!(
            r#"<li class="config-item">"#,
            "{grip}",
            "{remove}",
            r#"<input type="text" class="config-raw" name="cfg" value="assistant.model.id=opus" "#,
            r#"placeholder="assistant.model.id=opus" autocomplete="off" autocapitalize="off" "#,
            r#"spellcheck="false" aria-label="Configuration argument">"#,
            r#"</li>"#,
            r#"<li class="config-item">"#,
            "{grip}",
            "{remove}",
            r#"<select class="config-pick" name="cfg" aria-label="Configuration">"#,
            r#"<option value="">Choose a configuration…</option>"#,
            r#"<optgroup label="personas">"#,
            r#"<option value="personas/dev" selected>dev</option>"#,
            r#"</optgroup>"#,
            r#"</select>"#,
            r#"</li>"#,
        ),
        grip = GRIP,
        remove = REMOVE
    );

    assert_eq!(rows(&markup), expected);
}

/// The `+` buttons clone the row they add from a template, taking its first
/// element and filling the field inside.
#[test]
fn the_chooser_carries_a_template_for_each_kind_of_row() {
    let markup = chooser(&[entry("personas/dev")], &[]).into_string();

    assert_eq!(
        template(&markup, "named"),
        format!(
            concat!(
                r#"<li class="config-item">"#,
                "{grip}",
                "{remove}",
                r#"<select class="config-pick" name="cfg" aria-label="Configuration">"#,
                r#"<option value="">Choose a configuration…</option>"#,
                r#"<optgroup label="personas">"#,
                r#"<option value="personas/dev">dev</option>"#,
                r#"</optgroup>"#,
                r#"</select>"#,
                r#"</li>"#,
            ),
            grip = GRIP,
            remove = REMOVE
        )
    );

    assert_eq!(
        template(&markup, "value"),
        format!(
            concat!(
                r#"<li class="config-item">"#,
                "{grip}",
                "{remove}",
                r#"<input type="text" class="config-raw" name="cfg" value="" "#,
                r#"placeholder="assistant.model.id=opus" autocomplete="off" "#,
                r#"autocapitalize="off" spellcheck="false" "#,
                r#"aria-label="Configuration argument">"#,
                r#"</li>"#,
            ),
            grip = GRIP,
            remove = REMOVE
        )
    );
}

/// The picker starts on an option that chooses nothing, so adding a row does
/// not quietly apply whichever configuration sorts first.
#[test]
fn a_fresh_picker_selects_nothing() {
    let markup = picker(&[entry("personas/dev")], "").into_string();

    assert_eq!(
        markup,
        concat!(
            r#"<select class="config-pick" name="cfg" aria-label="Configuration">"#,
            r#"<option value="">Choose a configuration…</option>"#,
            r#"<optgroup label="personas">"#,
            r#"<option value="personas/dev">dev</option>"#,
            r#"</optgroup>"#,
            r#"</select>"#,
        )
    );
}
