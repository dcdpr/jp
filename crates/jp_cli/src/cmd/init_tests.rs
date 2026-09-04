use camino_tempfile::tempdir;
use jp_printer::{OutputFormat, Printer};
use schematic::{SchemaBuilder, Schematic as _};

use super::*;

#[test]
fn test_app_config_schema_serializes_to_json() {
    let builder = SchemaBuilder::default();
    let schema = jp_config::AppConfig::build_schema(builder);
    serde_json::to_string_pretty(&schema)
        .expect("AppConfig schema should serialize to JSON without errors");
}

/// The wizard is the only way to answer what `init` needs, so a run that cannot
/// prompt has to stop.
/// It stops before creating anything: the directory and the workspace ID are
/// written before the first question, and a workspace holding an ID but no
/// config is worse than no workspace at all.
#[test]
fn init_without_a_user_writes_nothing() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("project");

    let (printer, _, _) = Printer::memory(OutputFormat::Text);
    let init = Init {
        path: Some(root.clone()),
    };

    let error = init
        .run(&printer, true)
        .expect_err("the wizard cannot run without a user");
    assert_eq!(
        error.message.as_deref(),
        Some(
            "`jp init` asks which model to use and whether to confirm tool calls, and nobody is \
             available to answer; run it without --non-interactive"
        )
    );
    assert!(!root.exists(), "a refused init created the workspace root");
}
