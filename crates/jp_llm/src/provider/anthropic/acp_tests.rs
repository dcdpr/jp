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
    qualify_versions(b"0.76.0\n", b"2.1.257 (Claude Code)\n").unwrap();
    assert_matches!(
        qualify_versions(b"0.77.0", b"2.1.257"),
        Err(Error::UnsupportedVersion {
            check: Check::AdapterVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.76.0", b"2.1.258"),
        Err(Error::UnsupportedVersion {
            check: Check::ClaudeVersion,
            ..
        })
    );
    assert_matches!(
        qualify_versions(b"0.76.0", b"2.1.257-extra"),
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
fn model_qualification_does_not_substitute_aliases() {
    let model = model_details(&"claude-opus-5".parse().unwrap()).unwrap();
    assert_eq!(model.id.to_string(), "anthropic/claude-opus-5");
    assert_eq!(model.subscription, Some(true));
    assert_eq!(model.context_window, None);
    assert_eq!(model.prefill, Some(false));
    assert_matches!(
        model_details(&"opus".parse().unwrap()),
        Err(Error::UnsupportedModel { .. })
    );
}

#[test(tokio::test)]
async fn subscription_construction_uses_no_store_or_external_runtime() {
    let mut config = AppConfig::new_test().providers.llm.anthropic;
    config.auth = vec![AuthEntry::Subscription(None)];
    let provider = Anthropic::new(&config).unwrap();
    assert!(!resolve::needs_store(&config));
    assert_matches!(
        provider.resolve("claude-opus-5").await.unwrap().route,
        Route::Acp
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
    assert!(removes_variable("ANTHROPIC_BASE_URL"));
    assert!(removes_variable("CLAUDE_CODE_USE_BEDROCK"));
    assert!(removes_variable("CLAUDE_CODE_OAUTH_TOKEN"));
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
        read_output(&mut command, Check::AdapterVersion).await,
        Err(Error::OutputLimit {
            check: Check::AdapterVersion
        })
    );
}

#[test(tokio::test)]
async fn missing_runtime_has_setup_guidance() {
    let mut command = Command::new("jp-test-nonexistent-claude-agent-acp");
    let error = read_output(&mut command, Check::AdapterVersion)
        .await
        .unwrap_err();
    assert_matches!(&error, Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound);
    assert_eq!(
        error.to_string(),
        "Claude ACP adapter-version check failed; install \
         @agentclientprotocol/claude-agent-acp@0.76.0 with Node.js 22+ and optional dependencies \
         enabled"
    );
}

#[cfg(unix)]
#[test(tokio::test)]
async fn command_failure_is_typed_and_does_not_include_output() {
    let mut command = Command::new("sh");
    command.args(["-c", "printf private-diagnostic; exit 7"]);
    let error = read_output(&mut command, Check::Authentication)
        .await
        .unwrap_err();
    assert_matches!(error, Error::CommandFailed { status, .. } if status.code() == Some(7));
    assert_eq!(
        error.to_string(),
        "Claude ACP authentication check exited with exit status: 7; check `claude-agent-acp \
         --cli auth status --json`"
    );
}
