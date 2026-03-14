use schematic::{SchemaBuilder, Schematic as _};

#[test]
fn test_app_config_schema_serializes_to_json() {
    let builder = SchemaBuilder::default();
    let schema = jp_config::AppConfig::build_schema(builder);
    serde_json::to_string_pretty(&schema)
        .expect("AppConfig schema should serialize to JSON without errors");
}
