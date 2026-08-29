use std::str::FromStr as _;

use super::*;

/// Render an error with its full source chain via `Display`.
fn message_of(error: &cmd::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        message.push_str(": ");
        message.push_str(&inner.to_string());
        source = inner.source();
    }
    message
}

fn target(s: &str) -> WorkspaceTarget {
    WorkspaceTarget::from_str(s).unwrap()
}

#[test]
fn global_flag_supplies_an_absent_target() {
    let global = target("ws123");

    assert_eq!(
        target_for("jp w show", None, Some(&global)).unwrap(),
        Some(global)
    );
}

#[test]
fn positional_target_stands_alone() {
    let positional = target("ws123");

    assert_eq!(
        target_for("jp w show", Some(positional.clone()), None).unwrap(),
        Some(positional)
    );
}

#[test]
fn naming_the_workspace_twice_is_rejected() {
    // Honoring either one silently would hide the other; the flag spells the
    // positional target, it does not override it.
    let error = target_for("jp w show", Some(target("ws123")), Some(&target("ws456"))).unwrap_err();

    assert!(
        message_of(&error).contains("Name the workspace once"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn no_target_at_all_stays_absent() {
    assert_eq!(target_for("jp w use", None, None).unwrap(), None);
}

#[test]
fn use_adopts_the_global_flag_as_its_target() {
    let command = Commands::Use(use_::Use { target: None })
        .with_global_target(Some(&target("ws123")))
        .unwrap();

    let Commands::Use(args) = command else {
        panic!("expected the `use` subcommand");
    };
    assert_eq!(args.target, Some(target("ws123")));
}

#[test]
fn show_adopts_the_global_flag_as_its_target() {
    let command = Commands::Show(show::Show { target: None })
        .with_global_target(Some(&target("ws123")))
        .unwrap();

    let Commands::Show(args) = command else {
        panic!("expected the `show` subcommand");
    };
    assert_eq!(args.target, Some(target("ws123")));
}

#[test]
fn ls_rejects_the_global_flag() {
    // `ls` has no target semantics, so the flag can only be a mistake — and
    // silently ignoring it is what makes such mistakes hard to spot.
    let error = Commands::Ls(ls::Ls {})
        .with_global_target(Some(&target("ws123")))
        .unwrap_err();

    assert!(
        message_of(&error).contains("nothing to select"),
        "unexpected error: {error:?}"
    );
}

#[test]
fn ls_without_the_flag_is_untouched() {
    assert!(matches!(
        Commands::Ls(ls::Ls {}).with_global_target(None).unwrap(),
        Commands::Ls(_)
    ));
}
