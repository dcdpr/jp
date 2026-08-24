use std::collections::BTreeMap;

use clap::Parser as _;
use strip_ansi_escapes::strip_str;

use super::*;
use crate::cmd::{
    conversation_id::ConversationIds as _, label::LabelDirectives, target::ConversationTarget,
};

#[derive(Debug, clap::Parser)]
struct Cmd {
    #[command(flatten)]
    label: Label,
}

fn parse(args: &[&str]) -> Label {
    Cmd::try_parse_from(args).unwrap().label
}

/// `--id` is global, so it reads the same on either side of the verb.
/// Both spellings appear in the documentation, and a user has no reason to
/// expect one to work and the other not to.
#[test]
fn the_id_flag_is_accepted_on_either_side_of_the_verb() {
    let before = parse(&["label", "--id=jp-c17866928997", "add", "a=1"]);
    let after = parse(&["label", "add", "--id=jp-c17866928997", "a=1"]);

    let expected = [ConversationTarget::Id(
        "jp-c17866928997".parse().expect("valid id"),
    )];

    assert_eq!(before.target.ids(), expected);
    assert_eq!(after.target.ids(), expected);
}

/// A bare `jp c label` lists, so it needs no verb.
#[test]
fn a_bare_invocation_has_no_verb() {
    assert!(parse(&["label"]).command.is_none());
    assert!(parse(&["label", "--id=jp-c17866928997"]).command.is_none());
}

#[test]
fn the_verbs_parse_and_carry_their_arguments() {
    let Some(Commands::Add(add)) = parse(&["label", "add", "a=1", "b", ":c"]).command else {
        panic!("expected `add`");
    };
    assert_eq!(add.labels, ["a=1", "b", ":c"]);

    let Some(Commands::Rm(rm)) = parse(&["label", "rm", "a", "b"]).command else {
        panic!("expected `rm`");
    };
    assert_eq!(rm.keys, ["a", "b"]);

    assert!(matches!(
        parse(&["label", "ls"]).command,
        Some(Commands::Ls)
    ));
}

/// A bare `rm` clears every label.
/// The argument slot holds only keys, so an empty one cannot swallow the
/// conversation target the way an optional flag value would.
#[test]
fn a_bare_rm_removes_everything() {
    let Some(Commands::Rm(rm)) = parse(&["label", "rm"]).command else {
        panic!("expected `rm`");
    };

    assert!(rm.keys.is_empty());
}

/// The short forms are what a user reaches for after the first few times.
#[test]
fn the_verbs_have_short_aliases() {
    assert!(matches!(
        parse(&["label", "a", "x=1"]).command,
        Some(Commands::Add(_))
    ));
    assert!(matches!(
        parse(&["label", "r", "x"]).command,
        Some(Commands::Rm(_))
    ));
}

/// Adding nothing is a mistake, not a no-op.
/// Removing nothing means "remove everything", so it parses.
#[test]
fn add_requires_at_least_one_argument() {
    assert!(Cmd::try_parse_from(["label", "add"]).is_err());
    assert!(Cmd::try_parse_from(["label", "rm"]).is_ok());
}

/// Values and keys are bare arguments, so the shell splits them and nothing
/// needs escaping: a comma is just a character.
#[test]
fn arguments_carry_values_verbatim() {
    let Some(Commands::Add(add)) = parse(&["label", "add", "branch=feat,exp"]).command else {
        panic!("expected `add`");
    };

    assert_eq!(add.labels, ["branch=feat,exp"]);
    assert_eq!(
        LabelDirective::parse_set::<true>(&add.labels[0]).unwrap(),
        LabelDirective::Set {
            key: "branch".to_owned(),
            value: "feat,exp".to_owned()
        }
    );
}

/// The conversation is named with `--id`, never positionally, so a key that
/// looks like a conversation target is unambiguous.
#[test]
fn a_target_keyword_is_an_ordinary_key() {
    let Some(Commands::Rm(rm)) = parse(&["label", "rm", "active"]).command else {
        panic!("expected `rm`");
    };

    assert_eq!(rm.keys, ["active"]);
    assert_eq!(
        LabelDirective::parse_remove(&rm.keys[0]).unwrap(),
        LabelDirective::Remove("active".to_owned())
    );
}

// ── Reported output ──────────────────────────────────────────────────────────

fn resolved(directives: Vec<LabelDirective>) -> label::Resolved {
    // Built through the same constructor the alias-free commands use, so the
    // test can't produce a `Resolved` the command couldn't.
    LabelDirectives::<false, false>(directives).resolved()
}

/// Run `directives` against `labels` and render the reported line, the way the
/// command does.
fn line(added: bool, labels: &[(&str, &str)], directives: Vec<LabelDirective>) -> String {
    let mut map: BTreeMap<String, String> = labels
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();

    let directives = resolved(directives);
    let applied = label::apply(&mut map, &directives);

    let mut report = Report::new(added);
    let touched = report.touched(&directives, &applied);

    strip_str(report.line(&touched, "jp-c123: Title Here"))
}

#[test]
fn added_labels_are_listed_as_the_user_writes_them() {
    assert_eq!(
        line(true, &[], vec![
            LabelDirective::Set {
                key: "foo".to_owned(),
                value: String::new()
            },
            LabelDirective::Set {
                key: "baz".to_owned(),
                value: "qux".to_owned()
            },
        ]),
        "Added labels foo, baz=qux to jp-c123: Title Here"
    );
}

/// A removal reports the values the labels held, not the keys the user typed,
/// so the line can be pasted back as an `add`.
#[test]
fn removed_labels_are_listed_with_the_values_they_held() {
    assert_eq!(
        line(
            false,
            &[("foo", ""), ("qux", "quux"), ("keep", "me")],
            vec![
                LabelDirective::Remove("foo".to_owned()),
                LabelDirective::Remove("qux".to_owned()),
            ]
        ),
        "Removed labels foo, qux=quux from jp-c123: Title Here"
    );
}

/// The whole point of the bare form: what it took is what you need to put back.
#[test]
fn clearing_everything_lists_what_was_there() {
    assert_eq!(
        line(false, &[("foo", ""), ("bar", ""), ("qux", "quux")], vec![
            LabelDirective::RemoveAll
        ]),
        "Removed labels bar, foo, qux=quux from jp-c123: Title Here"
    );
}

/// A removal that matched nothing says so rather than printing an empty list.
#[test]
fn a_removal_that_matched_nothing_is_reported_as_such() {
    assert_eq!(
        line(false, &[], vec![LabelDirective::RemoveAll]),
        "No labels to remove from jp-c123: Title Here"
    );
}

/// The JSON form exposes the parts a script needs, rather than a sentence to
/// parse.
/// Labels carry the same `{key, value}` shape `jp c show` emits.
#[test]
fn the_json_form_is_structured() {
    let mut map = BTreeMap::from([
        ("foo".to_owned(), String::new()),
        ("qux".to_owned(), "quux".to_owned()),
    ]);
    let directives = resolved(vec![LabelDirective::RemoveAll]);
    let applied = label::apply(&mut map, &directives);

    let mut report = Report::new(false);
    let labels = report.touched(&directives, &applied);
    let id: ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(
        report.json(&labels, id, Some("Tool Chaining")),
        serde_json::json!({
            "action": "removed",
            "conversation": { "id": "jp-c17866928997", "title": "Tool Chaining" },
            "labels": [
                { "key": "foo", "value": "" },
                { "key": "qux", "value": "quux" },
            ],
        })
    );
}

/// A removal that matched nothing is an empty array, not prose.
/// An untitled conversation reports a null title rather than omitting the
/// field.
#[test]
fn the_json_form_stays_structured_when_nothing_matched() {
    let mut map = BTreeMap::new();
    let directives = resolved(vec![LabelDirective::RemoveAll]);
    let applied = label::apply(&mut map, &directives);

    let mut report = Report::new(false);
    let labels = report.touched(&directives, &applied);
    let id: ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(
        report.json(&labels, id, None),
        serde_json::json!({
            "action": "removed",
            "conversation": { "id": "jp-c17866928997", "title": null },
            "labels": [],
        })
    );
}

/// The collapsed line unions what was removed across targets, since a bare `rm`
/// takes a different set from each conversation.
#[test]
fn the_collapsed_line_unions_every_target() {
    let directives = resolved(vec![LabelDirective::RemoveAll]);
    let mut report = Report::new(false);

    for labels in [
        &[("foo", ""), ("qux", "quux")][..],
        &[("foo", ""), ("bar", "")][..],
    ] {
        let mut map: BTreeMap<String, String> = labels
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        let applied = label::apply(&mut map, &directives);
        report.touched(&directives, &applied);
    }

    let union = report.union.clone();
    assert_eq!(
        strip_str(report.line(&union, "2 conversations")),
        "Removed labels foo, qux=quux, bar from 2 conversations"
    );
}

/// An untitled conversation reports the ID alone rather than a dangling colon.
#[test]
fn a_target_without_a_title_is_just_the_id() {
    let id: jp_conversation::ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(strip_str(conversation_target(id, None)), "jp-c17866928997");
    assert_eq!(
        strip_str(conversation_target(id, Some("Some Title"))),
        "jp-c17866928997: Some Title"
    );
}

/// Only `add` needs the target's own configuration, because only `add` can
/// carry an alias.
#[test]
fn only_add_layers_the_target_config() {
    assert_eq!(
        parse(&["label", "add", "a=1"])
            .conversation_load_request()
            .config_conversation,
        Some(0)
    );
    assert_eq!(
        parse(&["label", "rm", "a"])
            .conversation_load_request()
            .config_conversation,
        None
    );
    assert_eq!(
        parse(&["label"])
            .conversation_load_request()
            .config_conversation,
        None
    );
}
