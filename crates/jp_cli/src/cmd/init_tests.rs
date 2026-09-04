use jp_config::model::id::ProviderId;
use schematic::{SchemaBuilder, Schematic as _};

use super::default_model_id_for;

#[test]
fn test_app_config_schema_serializes_to_json() {
    let builder = SchemaBuilder::default();
    let schema = jp_config::AppConfig::build_schema(builder);
    serde_json::to_string_pretty(&schema)
        .expect("AppConfig schema should serialize to JSON without errors");
}

#[test]
fn suggests_a_current_ga_model_for_supported_providers() {
    let id = |provider| default_model_id_for(provider).map(|id| id.name.to_string());

    assert_eq!(
        id(ProviderId::Anthropic).as_deref(),
        Some("claude-sonnet-5")
    );
    // A GA endpoint, deliberately: a preview one can be renamed or pulled out
    // from under a workspace that was initialized with it.
    assert_eq!(id(ProviderId::Google).as_deref(), Some("gemini-3.8-flash"));
    assert_eq!(id(ProviderId::Openai).as_deref(), Some("gpt-5.6-terra"));
    assert_eq!(id(ProviderId::Ollama), None);
}
