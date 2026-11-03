use super::*;

/// A ticket's labels, from tokens.
fn labels(tokens: &[&str]) -> Labels {
    Labels::from_tokens(tokens).unwrap()
}

fn owned(tokens: &[&str]) -> Vec<String> {
    tokens.iter().map(|token| (*token).to_owned()).collect()
}

#[test]
fn add_keeps_what_the_ticket_carries() {
    let current = labels(&["client=cli", "type=bug"]);

    assert_eq!(add(&current, &owned(&["package=jp_cli"])), [
        "client=cli",
        "type=bug",
        "package=jp_cli"
    ]);
}

/// `set` replaces only the keys it names, and naming a key twice gives it both
/// values.
#[test]
fn set_replaces_what_the_named_keys_hold() {
    let current = labels(&[
        "client=cli",
        "package=jp_cli",
        "package=jp_config",
        "type=task",
    ]);

    assert_eq!(
        set(
            &current,
            &owned(&["Type=bug", "package=jp_md", "package=jp_term"])
        ),
        ["client=cli", "Type=bug", "package=jp_md", "package=jp_term"]
    );
}

#[test]
fn remove_takes_one_label_or_every_label_under_a_bare_key() {
    let current = labels(&[
        "client=cli",
        "package=jp_cli",
        "package=jp_config",
        "blocked",
    ]);

    assert_eq!(remove(&current, &owned(&["package=JP_CLI"])), [
        "blocked",
        "client=cli",
        "package=jp_config"
    ]);
    assert_eq!(remove(&current, &owned(&["package", "blocked"])), [
        "client=cli"
    ]);
    assert_eq!(remove(&current, &[]), current.to_tokens());
}

/// A label naming a value matches only that value, so `type=bug` doesn't take a
/// bare `type` or `type=task`.
#[test]
fn a_named_value_matches_only_that_value() {
    let current = labels(&["type=task", "draft"]);

    assert_eq!(
        carried(&current, &owned(&["type=bug", "draft=yes", "TYPE=Task"])),
        ["TYPE=Task"]
    );
}

#[test]
fn difference_names_what_left_and_what_arrived() {
    let before = labels(&["client=cli", "type=task"]);
    let after = labels(&["client=cli", "type=bug"]);

    assert_eq!(
        difference(&before, &after),
        (owned(&["type=task"]), owned(&["type=bug"]))
    );
}
