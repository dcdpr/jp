use assert_matches::assert_matches;
use jp_config::{
    AppConfig,
    providers::llm::{AuthEntry, anthropic::SubscriptionFlow},
};
use test_log::test;

use super::*;
use crate::{
    Error as LlmError,
    provider::anthropic::{
        Anthropic,
        acp::schema::SessionConfigSelectOption,
        resolve::{self, Route},
    },
};

#[test]
fn qualified_runtime_pair() {
    qualify_versions(b"0.102.0\n", b"2.1.321 (Claude Code)\n").unwrap();
    assert_matches!(
        qualify_versions(b"0.103.0", b"2.1.321"),
        Err(Error::UnsupportedVersion {
            check: Check::AdapterVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.102.0", b"2.1.322"),
        Err(Error::UnsupportedVersion {
            check: Check::ClaudeVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.102.0", b"2.1.321-extra"),
        Err(Error::UnsupportedVersion { .. })
    );
}

#[test]
fn subscription_status_is_fail_closed() {
    let login = || Login::Named("/accounts/personal".into());
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","email":"ignored"}"#, login()).unwrap();
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"Claude Pro"}"#, login()).unwrap();
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"ultra"}"#, login()).unwrap();
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"Claude Ultra"}"#, login()).unwrap();
    for (status, expected) in [
        (r#"{"loggedIn":false}"#, Rejection::SignedOut),
        (r#"{"loggedIn":true}"#, Rejection::NotClaudeAccount),
        (
            r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty","subscriptionType":"max"}"#,
            Rejection::NotClaudeAccount,
        ),
        (
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"unknown"}"#,
            Rejection::NoPlan,
        ),
        (
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"bedrock","subscriptionType":"max"}"#,
            Rejection::ThirdParty,
        ),
        (
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","apiKeySource":"apiKeyHelper"}"#,
            Rejection::ApiKey {
                origin: "apiKeyHelper".into(),
            },
        ),
    ] {
        assert_matches!(
            validate_auth(status.as_bytes(), login()),
            Err(Error::LoginRejected { reason, .. }) if reason == expected,
            "{status}"
        );
    }
    assert_matches!(
        validate_auth(b"not json", login()),
        Err(Error::AuthStatus(_))
    );
}

#[test]
fn rejected_inherited_login_points_at_named_subscriptions() {
    let error = validate_auth(br#"{"loggedIn":false}"#, Login::Inherited(None)).unwrap_err();
    assert_eq!(
        error.to_string(),
        "Claude Code's inherited login at ~/.claude cannot serve subscription requests: it is \
         signed out; the `auth` chain named no subscription, so pass `--auth sub:<name>` to use a \
         registered login (`jp provider llm auth list` shows them), or sign in with \
         `claude-agent-acp --cli auth login --claudeai`"
    );
}

#[test]
fn rejected_named_login_says_how_to_sign_in_again() {
    let error = validate_auth(
        br#"{"loggedIn":false}"#,
        Login::Named("/accounts/personal".into()),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "the Claude Code login at /accounts/personal cannot serve subscription requests: it is \
         signed out; sign in again with `jp provider llm auth login anthropic --name <name> \
         --config-dir /accounts/personal`"
    );
}

#[test]
fn rejected_api_key_login_names_the_key_source() {
    let error = validate_auth(
        br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","apiKeySource":"apiKeyHelper"}"#,
        Login::Inherited(Some("/custom/claude".into())),
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Claude Code's inherited login at /custom/claude (from CLAUDE_CONFIG_DIR) cannot serve \
         subscription requests: it authenticates with an API key from `apiKeyHelper`; the `auth` \
         chain named no subscription, so pass `--auth sub:<name>` to use a registered login (`jp \
         provider llm auth list` shows them), or remove that API key setting from Claude Code's \
         settings"
    );
}

fn offered(values: &[(&str, &str)]) -> Vec<SessionConfigSelectOption> {
    values
        .iter()
        .map(|(value, name)| SessionConfigSelectOption {
            value: (*value).into(),
            name: Some((*name).to_owned()),
        })
        .collect()
}

/// The values and labels Claude Code's `model` setting offered when
/// `live.jsonl` was recorded.
const RECORDED: &[(&str, &str)] = &[
    ("default", "Default (recommended)"),
    ("opus", "Opus 5.5"),
    ("fable", "Fable 5.2"),
    ("sonnet", "Sonnet 5.5"),
    ("haiku", "Haiku 5.5"),
    ("claude-fable-5-1", "Fable 5.1"),
    ("claude-haiku-4-5-20251001", "Haiku 4.5"),
    ("claude-sonnet-5", "Sonnet 5"),
    ("claude-opus-5", "Opus 5"),
    ("claude-fable-5", "Fable 5"),
    ("claude-opus-4-8", "Opus 4.8"),
    ("claude-opus-4-7", "Opus 4.7"),
    ("claude-opus-4-6", "Opus 4.6"),
    ("claude-sonnet-4-6", "Sonnet 4.6"),
];

#[test]
fn an_api_model_is_sent_as_the_offer_with_its_version() {
    let offers = offered(RECORDED);

    assert_eq!(subscription_model("claude-opus-5-5", &offers), "opus");
    assert_eq!(
        subscription_model("claude-opus-5", &offers),
        "claude-opus-5"
    );
    assert_eq!(subscription_model("claude-fable-5-2", &offers), "fable");
    assert_eq!(
        subscription_model("claude-fable-5-1", &offers),
        "claude-fable-5-1"
    );
    assert_eq!(subscription_model("claude-sonnet-5-5", &offers), "sonnet");
    assert_eq!(
        subscription_model("claude-sonnet-5", &offers),
        "claude-sonnet-5"
    );
    // Date suffixes are ignored on both the offered option and the requested
    // model name.
    assert_eq!(
        subscription_model("claude-haiku-4-5", &offers),
        "claude-haiku-4-5-20251001"
    );
    assert_eq!(
        subscription_model("claude-opus-4-7-20260416", &offers),
        "claude-opus-4-7"
    );
}

#[test]
fn a_dotted_version_matches_a_dashed_one() {
    let offers = offered(RECORDED);

    assert_eq!(subscription_model("claude-fable-5.2", &offers), "fable");
    assert_eq!(
        subscription_model("claude-opus-4.8", &offers),
        "claude-opus-4-8"
    );
}

#[test]
fn without_its_version_a_model_is_sent_as_its_family() {
    let offers = offered(RECORDED);

    assert_eq!(subscription_model("claude-opus-9", &offers), "opus");
    assert_eq!(subscription_model("claude-opus-4-1", &offers), "opus");
    assert_eq!(subscription_model("claude-fable-6", &offers), "fable");
}

#[test]
fn several_offers_of_a_version_prefer_the_1m_variant_then_the_bare_alias() {
    assert_eq!(
        subscription_model(
            "claude-opus-5-5",
            &offered(&[("opus", "Opus 5.5"), ("opus[1m]", "Opus 5.5")])
        ),
        "opus[1m]"
    );
    assert_eq!(
        subscription_model(
            "claude-opus-9",
            &offered(&[("claude-opus-4-8", "Opus 4.8"), ("opus", "Opus 5.5")])
        ),
        "opus"
    );
}

#[test]
fn without_a_matching_offer_the_api_name_is_sent() {
    assert_eq!(subscription_model("claude-opus-5", &[]), "claude-opus-5");

    let offers = offered(&[("opus", "Opus 5.5"), ("sonnet", "Sonnet 5.5")]);
    assert_eq!(
        subscription_model("claude-haiku-4-5", &offers),
        "claude-haiku-4-5"
    );
    assert_eq!(subscription_model("foobarbaz", &offers), "foobarbaz");
}

#[test]
fn listed_models_are_named_by_their_api_id() {
    let mut offers = offered(RECORDED);
    offers.push(SessionConfigSelectOption {
        value: "some-future-family".into(),
        name: None,
    });
    // A second Opus 5.5 maps to the same API model, so it is listed once.
    offers.push(SessionConfigSelectOption {
        value: "opus[1m]".into(),
        name: Some("Opus 5.5".to_owned()),
    });

    let models = served_models(&offers);

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.to_string())
            .collect::<Vec<_>>(),
        [
            // Bare aliases resolve to the version in their display label, even
            // if uncataloged.
            "anthropic/claude-opus-5-5",
            "anthropic/claude-fable-5-2",
            "anthropic/claude-sonnet-5-5",
            "anthropic/claude-haiku-5-5",
            "anthropic/claude-fable-5-1",
            // Dated values resolve to their cataloged model ID.
            "anthropic/claude-haiku-4-5",
            "anthropic/claude-sonnet-5",
            "anthropic/claude-opus-5",
            "anthropic/claude-fable-5",
            "anthropic/claude-opus-4-8",
            "anthropic/claude-opus-4-7",
            "anthropic/claude-opus-4-6",
            "anthropic/claude-sonnet-4-6",
        ]
    );
    assert!(models.iter().all(ModelDetails::served_by_subscription));
}

#[test]
fn a_listed_model_round_trips_to_its_offer() {
    let offers = offered(RECORDED);

    for model in served_models(&offers) {
        let sent = subscription_model(&model.id.name, &offers);
        let label = offers
            .iter()
            .find(|option| option.value.0 == sent)
            .and_then(|option| option.name.as_deref());
        assert_eq!(
            label,
            model.display_name.as_deref(),
            "{} was sent as {sent}",
            model.id
        );
    }
}

#[test]
fn an_unlabelled_alias_is_the_newest_model_in_its_family() {
    let models = served_models(&[SessionConfigSelectOption {
        value: "sonnet".into(),
        name: None,
    }]);

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.to_string())
            .collect::<Vec<_>>(),
        ["anthropic/claude-sonnet-5-5"]
    );
}

#[test]
fn a_listed_model_keeps_its_api_models_overrides() {
    let [fable] = served_models(&offered(&[("claude-fable-5-1[1m]", "Fable 5.1")]))
        .try_into()
        .unwrap();

    assert!(fable.reasoning.is_some());
}

#[test]
fn runtime_identity_does_not_mistake_an_organization_for_an_account() {
    let identity = subscription_identity(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","email":"first@example.com","orgId":"11111111-1111-1111-1111-111111111111"}"#).unwrap().unwrap();
    assert_eq!(identity, AccountIdentity {
        account_id: None,
        email: Some("first@example.com".into())
    });
    assert_eq!(
        subscription_identity(br#"{"loggedIn":false}"#).unwrap(),
        None
    );
}

#[test]
fn lifecycle_commands_select_subscription_login_and_logout() {
    assert_eq!(Check::Login.args(), &[
        "--cli",
        "auth",
        "login",
        "--claudeai"
    ]);
    assert_eq!(Check::Logout.args(), &["--cli", "auth", "logout"]);
}

#[cfg(unix)]
#[test(tokio::test)]
async fn signed_out_status_json_survives_exit_one() {
    let mut command = Command::new("sh");
    command.args(["-c", r#"printf '{"loggedIn":false}'; exit 1"#]);
    let output = read_output(command, Check::Authentication).await.unwrap();
    assert_eq!(output, br#"{"loggedIn":false}"#);
    assert_eq!(subscription_identity(&output).unwrap(), None);
}

#[cfg(unix)]
#[test(tokio::test)]
async fn failed_logout_does_not_accept_exit_one() {
    let mut command = Command::new("sh");
    command.args(["-c", "exit 1"]);
    assert_matches!(read_output(command, Check::Logout).await, Err(Error::CommandFailed { check: Check::Logout, status }) if status.code() == Some(1));
}

#[cfg(unix)]
#[test(tokio::test)]
async fn failed_status_command_cannot_report_a_usable_login() {
    let mut command = Command::new("sh");
    command.args(["-c", r#"printf '{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max"}'; exit 1"#]);
    assert_matches!(read_output(command, Check::Authentication).await, Err(Error::CommandFailed { check: Check::Authentication, status }) if status.code() == Some(1));
}

#[test]
fn model_names_are_not_restricted_to_the_probe_model() {
    let model = model_details(&"claude-opus-5".parse().unwrap());
    assert_eq!(model.id.to_string(), "anthropic/claude-opus-5");
    assert_eq!(model.subscription, Some(true));
    assert_eq!(model.context_window, None);
    assert_eq!(model.prefill, Some(false));
    for name in [
        "claude-haiku-4-5",
        "claude-sonnet-4-6",
        "haiku",
        "future-model",
    ] {
        let name = name.parse().unwrap();
        let model = model_details(&name);
        assert_eq!(model.id.name, name);
    }
}

#[test(tokio::test)]
async fn subscription_construction_uses_no_store_or_external_runtime() {
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.auth = vec![AuthEntry::Subscription(None)];
    let provider = Anthropic::new(&config).unwrap();
    assert!(!resolve::needs_store(&config));
    assert_matches!(
        provider.resolve("claude-opus-5").await.unwrap().route,
        Route::Acp(None)
    );
}

#[test]
fn api_construction_ignores_subscription_flow() {
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.api_key_env = "JP_TEST_PHASE1_MISSING_API_KEY".into();
    for flow in [SubscriptionFlow::Acp, SubscriptionFlow::Direct] {
        config.subscription_flow = flow;
        assert!(!resolve::needs_store(&config));
        assert_matches!(Anthropic::new(&config), Err(LlmError::MissingEnv(variable)) if variable == "JP_TEST_PHASE1_MISSING_API_KEY");
    }
}

#[test]
fn child_environment_filter_preserves_native_login_location() {
    assert!(removes_variable("ANTHROPIC_API_KEY"));
    assert!(removes_variable("anthropic_api_key"));
    assert!(removes_variable("FORCE_PROMPT_CACHING_5M"));
    assert!(removes_variable("ENABLE_PROMPT_CACHING_1H"));
    assert!(removes_variable("ANTHROPIC_BASE_URL"));
    assert!(removes_variable("CLAUDE_CODE_USE_BEDROCK"));
    assert!(removes_variable("CLAUDE_CODE_OAUTH_TOKEN"));
    assert!(removes_variable("DISABLE_PROMPT_CACHING_OPUS"));
    assert!(removes_variable("DISABLE_PROMPT_CACHING"));
    assert!(removes_variable("CLAUDE_CODE_PROMPT_CACHE_TTL"));
    assert!(!removes_variable("HOME"));
    assert!(!removes_variable("CLAUDE_CONFIG_DIR"));
    assert!(!removes_variable("PATH"));
}

#[cfg(unix)]
#[test(tokio::test)]
async fn inspection_output_is_bounded() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf '%65537s' ''"]);
    assert_matches!(
        read_output(command, Check::AdapterVersion).await,
        Err(Error::OutputLimit {
            check: Check::AdapterVersion
        })
    );
}

#[test(tokio::test)]
async fn missing_runtime_has_setup_guidance() {
    let command = Command::new("jp-test-nonexistent-claude-agent-acp");
    let error = read_output(command, Check::AdapterVersion)
        .await
        .unwrap_err();
    assert_matches!(&error, Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound);
    assert_eq!(
        error.to_string(),
        "Claude ACP adapter-version check failed; install \
         @agentclientprotocol/claude-agent-acp@0.102.0 with Node.js 22+ and optional dependencies \
         enabled"
    );
}

#[cfg(unix)]
#[test(tokio::test)]
async fn command_failure_is_typed_and_does_not_include_output() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf private-diagnostic; exit 7"]);
    let error = read_output(command, Check::Authentication)
        .await
        .unwrap_err();
    assert_matches!(error, Error::CommandFailed { status, .. } if status.code() == Some(7));
    assert_eq!(
        error.to_string(),
        "Claude ACP authentication check exited with exit status: 7; check `claude-agent-acp \
         --cli auth status --json`"
    );
}
