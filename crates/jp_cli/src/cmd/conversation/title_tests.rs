use clap::Parser as _;
use jp_config::{PartialAppConfig, model::id::PartialModelIdOrAliasConfig};

use super::{DEFAULT_COUNT, IntoPartialAppConfig as _, Title};

/// Parse a `Title` from `jp conversation title <args>` for flag tests.
fn parse_title(args: &[&str]) -> Title {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        title: Title,
    }

    let mut argv = vec!["title"];
    argv.extend_from_slice(args);
    TestCli::try_parse_from(argv).unwrap().title
}

#[test]
fn count_defaults_to_three() {
    assert_eq!(parse_title(&[]).count, DEFAULT_COUNT);
}

/// Generating zero titles has no meaningful outcome, so it is rejected at parse
/// time rather than producing an empty picker.
#[test]
fn count_of_zero_is_rejected() {
    #[derive(clap::Parser)]
    struct TestCli {
        #[command(flatten)]
        title: Title,
    }

    assert!(TestCli::try_parse_from(["title", "--count", "0"]).is_err());
}

/// `--model` reaches the title generator through `assistant.model`, which is
/// where `resolve_model`'s override argument reads it from.
#[test]
fn model_flag_lands_on_the_assistant_model() {
    let title = parse_title(&["--model", "anthropic/claude-haiku-4-5"]);
    let partial = title
        .apply_cli_config(None, PartialAppConfig::empty(), None)
        .unwrap();

    assert_eq!(
        partial.assistant.model.id,
        PartialModelIdOrAliasConfig::from("anthropic/claude-haiku-4-5")
    );
}

#[test]
fn without_the_model_flag_the_assistant_model_is_untouched() {
    let title = parse_title(&[]);
    let partial = title
        .apply_cli_config(None, PartialAppConfig::empty(), None)
        .unwrap();

    assert_eq!(
        partial.assistant.model.id,
        PartialAppConfig::empty().assistant.model.id
    );
}
