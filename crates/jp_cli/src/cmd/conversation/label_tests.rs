use clap::Parser as _;

use super::*;
use crate::cmd::{conversation_id::ConversationIds as _, target::ConversationTarget};

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
        parse(&["label", "reset"]).command,
        Some(Commands::Reset)
    ));
    assert!(matches!(
        parse(&["label", "ls"]).command,
        Some(Commands::Ls)
    ));
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

/// A mutation with nothing to mutate is a mistake, not a no-op.
#[test]
fn add_and_rm_require_at_least_one_argument() {
    assert!(Cmd::try_parse_from(["label", "add"]).is_err());
    assert!(Cmd::try_parse_from(["label", "rm"]).is_err());
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
