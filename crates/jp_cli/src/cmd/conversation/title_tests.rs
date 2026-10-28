use jp_config::{PartialAppConfig, model::id::PartialModelIdOrAliasConfig};

use super::{DEFAULT_COUNT, IntoPartialAppConfig as _, Title};
use crate::cli::parse_args;

/// Parse a `Title` from `jp conversation title <args>` for flag tests.
fn parse_title(args: &[&str]) -> Title {
    parse_args(args).unwrap()
}

#[test]
fn count_defaults_to_three() {
    assert_eq!(parse_title(&[]).count, DEFAULT_COUNT);
}

/// Generating zero titles has no meaningful outcome, so it is rejected at parse
/// time rather than producing an empty picker.
#[test]
fn count_of_zero_is_rejected() {
    let error = parse_args::<Title>(&["--count", "0"]).unwrap_err();

    assert_eq!(
        error.render().to_string(),
        "error: invalid value '0' for '--count <COUNT>': at least one title must be \
         generated\n\nFor more information, try '--help'.\n"
    );
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
