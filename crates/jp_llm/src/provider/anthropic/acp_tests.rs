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
        resolve::{self, Route},
    },
};

#[test]
fn qualified_runtime_pair() {
    qualify_versions(b"0.81.0\n", b"2.1.280 (Claude Code)\n").unwrap();
    assert_matches!(
        qualify_versions(b"0.82.0", b"2.1.280"),
        Err(Error::UnsupportedVersion {
            check: Check::AdapterVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.81.0", b"2.1.281"),
        Err(Error::UnsupportedVersion {
            check: Check::ClaudeVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.81.0", b"2.1.280-extra"),
        Err(Error::UnsupportedVersion { .. })
    );
}

#[test]
fn subscription_status_is_fail_closed() {
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","email":"ignored"}"#).unwrap();
    validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"Claude Pro"}"#).unwrap();
    assert_matches!(validate_auth(br#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty","subscriptionType":"max"}"#), Err(Error::SubscriptionRequired));
    assert_matches!(validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"bedrock","subscriptionType":"max"}"#), Err(Error::SubscriptionRequired));
    assert_matches!(
        validate_auth(br#"{"loggedIn":false}"#),
        Err(Error::SubscriptionRequired)
    );
    assert_matches!(
        validate_auth(br#"{"loggedIn":true}"#),
        Err(Error::SubscriptionRequired)
    );
    assert_matches!(validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"unknown"}"#), Err(Error::SubscriptionRequired));
    assert_matches!(validate_auth(br#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"max","apiKeySource":"apiKeyHelper"}"#), Err(Error::SubscriptionRequired));
    assert_matches!(validate_auth(b"not json"), Err(Error::AuthStatus(_)));
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
         @agentclientprotocol/claude-agent-acp@0.81.0 with Node.js 22+ and optional dependencies \
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
