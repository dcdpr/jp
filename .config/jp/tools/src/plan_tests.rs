use camino_tempfile::Utf8TempDir;
use jp_tool::Outcome;

use super::*;

fn tasks(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn content(result: ToolResult) -> String {
    match result.expect("tool result") {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

fn error_message(result: ToolResult) -> String {
    match result.expect("tool result") {
        Outcome::Error { message, .. } => message,
        other => panic!("expected error, got: {other:?}"),
    }
}

#[test]
fn create_marks_first_task_in_progress_and_writes_file() {
    let dir = Utf8TempDir::new().unwrap();

    let out = content(create(
        dir.path(),
        "my-plan",
        tasks(&["one", "two", "three"]),
    ));

    assert!(out.contains("(0/3 complete)"), "{out}");
    assert!(out.contains("- [>] one"), "{out}");
    assert!(out.contains("- [ ] two"), "{out}");
    assert!(out.contains("- [ ] three"), "{out}");
    assert!(path_for(dir.path(), "my-plan").exists());
}

#[test]
fn create_rejects_empty_task_list() {
    let dir = Utf8TempDir::new().unwrap();
    let message = error_message(create(dir.path(), "my-plan", tasks(&[])));
    assert!(message.contains("at least one task"), "{message}");
}

#[test]
fn create_rejects_blank_task() {
    let dir = Utf8TempDir::new().unwrap();
    let message = error_message(create(dir.path(), "my-plan", tasks(&["ok", "   "])));
    assert!(message.contains("must not be empty"), "{message}");
}

#[test]
fn create_resets_an_existing_plan() {
    let dir = Utf8TempDir::new().unwrap();

    create(dir.path(), "my-plan", tasks(&["a", "b"])).unwrap();
    advance(dir.path(), "my-plan").unwrap(); // a done, b in progress

    // Re-creating with new tasks starts fresh.
    let out = content(create(dir.path(), "my-plan", tasks(&["x", "y", "z"])));
    assert!(out.contains("(0/3 complete)"), "{out}");
    assert!(out.contains("- [>] x"), "{out}");
}

#[test]
fn advance_completes_current_and_starts_next() {
    let dir = Utf8TempDir::new().unwrap();
    create(dir.path(), "my-plan", tasks(&["one", "two", "three"])).unwrap();

    let out = content(advance(dir.path(), "my-plan"));

    assert!(out.contains("(1/3 complete)"), "{out}");
    assert!(out.contains("- [x] one"), "{out}");
    assert!(out.contains("- [>] two"), "{out}");
    assert!(out.contains("- [ ] three"), "{out}");
}

#[test]
fn advance_through_completion() {
    let dir = Utf8TempDir::new().unwrap();
    create(dir.path(), "my-plan", tasks(&["one", "two"])).unwrap();

    advance(dir.path(), "my-plan").unwrap();
    let out = content(advance(dir.path(), "my-plan"));

    assert!(out.contains("(2/2 complete)"), "{out}");
    assert!(out.contains("- [x] one"), "{out}");
    assert!(out.contains("- [x] two"), "{out}");
}

#[test]
fn advance_past_completion_is_idempotent() {
    let dir = Utf8TempDir::new().unwrap();
    create(dir.path(), "my-plan", tasks(&["only"])).unwrap();
    advance(dir.path(), "my-plan").unwrap(); // complete

    let out = content(advance(dir.path(), "my-plan"));
    assert!(out.contains("already complete"), "{out}");
    assert!(out.contains("(1/1 complete)"), "{out}");
}

#[test]
fn retreat_reopens_previous_task() {
    let dir = Utf8TempDir::new().unwrap();
    create(dir.path(), "my-plan", tasks(&["one", "two", "three"])).unwrap();
    advance(dir.path(), "my-plan").unwrap(); // one done, two in progress

    let out = content(retreat(dir.path(), "my-plan"));

    assert!(out.contains("(0/3 complete)"), "{out}");
    assert!(out.contains("- [>] one"), "{out}");
    assert!(out.contains("- [ ] two"), "{out}");
}

#[test]
fn retreat_past_first_task_discards_the_plan() {
    let dir = Utf8TempDir::new().unwrap();
    create(dir.path(), "my-plan", tasks(&["one", "two"])).unwrap();

    let out = content(retreat(dir.path(), "my-plan"));

    assert!(out.contains("discarded"), "{out}");
    assert!(!path_for(dir.path(), "my-plan").exists());
}

#[test]
fn advance_on_missing_plan_errors() {
    let dir = Utf8TempDir::new().unwrap();
    let message = error_message(advance(dir.path(), "ghost"));
    assert!(message.contains("No plan named \"ghost\""), "{message}");
}

#[test]
fn retreat_on_missing_plan_errors() {
    let dir = Utf8TempDir::new().unwrap();
    let message = error_message(retreat(dir.path(), "ghost"));
    assert!(message.contains("No plan named \"ghost\""), "{message}");
}

#[test]
fn corrupt_plan_file_is_removed_and_reported_as_missing() {
    let dir = Utf8TempDir::new().unwrap();
    std::fs::write(path_for(dir.path(), "broken"), "{ not valid json").unwrap();

    let message = error_message(advance(dir.path(), "broken"));

    assert!(message.contains("No plan named \"broken\""), "{message}");
    assert!(
        !path_for(dir.path(), "broken").exists(),
        "corrupt plan file should be removed so it can be recreated"
    );
}

#[test]
fn plans_dir_is_scoped_to_workspace_and_conversation() {
    let dir = plans_dir(Utf8Path::new("/ws"), "conv-1");
    assert_eq!(dir, Utf8Path::new("/ws/.jp/mcp/state/plans/conv-1"));
}

#[test]
fn validate_name_accepts_simple_names() {
    assert!(validate_name("refactor-config").is_ok());
    assert!(validate_name("Phase 1").is_ok());
    assert!(validate_name("step_2").is_ok());
}

#[test]
fn validate_name_rejects_path_traversal_and_separators() {
    assert!(validate_name("../escape").is_err());
    assert!(validate_name("a/b").is_err());
    assert!(validate_name("with.dot").is_err());
    assert!(validate_name("").is_err());
    assert!(validate_name("   ").is_err());
}
