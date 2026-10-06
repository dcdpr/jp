//! A query draft left behind by an earlier run, reopened by a later one.
//!
//! The draft's config block is pre-filled from the config in effect when it was
//! written.
//! When a later run reopens the draft under a different config, the block must
//! show that run's config, with only the fields the user actually edited
//! carried over.
//! A pre-filled value the user never touched is not user intent, and must not
//! override what the later run resolved.

use std::{fs, sync::Mutex};

use camino_tempfile::Utf8TempDir;
use jp_config::{
    AppConfig, PartialAppConfig,
    fs::load_partial,
    model::{
        id::{PartialModelIdConfig, ProviderId},
        parameters::ReasoningConfig,
    },
    util::build,
};
use jp_conversation::ConversationStream;
use jp_editor::{EditOutcome, EditRequest, EditorBackend};

use super::{
    CUT_MARKER, EditorResult, QUERY_FILENAME, config_seed, edit_query, parser::QueryDocument,
    refresh_untouched,
};

/// An editor that records the file it is shown, optionally applies one textual
/// edit, and saves.
struct ScriptedEditor {
    /// `(from, to)`: replace `from` with `to` before saving.
    edit: Option<(&'static str, &'static str)>,

    /// The file content as it was when the editor opened it.
    shown: Mutex<String>,
}

impl ScriptedEditor {
    fn saving() -> Self {
        Self {
            edit: None,
            shown: Mutex::default(),
        }
    }

    fn replacing(from: &'static str, to: &'static str) -> Self {
        Self {
            edit: Some((from, to)),
            shown: Mutex::default(),
        }
    }

    /// The config block the user saw when the editor opened.
    fn shown_config_block(&self) -> String {
        let shown = self.shown.lock().unwrap();
        let doc = QueryDocument::try_from(shown.as_str()).unwrap();
        doc.meta.config.value.to_owned()
    }
}

impl EditorBackend for ScriptedEditor {
    fn edit_text(&self, content: &str) -> EditorResult<(EditOutcome, String)> {
        Ok((EditOutcome::Saved, content.to_owned()))
    }

    fn edit_file(&self, req: EditRequest<'_>) -> EditorResult<EditOutcome> {
        let path = &req.paths[0];
        let content = fs::read_to_string(path).unwrap();
        self.shown.lock().unwrap().clone_from(&content);

        if let Some((from, to)) = self.edit {
            assert!(content.contains(from), "`{from}` not in draft:\n{content}");
            fs::write(path, content.replace(from, to)).unwrap();
        }

        Ok(EditOutcome::Saved)
    }
}

fn config_with_model(name: &str) -> AppConfig {
    config_with_model_and(name, "")
}

/// A config running `name`, with the TOML in `extra` layered on top.
fn config_with_model_and(name: &str, extra: &str) -> AppConfig {
    let mut partial = AppConfig::new_test().to_partial();
    partial.assistant.model.id = PartialModelIdConfig {
        provider: Some(ProviderId::Anthropic),
        name: Some(name.parse().unwrap()),
    }
    .into();

    let extra = toml::from_str::<PartialAppConfig>(extra).unwrap();
    build(load_partial(partial, extra).unwrap()).unwrap()
}

/// The config the conversation ends up with once the editor's output is applied
/// on top of the run's own config.
fn applied(run: &AppConfig, editor_output: PartialAppConfig) -> AppConfig {
    build(load_partial(run.to_partial(), editor_output).unwrap()).unwrap()
}

/// A draft kept after a failed turn, reopened under a different model, with
/// nothing edited in either session.
///
/// This is `jp q` failing on a stale model, followed by `jp q -cWORKSPACE`: the
/// second run resolved a working model, and the untouched pre-filled block from
/// the first run put the stale one back.
#[test]
fn an_untouched_draft_block_follows_the_current_config() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();

    let stale = config_with_model("claude-opus-5.5");
    let editor = ScriptedEditor::saving();
    edit_query(&stale, dir.path(), &stream, "hello", &editor, None).unwrap();

    let current = config_with_model("claude-opus-5-5");
    let editor = ScriptedEditor::saving();
    let (query, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    assert_eq!(query, "hello");
    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5-5"

            [assistant.model.parameters]
            reasoning = "auto""#});
    assert_eq!(
        applied(&current, output)
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-opus-5-5"
    );
}

/// A draft written before drafts carried a seed has no record of what was
/// pre-filled, so its block is replaced by the current config.
///
/// This is the shape of every draft already on disk, including the one that
/// holds a stale model after a failed turn.
#[test]
fn a_draft_without_a_seed_takes_the_current_config() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();
    let draft = format!(
        indoc::indoc! {r#"
            hello

            {}

            # Active Configuration

            ```toml
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5.5"
            ```
        "#},
        CUT_MARKER
    );
    fs::write(dir.path().join(QUERY_FILENAME), draft).unwrap();

    let current = config_with_model("claude-opus-5-5");
    let editor = ScriptedEditor::saving();
    let (query, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    assert_eq!(query, "hello");
    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5-5"

            [assistant.model.parameters]
            reasoning = "auto""#});
    assert_eq!(
        applied(&current, output)
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-opus-5-5"
    );
}

/// A field the user edited in the first session survives the reopen, even
/// though the run's config changed underneath it.
#[test]
fn an_edited_draft_field_survives_a_config_change() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();

    let first = config_with_model("claude-opus-5");
    let editor =
        ScriptedEditor::replacing(r#"name = "claude-opus-5""#, r#"name = "claude-haiku-4-5""#);
    edit_query(&first, dir.path(), &stream, "hello", &editor, None).unwrap();

    let current = config_with_model("claude-opus-5-5");
    let editor = ScriptedEditor::saving();
    let (_, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-haiku-4-5"

            [assistant.model.parameters]
            reasoning = "auto""#});
    assert_eq!(
        applied(&current, output)
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-haiku-4-5"
    );
}

/// Editing one field of the block does not freeze the others: the model the
/// user never touched still follows the current config.
#[test]
fn editing_one_draft_field_leaves_the_others_following_the_config() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();

    let stale = config_with_model("claude-opus-5.5");
    let editor = ScriptedEditor::replacing(r#"reasoning = "auto""#, r#"reasoning = "off""#);
    edit_query(&stale, dir.path(), &stream, "hello", &editor, None).unwrap();

    let current = config_with_model("claude-opus-5-5");
    let editor = ScriptedEditor::saving();
    let (_, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5-5"

            [assistant.model.parameters]
            reasoning = "off""#});
    assert_eq!(
        applied(&current, output)
            .assistant
            .model
            .id
            .resolved()
            .to_string(),
        "anthropic/claude-opus-5-5"
    );
}

/// A kept draft whose block no longer parses is shown as it stands, and once
/// the user repairs it, the pre-filled fields they left alone still follow this
/// run's config.
///
/// The block cannot be refreshed before the editor opens, so the refresh has to
/// happen when the repaired block is read back.
#[test]
fn a_repaired_draft_block_follows_the_current_config() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();

    let stale = config_with_model("claude-opus-5.5");
    edit_query(
        &stale,
        dir.path(),
        &stream,
        "hello",
        &ScriptedEditor::saving(),
        None,
    )
    .unwrap();

    // A draft left with a half-typed edit: the closing quote is missing.
    let path = dir.path().join(QUERY_FILENAME);
    let draft = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        draft.replace(r#"reasoning = "auto""#, r#"reasoning = "off"#),
    )
    .unwrap();

    let current = config_with_model("claude-opus-5-5");
    let editor = ScriptedEditor::replacing(r#"reasoning = "off"#, r#"reasoning = "off""#);
    let (_, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    // The invalid block reaches the editor untouched: nothing in it can be read.
    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5.5"

            [assistant.model.parameters]
            reasoning = "off"#});

    let applied = applied(&current, output);
    assert_eq!(
        applied.assistant.model.id.resolved().to_string(),
        "anthropic/claude-opus-5-5"
    );
    assert_eq!(
        applied.assistant.model.parameters.reasoning,
        Some(ReasoningConfig::Off)
    );
}

/// A setting the user added to the block keeps its merge strategy across a
/// reopen.
///
/// Clearing the inherited instructions with `strategy = "replace"` and an empty
/// list must still clear them after the draft is reopened under another model,
/// rather than vanish or turn into an append.
#[test]
fn an_added_draft_setting_keeps_its_merge_strategy() {
    let dir = Utf8TempDir::new().unwrap();
    let stream = ConversationStream::new_test();

    let stale = config_with_model("claude-opus-5.5");
    let editor = ScriptedEditor::replacing(
        r#"reasoning = "auto""#,
        "reasoning = \"auto\"\n\n[assistant.instructions]\nvalue = []\nstrategy = \"replace\"",
    );
    edit_query(&stale, dir.path(), &stream, "hello", &editor, None).unwrap();

    let current = config_with_model_and("claude-opus-5-5", indoc::indoc! {r#"
        [[assistant.instructions]]
        title = "Inherited"
        items = ["Answer in French."]
    "#});
    assert_eq!(current.assistant.instructions.len(), 1);

    let editor = ScriptedEditor::saving();
    let (_, output) = edit_query(&current, dir.path(), &stream, "", &editor, None).unwrap();

    // Re-rendered through the config type, so the default flag is spelled out.
    assert_eq!(editor.shown_config_block(), indoc::indoc! {r#"
            [assistant.instructions]
            value = []
            strategy = "replace"
            discard_when_merged = false

            [assistant.model.id]
            provider = "anthropic"
            name = "claude-opus-5-5"

            [assistant.model.parameters]
            reasoning = "auto""#});
    assert_eq!(applied(&current, output).assistant.instructions, vec![]);
}

/// Refreshing an untouched block onto another run's seed yields that seed
/// exactly.
///
/// Fails when [`config_seed`] gains a pre-filled field that
/// [`refresh_untouched`] does not refresh: an untouched value in it would
/// otherwise survive a config change.
#[test]
fn refresh_untouched_covers_every_pre_filled_field() {
    let stale = config_seed(&config_with_model("claude-opus-5.5"));
    let current = config_seed(&config_with_model_and(
        "claude-opus-5-5",
        "assistant.model.parameters.reasoning = \"off\"",
    ));
    assert_ne!(
        stale.assistant.model.parameters.reasoning,
        current.assistant.model.parameters.reasoning
    );

    assert_eq!(
        refresh_untouched(stale.clone(), Some(&stale), &current),
        current
    );
}
