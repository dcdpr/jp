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

/// The values resolved under `key`, for readable assertions.
fn values<'a>(resolved: &'a Resolution, key: &str) -> Vec<&'a str> {
    resolved
        .get(key)
        .map(|values| values.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

/// Every key that resolved, in rule-declaration order.
fn keys(resolved: &Resolution) -> Vec<&str> {
    resolved.keys().map(String::as_str).collect()
}

/// What [`Resolver::automatic`] hands back.
type Resolution = IndexMap<String, Vec<String>>;

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

    assert_eq!(keys(&resolved), ["team", "stage"], "in rule order");
    assert_eq!(values(&resolved, "team"), ["platform"]);
    assert_eq!(values(&resolved, "stage"), ["review"]);
}

/// A list-valued rule contributes every value it names, in order.
#[tokio::test]
async fn list_rules_resolve_to_every_value() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "crate": ["jp_config", "jp_llm"],
            "stage": { "value": ["draft", "review"] }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(values(&resolved, "crate"), ["jp_config", "jp_llm"]);
    assert_eq!(values(&resolved, "stage"), ["draft", "review"]);
}

/// A rule that produced nothing keeps its key, so a caller can tell it apart
/// from a rule that never matched and replace the key's set with nothing.
#[tokio::test]
async fn an_empty_list_rule_keeps_its_key_with_no_values() {
    let (rules, tmp, printer, _err, prompts) = setup(r#"{ "crate": { "value": [] } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(keys(&resolved), ["crate"]);
    assert!(values(&resolved, "crate").is_empty());
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
    assert_eq!(keys(&new), ["onnew", "onboth"]);

    let fork = resolver.automatic(Trigger::Fork).await.unwrap();
    assert_eq!(keys(&fork), ["onfork", "onboth"]);
}

#[tokio::test]
async fn unattended_commands_run_without_prompting() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "greeting": { "value": { "cmd": "echo hello" }, "run": "unattended" }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(values(&resolved, "greeting"), ["hello"]);
}

/// Each line of stdout is one value, so a value may contain any character
/// without an escaping rule.
/// Empty lines are dropped, which is what keeps a trailing newline from adding
/// an empty value.
#[tokio::test]
async fn command_output_is_one_value_per_line() {
    let (rules, tmp, printer, _err, prompts) = setup(
        r#"{
            "crate": {
                "value": {
                    "cmd": {
                        "program": "printf 'jp_config\\nwith spaces, and a comma\\n\\n'",
                        "args": [],
                        "shell": true
                    }
                },
                "run": "unattended"
            }
        }"#,
    );
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(values(&resolved, "crate"), [
        "jp_config",
        "with spaces, and a comma"
    ]);
}

/// A command that writes nothing produces no values, rather than a bare label.
/// Its key survives, because the rule ran and succeeded.
#[tokio::test]
async fn a_silent_command_produces_no_values() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "quiet": { "value": { "cmd": "true" }, "run": "unattended" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert_eq!(keys(&resolved), ["quiet"]);
    assert!(values(&resolved, "quiet").is_empty());
}

/// A rule that failed is dropped entirely: its key is absent, so whatever the
/// conversation already carries is left alone.
#[tokio::test]
async fn a_failing_command_drops_its_key() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "broken": { "value": { "cmd": "false" }, "run": "unattended" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();

    assert!(resolved.is_empty());
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

    assert_eq!(values(&resolved, "piped"), ["a-b-c"]);
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
        Some(("manual".to_owned(), vec!["yes".to_owned()]))
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

    assert_eq!(keys(&resolved), ["ok"], "the working rule survives");
    assert_eq!(values(&resolved, "ok"), ["kept"]);

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

    assert_eq!(keys(&resolved), ["ok"], "the declined label is dropped");
    assert_eq!(values(&resolved, "ok"), ["kept"]);
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

    assert_eq!(values(&resolved, "asks"), ["hi"]);
}

/// A static rule never consults `run`, so it resolves with no terminal even
/// under the default `ask` policy.
#[tokio::test]
async fn static_rules_are_unaffected_by_run_policy() {
    let (rules, tmp, printer, _err, prompts) =
        setup(r#"{ "plain": { "value": "v", "run": "ask" } }"#);
    let resolver = Resolver::new(&rules, tmp.path(), false, &printer, &prompts);

    let resolved = resolver.automatic(Trigger::New).await.unwrap();
    assert_eq!(values(&resolved, "plain"), ["v"]);
}
