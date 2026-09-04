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
/// prompt has to stop, and stop having written nothing.
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
             available to answer; run it without --no-interactive"
        )
    );
    assert!(!root.exists(), "a refused init created the workspace root");
}

/// Everything `init` writes, it writes here — and only once the wizard has
/// handed over every answer.
/// A directory carrying `.jp/.id` reads as a workspace to every later command,
/// so the ID and the config it needs are written in one go.
#[test]
fn create_workspace_writes_the_id_and_the_collected_answers() {
    let tmp = tempdir().unwrap();
    let root = tmp.path().join("project");

    Init::create_workspace(
        &root,
        RunMode::Ask,
        ProviderId::Anthropic,
        &Name("claude-sonnet-4".to_owned()),
    )
    .unwrap();

    let storage = root.join(".jp");
    assert!(storage.join(".id").exists(), "no workspace ID was stored");
    assert_eq!(
        fs::read_to_string(storage.join("config.toml")).unwrap(),
        "[assistant.model.id]\nprovider = \"anthropic\"\nname = \
         \"claude-sonnet-4\"\n\n[conversation.tools.'*']\nrun = \"ask\"\n"
    );
}
