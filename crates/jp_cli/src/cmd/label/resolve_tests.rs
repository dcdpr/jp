use camino_tempfile::{Utf8TempDir, tempdir};
use indexmap::IndexMap;
use jp_config::conversation::label::LabelConfig;
use jp_inquire::prompt::MockPromptBackend;
use jp_printer::{OutputFormat, Printer, SharedBuffer};

use super::*;

/// A resolver over `rules`, with no terminal available.
///
/// Returns the temp dir so it outlives the resolver's `root` borrow.
/// The prompt backend is scripted with no responses, which is correct for these
/// tests: with `is_tty` false the `Ask` branch errors before ever prompting.
fn setup(
    rules: &str,
) -> (
    IndexMap<String, LabelConfig>,
    Utf8TempDir,
    Printer,
    SharedBuffer,
    MockPromptBackend,
) {
    let rules: IndexMap<String, LabelConfig> = serde_json::from_str(rules).unwrap();
    let tmp = tempdir().unwrap();
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);
    (rules, tmp, printer, err, MockPromptBackend::new())
}

#[tokio::test]
async fn static_rules_resolve_without_running_anything() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "team": "platform",
            "stage": { "value": "review", "apply_on": { "new": true } },
            "later": { "value": "x", "apply_on": { "new": false, "fork": true } }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved["team"], "platform");
    assert_eq!(resolved["stage"], "review");
}

/// `apply_on` selects which rules a trigger applies.
/// A rule that opts out of `new` still applies on `fork` when it opts into it.
#[tokio::test]
async fn apply_on_selects_rules_per_trigger() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "onnew": { "value": "a" },
            "onfork": { "value": "b", "apply_on": { "new": false, "fork": true } },
            "onboth": { "value": "c", "apply_on": { "new": true, "fork": true } }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let new = resolver.automatic(Trigger::New).await.unwrap();
    assert_eq!(new.keys().collect::<Vec<_>>(), ["onboth", "onnew"]);

    let fork = resolver.automatic(Trigger::Fork).await.unwrap();
    assert_eq!(fork.keys().collect::<Vec<_>>(), ["onboth", "onfork"]);
}

#[tokio::test]
async fn unattended_commands_run_and_their_stdout_is_trimmed() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "greeting": { "value": { "cmd": "echo  hello  " }, "run": "unattended" }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved["greeting"], "hello");
}

#[tokio::test]
async fn shell_commands_get_a_shell() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "piped": {
                "value": {
                    "cmd": { "program": "echo a b c | tr ' ' -", "args": [], "shell": true }
                },
                "run": "unattended"
            }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved["piped"], "a-b-c");
}

/// A rule the user asked for by name resolves regardless of `apply_on`.
#[tokio::test]
async fn alias_ignores_apply_on() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "manual": { "value": "yes", "apply_on": { "new": false, "fork": false } } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    assert!(resolver.automatic(Trigger::New).await.unwrap().is_empty());
    assert_eq!(
        resolver.alias("manual").await.unwrap(),
        Some(("manual".to_owned(), "yes".to_owned()))
    );
}

// ── Failure semantics: automatic application degrades, aliases error ──────────

#[tokio::test]
async fn automatic_application_drops_a_failing_command() {
    let (rules, tmp, printer, err, prompts) = setup(
        r#"{
            "ok": "kept",
            "broken": { "value": { "cmd": "false" }, "run": "unattended" }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved.len(), 1, "the working rule survives");
    assert_eq!(resolved["ok"], "kept");

    printer.flush();
    assert!(
        err.lock().contains("Skipping label 'broken'"),
        "the drop is reported, got: {}",
        err.lock()
    );
}

#[tokio::test]
async fn alias_errors_on_a_failing_command() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "broken": { "value": { "cmd": "false" }, "run": "unattended" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let error = resolver.alias("broken").await.unwrap_err().to_string();
    assert!(error.contains("broken"), "got: {error}");
}

#[tokio::test]
async fn alias_errors_on_an_unknown_name() {
    let (rules, tmp, printer, _err, prompts) = setup(r#"{ "known": "x" }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let error = resolver.alias("missing").await.unwrap_err().to_string();
    assert!(error.contains("unknown label alias"), "got: {error}");
}

/// `deny` is a silent skip under automatic application and an error when the
/// user names the rule, since they'd otherwise be left wondering.
#[tokio::test]
async fn deny_is_skipped_automatically_but_errors_as_an_alias() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "denied": { "value": { "cmd": "echo x" }, "run": "deny" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    assert!(resolver.automatic(Trigger::New).await.unwrap().is_empty());

    let error = resolver.alias("denied").await.unwrap_err().to_string();
    assert!(error.contains("deny"), "got: {error}");
}

/// Without a terminal there is no safe assumption: running an unconfirmed
/// command is a security decision, and skipping silently loses a label the
/// config asked for.
/// Both paths abort with a message naming the fix.
#[tokio::test]
async fn ask_without_a_terminal_aborts() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "asks": { "value": { "cmd": "echo x" } } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let error = resolver.automatic(Trigger::New).await.unwrap_err();
    let error = error.to_string();
    assert!(error.contains("no terminal"), "got: {error}");

    let error = resolver.alias("asks").await.unwrap_err().to_string();
    assert!(error.contains("no terminal"), "got: {error}");
}

/// An explicit `n` at the prompt drops that one label and lets the surrounding
/// command carry on.
#[tokio::test]
async fn declining_the_prompt_drops_only_that_label() {
    let (rules, tmp, printer, _err, _) = setup(
        r#"{
            "ok": "kept",
            "asks": { "value": { "cmd": "echo x" } }
        }"#,
    );
    let prompts = MockPromptBackend::new().with_inline_responses(['n']);
    let resolver = Resolver::new(&rules, tmp.path(), true, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved.len(), 1, "the declined label is dropped");
    assert_eq!(resolved["ok"], "kept");
}

/// Cancelling the prompt (Ctrl-C, Esc) is not an answer: it aborts the whole
/// command, so a conversation is never created and no turn is sent.
///
/// The mock returns `OperationCanceled` once its scripted responses run out, so
/// an empty script is exactly a cancelled prompt.
#[tokio::test]
async fn cancelling_the_prompt_aborts_the_command() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "asks": { "value": { "cmd": "echo x" } } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), true, &printer, &prompts);

    assert!(
        resolver.automatic(Trigger::New).await.is_err(),
        "a cancelled prompt must not resolve to a silent decline"
    );
}

/// Approving runs the command and keeps its output.
#[tokio::test]
async fn approving_the_prompt_runs_the_command() {
    let (rules, tmp, printer, _err, _) = setup(r#"{ "asks": { "value": { "cmd": "echo hi" } } }"#);
    let prompts = MockPromptBackend::new().with_inline_responses(['y']);
    let resolver = Resolver::new(&rules, tmp.path(), true, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(resolved["asks"], "hi");
}

/// A static rule never consults `run`, so it resolves with no terminal even
/// under the default `ask` policy.
#[tokio::test]
async fn static_rules_are_unaffected_by_run_policy() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "plain": { "value": "v", "run": "ask" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();
    assert_eq!(resolved["plain"], "v");
}
