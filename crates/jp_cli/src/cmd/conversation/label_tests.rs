use clap::Parser as _;
use jp_conversation::Labels;
use jp_printer::OutputFormat;
use strip_ansi_escapes::strip_str;

use super::*;
use crate::cmd::{
    conversation_id::ConversationIds as _, label::Grouped, target::ConversationTarget,
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

    let Some(Commands::Set(set)) = parse(&["label", "set", "a=1", "a=2"]).command else {
        panic!("expected `set`");
    };
    assert_eq!(set.labels, ["a=1", "a=2"]);

    let Some(Commands::Rm(rm)) = parse(&["label", "rm", "a", "b=2"]).command else {
        panic!("expected `rm`");
    };
    assert_eq!(rm.labels, ["a", "b=2"]);

    assert!(matches!(
        parse(&["label", "ls"]).command,
        Some(Commands::Ls)
    ));
}

/// A bare `rm` clears every label.
/// The argument slot holds only labels, so an empty one cannot swallow the
/// conversation target the way an optional flag value would.
#[test]
fn a_bare_rm_removes_everything() {
    let Some(Commands::Rm(rm)) = parse(&["label", "rm"]).command else {
        panic!("expected `rm`");
    };

    assert!(rm.labels.is_empty());
}

/// The short forms are what a user reaches for after the first few times.
#[test]
fn the_verbs_have_short_aliases() {
    assert!(matches!(
        parse(&["label", "a", "x=1"]).command,
        Some(Commands::Add(_))
    ));
    assert!(matches!(
        parse(&["label", "s", "x=1"]).command,
        Some(Commands::Set(_))
    ));
    assert!(matches!(
        parse(&["label", "r", "x"]).command,
        Some(Commands::Rm(_))
    ));
}

/// Applying nothing is a mistake, not a no-op.
/// Removing nothing means "remove everything", so it parses.
#[test]
fn add_and_set_require_at_least_one_argument() {
    assert!(Cmd::try_parse_from(["label", "add"]).is_err());
    assert!(Cmd::try_parse_from(["label", "set"]).is_err());
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
        LabelOperand::parse(&add.labels[0], true).unwrap(),
        LabelOperand::Pair {
            key: "branch".to_owned(),
            value: Some("feat,exp".to_owned())
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

    assert_eq!(rm.labels, ["active"]);
    assert_eq!(
        LabelOperand::parse(&rm.labels[0], false).unwrap(),
        LabelOperand::Pair {
            key: "active".to_owned(),
            value: None
        }
    );
}

/// Only the verbs that apply values need the target's own configuration,
/// because only they can carry an alias.
#[test]
fn only_the_applying_verbs_layer_the_target_config() {
    for args in [&["label", "add", "a=1"][..], &["label", "set", "a=1"][..]] {
        assert_eq!(
            parse(args).conversation_load_request().config_conversation,
            Some(0),
            "got: {args:?}"
        );
    }

    for args in [&["label", "rm", "a"][..], &["label"][..]] {
        assert_eq!(
            parse(args).conversation_load_request().config_conversation,
            None,
            "got: {args:?}"
        );
    }
}

// ── Reported output ──────────────────────────────────────────────────────────

/// Values grouped by key, the way the command groups its operands.
fn grouped(pairs: &[(&str, &[&str])]) -> Grouped {
    pairs
        .iter()
        .map(|(key, values)| {
            (
                (*key).to_owned(),
                values.iter().map(|value| (*value).to_owned()).collect(),
            )
        })
        .collect()
}

fn labels(pairs: &[(&str, &[&str])]) -> Labels {
    pairs
        .iter()
        .map(|(key, values)| (*key, values.iter().copied()))
        .collect()
}

const fn verb_of(change: &LabelChange) -> Verb {
    match change {
        LabelChange::Add(_) => Verb::Add,
        LabelChange::Set(_) => Verb::Set,
        LabelChange::Remove(_) | LabelChange::RemoveAll => Verb::Remove,
    }
}

/// Apply `change` to a conversation holding `before`, and return what the
/// command writes: the chrome for stderr, and the label lines for stdout.
fn report_of(before: &[(&str, &[&str])], change: &LabelChange) -> (String, Vec<String>) {
    let mut labels = labels(before);
    let applied = label::apply(&mut labels, change);
    let report = Report::new(verb_of(change));

    (
        strip_str(report.chrome(&applied.changes, "jp-c123: Title Here")),
        diff_lines(&applied.changes),
    )
}

/// Every verb reads the same way: `+` for a value the key gained, `-` for one
/// it lost, a space for one it kept.
#[test]
fn an_add_marks_what_it_gained_and_what_was_already_there() {
    let (chrome, lines) = report_of(
        &[("crate", &["jp_config"])],
        &LabelChange::Add(grouped(&[("crate", &["jp_llm"])])),
    );

    assert_eq!(chrome, "Added labels to jp-c123: Title Here");
    assert_eq!(lines, [" crate=jp_config", "+crate=jp_llm"]);
}

/// A `set` reads as a diff, so the values it displaced are on stdout with the
/// ones it applied, and the set stays undoable from its own output.
#[test]
fn a_set_reads_as_a_diff() {
    let (chrome, lines) = report_of(
        &[("crate", &["jp_config", "jp_llm"])],
        &LabelChange::Set(grouped(&[("crate", &["jp_cli"])])),
    );

    assert_eq!(chrome, "Set labels on jp-c123: Title Here");
    assert_eq!(lines, [
        "-crate=jp_config",
        "-crate=jp_llm",
        "+crate=jp_cli"
    ]);
}

/// A `set` over an absent key takes nothing away, so it is all additions.
#[test]
fn a_set_over_an_absent_key_is_all_additions() {
    let (chrome, lines) = report_of(&[], &LabelChange::Set(grouped(&[("crate", &["jp_cli"])])));

    assert_eq!(chrome, "Set labels on jp-c123: Title Here");
    assert_eq!(lines, ["+crate=jp_cli"]);
}

/// A value the `set` kept is context: it neither went nor arrived, but the
/// reader still sees what the key holds.
#[test]
fn a_set_marks_the_value_it_kept() {
    let (_, lines) = report_of(
        &[("crate", &["jp_config", "jp_llm"])],
        &LabelChange::Set(grouped(&[("crate", &["jp_config", "jp_cli"])])),
    );

    assert_eq!(lines, [
        " crate=jp_config",
        "-crate=jp_llm",
        "+crate=jp_cli"
    ]);
}

/// An invocation that named a key without changing it did nothing, and says so
/// rather than printing context lines that read as a change.
#[test]
fn a_set_that_changed_nothing_says_so() {
    let (chrome, lines) = report_of(
        &[("crate", &["jp_cli"])],
        &LabelChange::Set(grouped(&[("crate", &["jp_cli"])])),
    );

    assert_eq!(chrome, "No labels to apply to jp-c123: Title Here");
    assert!(lines.is_empty());
}

/// `jp c label add draft` asks for a presence the key already has once it holds
/// a real value, so there is nothing to apply.
#[test]
fn adding_a_bare_label_to_a_key_that_holds_values_says_so() {
    let (chrome, lines) = report_of(
        &[("foo", &["bar", "baz"])],
        &LabelChange::Add(grouped(&[("foo", &[""])])),
    );

    assert_eq!(chrome, "No labels to apply to jp-c123: Title Here");
    assert!(lines.is_empty());
}

/// A key that did not change is still context for one that did, so the reader
/// sees what the changed key sits alongside.
#[test]
fn an_unchanged_key_is_context_when_another_key_changed() {
    let (chrome, lines) = report_of(
        &[("crate", &["jp_cli"])],
        &LabelChange::Add(grouped(&[("crate", &["jp_cli"]), ("draft", &[""])])),
    );

    assert_eq!(chrome, "Added labels to jp-c123: Title Here");
    assert_eq!(lines, [" crate=jp_cli", "+draft"]);
}

#[test]
fn a_removal_marks_what_went_and_what_stayed() {
    let (chrome, lines) = report_of(
        &[("crate", &["jp_config", "jp_llm"])],
        &LabelChange::Remove(grouped(&[("crate", &["jp_llm"])])),
    );

    assert_eq!(chrome, "Removed labels from jp-c123: Title Here");
    assert_eq!(lines, [" crate=jp_config", "-crate=jp_llm"]);
}

/// A bare label is the key by itself, behind the marker.
#[test]
fn a_bare_label_reads_as_the_key_alone() {
    let (_, lines) = report_of(&[], &LabelChange::Add(grouped(&[("draft", &[""])])));
    assert_eq!(lines, ["+draft"]);

    let (_, lines) = report_of(
        &[("draft", &[""])],
        &LabelChange::Remove(grouped(&[("draft", &[])])),
    );
    assert_eq!(lines, ["-draft"]);

    let (_, lines) = report_of(&[], &LabelChange::Set(grouped(&[("draft", &[""])])));
    assert_eq!(lines, ["+draft"]);
}

/// The whole point of the bare form: what it took is what you need to put back.
#[test]
fn clearing_everything_lists_what_was_there() {
    let (chrome, lines) = report_of(
        &[
            ("crate", &["jp_llm"][..]),
            ("draft", &[""][..]),
            ("team", &["platform"][..]),
        ],
        &LabelChange::RemoveAll,
    );

    assert_eq!(chrome, "Removed labels from jp-c123: Title Here");
    assert_eq!(lines, ["-crate=jp_llm", "-draft", "-team=platform"]);
}

/// A removal that matched nothing says so rather than printing an empty list.
#[test]
fn a_removal_that_matched_nothing_is_reported_as_such() {
    let (chrome, lines) = report_of(&[], &LabelChange::RemoveAll);

    assert_eq!(chrome, "No labels to remove from jp-c123: Title Here");
    assert!(lines.is_empty());
}

/// Every alias can be declined at its prompt, leaving an `add` with nothing to
/// apply; it says so rather than borrowing the removal wording.
#[test]
fn an_add_with_nothing_to_apply_says_so() {
    let (chrome, lines) = report_of(&[], &LabelChange::Add(Grouped::new()));

    assert_eq!(chrome, "No labels to apply to jp-c123: Title Here");
    assert!(lines.is_empty());
}

/// The sentence is chrome and the labels are data, so they go to separate
/// streams: `jp c label add … | …` reads labels with no prose to strip.
/// The blank line between them is for the reader watching both.
#[test]
fn the_chrome_and_the_labels_go_to_separate_streams() {
    let (printer, out, err) = Printer::memory(OutputFormat::Text);
    let mut map = Labels::default();
    let change = LabelChange::Add(grouped(&[("crate", &["jp_llm"]), ("draft", &[""])]));
    let applied = label::apply(&mut map, &change);
    let report = Report::new(Verb::Add);

    report.print_chrome(&printer, &applied.changes, "jp-c123: Title Here");
    for line in diff_lines(&applied.changes) {
        printer.println(line);
    }
    printer.flush();

    assert_eq!(*err.lock(), "Added labels to jp-c123: Title Here\n\n");
    assert_eq!(*out.lock(), "+crate=jp_llm\n+draft\n");
}

/// The JSON form exposes the parts a script needs, rather than a sentence to
/// parse.
#[test]
fn the_json_form_carries_both_sides_of_each_change() {
    let mut map = labels(&[("crate", &["jp_config", "jp_llm"])]);
    let change = LabelChange::Set(grouped(&[("crate", &["jp_cli"])]));
    let applied = label::apply(&mut map, &change);

    let report = Report::new(Verb::Set);
    let id: ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(
        report.json(&applied.changes, id, Some("Tool Chaining")),
        serde_json::json!({
            "action": "set",
            "conversation": { "id": "jp-c17866928997", "title": "Tool Chaining" },
            "changes": [
                { "key": "crate", "before": ["jp_config", "jp_llm"], "after": ["jp_cli"] },
            ],
        })
    );
}

/// A bare label carries no values a reader can act on, so both sides are empty
/// arrays rather than an empty string a script would have to know about.
#[test]
fn the_json_form_leaves_a_bare_labels_encoding_out() {
    let mut map = Labels::default();
    let change = LabelChange::Add(grouped(&[("draft", &[""])]));
    let applied = label::apply(&mut map, &change);

    let report = Report::new(Verb::Add);
    let id: ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(
        report.json(&applied.changes, id, None),
        serde_json::json!({
            "action": "add",
            "conversation": { "id": "jp-c17866928997", "title": null },
            "changes": [{ "key": "draft", "before": [], "after": [] }],
        })
    );
}

/// An invocation that matched nothing is an empty array, not prose.
/// An untitled conversation reports a null title rather than omitting the
/// field.
#[test]
fn the_json_form_stays_structured_when_nothing_matched() {
    let mut map = Labels::default();
    let applied = label::apply(&mut map, &LabelChange::RemoveAll);

    let report = Report::new(Verb::Remove);
    let id: ConversationId = "jp-c17866928997".parse().unwrap();

    assert_eq!(
        report.json(&applied.changes, id, None),
        serde_json::json!({
            "action": "rm",
            "conversation": { "id": "jp-c17866928997", "title": null },
            "changes": [],
        })
    );
}

/// Past a handful of targets the report collapses to the keys it touched: each
/// conversation held something different, so there is no single before-and-
/// after to name.
#[test]
fn the_collapsed_line_names_every_key_touched() {
    let change = LabelChange::RemoveAll;
    let mut report = Report::new(Verb::Remove);

    for before in [
        &[("crate", &["jp_llm"][..]), ("team", &["platform"][..])][..],
        &[("crate", &["jp_cli"][..]), ("draft", &[""][..])][..],
    ] {
        let mut map = labels(before);
        let applied = label::apply(&mut map, &change);
        report.record(&applied.changes);
    }

    assert_eq!(
        strip_str(report.collapsed_line("2 conversations")),
        "Removed labels crate, team, draft from 2 conversations"
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
