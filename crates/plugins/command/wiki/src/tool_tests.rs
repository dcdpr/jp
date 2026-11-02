use std::fs;

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use indoc::indoc;
use jp_plugin::message::{PathsInfo, WorkspaceInfo};
use jp_tool::FsRule;
use serde_json::json;

use super::*;

const GLOSSARY: &str = indoc! {"
    # Glossary

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Label**: domain=llm
    - **Summary**: The words the project uses.

    ## Turn

    One cycle.
"};

const CHILD: &str = indoc! {"
    # Child

    - **Status**: Reference
    - **Authors**: jp
    - **Date**: 2026-10-30
    - **Parent**: W-0000001
    - **Summary**: A child page.

    ## Context

    ![Turn](0000001-glossary.md#turn)
"};

/// A wiki with a glossary and a page under it that embeds one of its sections,
/// an RFD linking to the glossary, and a source file.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    for (path, text) in [
        ("docs/wiki/.epoch", "2026-08-10\n"),
        ("docs/wiki/0000001-glossary.md", GLOSSARY),
        ("docs/wiki/0000002-child.md", CHILD),
        (
            "docs/rfd/100-tickets.md",
            "# RFD 100: Tickets\n\n- **Status**: Implemented\n\nSee [the \
             glossary](../wiki/0000001-glossary.md).\n",
        ),
        ("crates/x/src/lib.rs", "fn one() {}\nfn two() {}\n"),
    ] {
        let path = root.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    root
}

/// The workspace a call runs in, as the host describes it in `init`.
fn init(root: &Utf8TempDir) -> InitMessage {
    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.path().to_path_buf(),
            storage: root.path().join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo::default(),
        config: json!({}),
        options: Map::new(),
        args: vec![],
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: None,
    }
}

/// The plugin's options: the assistant is `jp`, and pages label their domain.
fn options() -> Options {
    let Value::Object(map) = json!({
        "assistant": "jp",
        "labels": { "domain": { "description": "The subsystem", "values": ["llm", "storage"] } }
    }) else {
        unreachable!("a table");
    };

    Options::read(&map).unwrap()
}

fn call(name: &str, arguments: Value) -> ToolCall {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are a table");
    };

    ToolCall {
        action: ToolAction::Run,
        name: name.to_owned(),
        arguments,
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: "test".to_owned(),
    }
}

/// A call held to a policy granting what `rules` grant.
fn with_policy(name: &str, arguments: Value, rules: Vec<FsRule>) -> ToolCall {
    let mut call = call(name, arguments);
    call.access = Some(
        serde_json::to_value(AccessPolicy {
            fs: rules,
            ..AccessPolicy::default()
        })
        .unwrap(),
    );

    call
}

fn run_call(root: &Utf8TempDir, call: &ToolCall) -> Outcome {
    run(&init(root), call, &options())
}

fn run_tool(root: &Utf8TempDir, name: &str, arguments: Value) -> Outcome {
    run_call(root, &call(name, arguments))
}

fn content(outcome: Outcome) -> String {
    match outcome {
        Outcome::Success { content } => content,
        other => panic!("expected success, got: {other:?}"),
    }
}

fn error_message(outcome: Outcome) -> String {
    match outcome {
        Outcome::Error { message, .. } => message,
        other => panic!("expected an error, got: {other:?}"),
    }
}

fn read(root: &Utf8TempDir, path: &str) -> String {
    fs::read_to_string(root.path().join(path)).unwrap()
}

/// The one page file whose name ends with `slug.md`.
fn page_file(root: &Utf8TempDir, slug: &str) -> Utf8PathBuf {
    let found: Vec<Utf8PathBuf> = fs::read_dir(root.path().join("docs/wiki"))
        .unwrap()
        .map(|entry| Utf8PathBuf::try_from(entry.unwrap().path()).unwrap())
        .filter(|path| path.as_str().ends_with(&format!("-{slug}.md")))
        .collect();
    assert_eq!(found.len(), 1, "{found:?}");

    found[0].clone()
}

/// Read and write in the wiki directory, and nothing else.
fn the_wiki() -> Vec<FsRule> {
    vec![FsRule::new("docs/wiki").with_read(true).with_write(true)]
}

#[test]
fn list_prints_the_pages_in_reading_order() {
    let root = workspace();

    assert_eq!(
        content(run_tool(&root, "list", json!({}))),
        concat!(
            "W-0000001  Reference  Glossary [domain=llm]\n",
            "  W-0000002  Reference  Child\n",
        )
    );
}

#[test]
fn list_says_when_nothing_matches() {
    let root = workspace();

    assert_eq!(
        content(run_tool(&root, "list", json!({ "query": "nowhere" }))),
        "No pages match.\n"
    );
    assert_eq!(
        content(run_tool(&root, "list", json!({ "status": "archived" }))),
        "No pages match.\n"
    );
}

/// `under` lists what sits below a page, indented from it, and names a page
/// that isn't there rather than matching nothing.
#[test]
fn list_under_a_page_lists_what_sits_below_it() {
    let root = workspace();

    assert_eq!(
        content(run_tool(&root, "list", json!({ "under": "W-0000001" }))),
        "W-0000002  Reference  Child\n"
    );
    assert_eq!(
        content(run_tool(&root, "list", json!({ "under": "W-0000002" }))),
        "No pages match.\n"
    );
    assert_eq!(
        error_message(run_tool(&root, "list", json!({ "under": "W-0000009" }))),
        "No wiki page W-0000009."
    );
}

/// A page the policy keeps from the call is named rather than skipped silently.
#[test]
fn list_names_the_pages_the_policy_hides() {
    let root = workspace();
    let call = with_policy("list", json!({}), vec![
        FsRule::new("docs/wiki").with_read(true),
        FsRule::new("docs/wiki/0000002-child.md").with_read(false),
    ]);

    assert_eq!(
        content(run_call(&root, &call)),
        concat!(
            "W-0000001  Reference  Glossary [domain=llm]\n",
            "Warning: These pages can't be read, so they aren't listed: W-0000002.\n",
        )
    );
}

#[test]
fn show_prints_the_page_fenced_as_markdown() {
    let root = workspace();

    let out = content(run_tool(&root, "show", json!({ "id": "W-0000001" })));

    assert_eq!(out, indoc! {"
        ```markdown
        # W-0000001: Glossary

        - **Path**: docs/wiki/0000001-glossary.md
        - **Status**: Reference
        - **Authors**: jp
        - **Date**: 2026-10-30
        - **Label**: domain=llm
        - **Summary**: The words the project uses.

        ## Referenced by

        - W-0000002: Child
        - RFD 100: Tickets

        ## Contents

        - Turn `#turn`
        ```
    "});
}

/// Reading a page grants nothing about what it embeds: the embed the policy
/// hides is a marker, and the rest of the page is shown.
#[test]
fn show_marks_an_embed_the_policy_hides() {
    let root = workspace();
    let arguments = json!({ "id": "0000002", "body": true, "expand_embeds": true });
    let call = with_policy("show", arguments, vec![
        FsRule::new("docs/wiki").with_read(true),
        FsRule::new("docs/wiki/0000001-glossary.md").with_read(false),
    ]);

    let out = content(run_call(&root, &call));

    assert!(
        out.contains(
            "## Context\n\n<!-- embed W-0000001#turn: not shown (access denied: read on \
             docs/wiki/0000001-glossary.md) -->\n```\n"
        ),
        "{out}"
    );
    assert!(
        out.ends_with(
            "Warning: 1 document can't be read, so a reference it makes to W-0000002 isn't \
             shown.\nWarning: docs/rfd/ can't be listed, so references to and from the documents \
             there aren't shown.\n"
        ),
        "{out}"
    );
}

/// What `wiki_show` returns by default is what `wiki_edit` replaces, so text
/// read, changed, and written back keeps its embeds.
#[test]
fn show_returns_the_text_as_written_unless_asked_to_expand() {
    let root = workspace();
    let section = json!({ "id": "0000002", "section": "context" });
    let expanded = json!({ "id": "0000002", "section": "context", "expand_embeds": true });

    assert_eq!(
        content(run_tool(&root, "show", section)),
        "```markdown\n## Context\n\n![Turn](0000001-glossary.md#turn)\n```\n"
    );
    assert_eq!(
        content(run_tool(&root, "show", expanded)),
        "```markdown\n## Context\n\n<!-- embed W-0000001#turn -->\n### Turn\n\nOne cycle.\n<!-- \
         /embed -->\n```\n"
    );
}

#[test]
fn show_takes_the_body_or_a_section_not_both() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(
            &root,
            "show",
            json!({ "id": "W-0000001", "body": true, "section": "turn" })
        )),
        "Pass `body` or `section`, not both."
    );
    assert_eq!(
        content(run_tool(
            &root,
            "show",
            json!({ "id": "W-0000001", "section": "Turn" })
        )),
        "```markdown\n## Turn\n\nOne cycle.\n```\n"
    );
}

#[test]
fn show_names_what_is_wrong_with_an_id() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(&root, "show", json!({}))),
        "Missing argument `id`."
    );
    assert_eq!(
        error_message(run_tool(&root, "show", json!({ "id": "nope" }))),
        "`nope` is not a wiki page id (try W-02wt0kx)."
    );
    assert_eq!(
        error_message(run_tool(&root, "show", json!({ "id": "W-0000009" }))),
        "No wiki page W-0000009."
    );
}

#[test]
fn create_writes_a_page_attributed_to_the_assistant() {
    let root = workspace();

    let out = content(run_tool(
        &root,
        "create",
        json!({
            "title": "Runbook",
            "summary": "How to release.",
            "body": "## Steps\n\nTag it.",
            "parent": "W-0000001",
            "labels": ["domain=storage"],
        }),
    ));

    let file = page_file(&root, "runbook");
    let id = format!("W-{}", &file.file_name().unwrap()[..7]);
    assert_eq!(
        out,
        format!("Created docs/wiki/{} ({id})\n", file.file_name().unwrap())
    );
    let source = fs::read_to_string(&file).unwrap();
    assert_eq!(
        source,
        format!(
            "# Runbook\n\n- **Status**: Reference\n- **Authors**: jp\n- **Date**: {}\n- \
             **Parent**: W-0000001\n- **Label**: domain=storage\n- **Summary**: How to \
             release.\n\n## Steps\n\nTag it.\n",
            cmd::today()
        )
    );
}

#[test]
fn create_needs_a_one_line_summary() {
    let root = workspace();
    let create = |arguments: Value| error_message(run_tool(&root, "create", arguments));

    assert_eq!(
        create(json!({ "title": "Runbook", "body": "Tag it." })),
        "Missing argument `summary`."
    );
    assert_eq!(
        create(json!({
            "title": "Runbook",
            "summary": "One.\nTwo.",
            "body": "Tag it.",
        })),
        "A summary is one sentence on one line."
    );
}

/// A hub is a title and a summary; the site lists the pages under it.
#[test]
fn create_without_a_body_writes_a_hub() {
    let root = workspace();

    content(run_tool(
        &root,
        "create",
        json!({ "title": "Guides", "summary": "How to do things." }),
    ));

    assert_eq!(
        fs::read_to_string(page_file(&root, "guides")).unwrap(),
        format!(
            "# Guides\n\n- **Status**: Reference\n- **Authors**: jp\n- **Date**: {}\n- \
             **Summary**: How to do things.\n",
            cmd::today()
        )
    );
}

/// `before` and `after` place a page among its siblings, under the parent the
/// same edit moves it to; one that isn't a sibling leaves the page unchanged.
#[test]
fn edit_places_a_page_among_its_siblings() {
    let root = workspace();
    content(run_tool(
        &root,
        "create",
        json!({ "title": "Guides", "summary": "How to do things." }),
    ));
    let guides = format!(
        "W-{}",
        &page_file(&root, "guides").file_name().unwrap()[..7]
    );
    let order = || read(&root, "docs/wiki/.order.json");

    assert_eq!(
        content(run_tool(
            &root,
            "edit",
            json!({ "id": guides, "before": "W-0000001" })
        )),
        format!("{guides} now sits above W-0000001.\n")
    );
    assert_eq!(
        order(),
        format!("{{\n  \"top\": [\n    \"{guides}\"\n  ],\n  \"children\": {{}}\n}}\n")
    );

    let child = read(&root, "docs/wiki/0000002-child.md");
    assert_eq!(
        error_message(run_tool(
            &root,
            "edit",
            json!({ "id": "W-0000002", "summary": "Moved.", "after": guides })
        )),
        format!(
            "W-0000002 and {guides} aren't siblings; a page is ranked among the pages sharing its \
             parent, or among the top pages when it has none."
        )
    );
    assert_eq!(read(&root, "docs/wiki/0000002-child.md"), child);

    // Moved to the top, it is a sibling of the top pages.
    let out = content(run_tool(
        &root,
        "edit",
        json!({ "id": "W-0000002", "parent": "none", "after": guides }),
    ));
    assert!(
        out.ends_with(&format!("W-0000002 now sits below {guides}.\n")),
        "{out}"
    );
    assert_eq!(
        order(),
        format!(
            "{{\n  \"top\": [\n    \"{guides}\",\n    \"W-0000002\"\n  ],\n  \"children\": \
             {{}}\n}}\n"
        )
    );
}

/// A Snapshot's links into source are pinned as it is created, which means
/// reading the file each names; one the policy hides is left as it is, and
/// named.
#[test]
fn create_leaves_unpinned_a_source_file_the_policy_hides() {
    let root = workspace();
    let call = with_policy(
        "create",
        json!({
            "title": "Notes",
            "status": "snapshot",
            "summary": "What a study found.",
            "body": "See [the code](../../crates/x/src/lib.rs#L1).",
        }),
        the_wiki(),
    );

    let out = content(run_call(&root, &call));

    let file = page_file(&root, "notes");
    assert!(
        out.ends_with(
            "Warning: ../../crates/x/src/lib.rs#L1 was left unpinned: access denied: read on \
             crates/x/src/lib.rs.\n"
        ),
        "{out}"
    );
    let source = fs::read_to_string(&file).unwrap();
    assert!(source.contains("- **Status**: Snapshot\n"), "{source}");
    assert!(
        source.ends_with("See [the code](../../crates/x/src/lib.rs#L1).\n"),
        "{source}"
    );
}

/// A page the policy hides might carry a label the vocabulary lets one page
/// carry, so a write naming such a key is refused while one is hidden.
#[test]
fn a_unique_label_is_refused_while_a_page_is_hidden() {
    let root = workspace();
    let Value::Object(map) = json!({ "labels": { "topic": {
        "description": "One page per topic",
        "values": [],
        "unique": "key-value",
    } } }) else {
        unreachable!("a table");
    };
    let options = Options::read(&map).unwrap();
    let call = with_policy(
        "create",
        json!({
            "title": "Runbook",
            "summary": "How.",
            "body": "Tag it.",
            "labels": ["topic=release"],
        }),
        vec![
            FsRule::new("docs/wiki").with_read(true).with_write(true),
            FsRule::new("docs/wiki/0000002-child.md").with_read(false),
        ],
    );

    assert_eq!(
        error_message(run(&init(&root), &call, &options)),
        "The vocabulary lets one page carry `topic`, and these pages can't be read to see that \
         none does: W-0000002."
    );
}

#[test]
fn create_has_to_be_allowed_to_create_in_the_wiki() {
    let root = workspace();
    let call = with_policy(
        "create",
        json!({ "title": "Runbook", "summary": "How.", "body": "Tag it." }),
        vec![FsRule::new("docs/wiki").with_read(true)],
    );

    assert_eq!(
        error_message(run_call(&root, &call)),
        "A page can't be created in docs/wiki: access denied: create on docs/wiki."
    );
    assert_eq!(
        fs::read_dir(root.path().join("docs/wiki")).unwrap().count(),
        3
    );
}

#[test]
fn edit_changes_only_what_it_is_given() {
    let root = workspace();

    let out = content(run_tool(
        &root,
        "edit",
        json!({ "id": "W-0000001", "summary": "Every word." }),
    ));

    assert_eq!(out, "Edited docs/wiki/0000001-glossary.md\n");
    assert_eq!(
        read(&root, "docs/wiki/0000001-glossary.md"),
        GLOSSARY.replace("The words the project uses.", "Every word.")
    );
}

#[test]
fn edit_names_what_to_change_when_given_nothing() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(&root, "edit", json!({ "id": "W-0000001" }))),
        "Name what to change: `title`, `summary`, `status`, `parent`, `before`, `after`, \
         `superseded_by`, `body`, `add_labels`, or `remove_labels`."
    );
    assert_eq!(
        error_message(run_tool(
            &root,
            "edit",
            json!({ "id": "W-0000001", "section": "turn" })
        )),
        "`section` names the section `body` replaces; pass the body too."
    );
}

/// A refusal leaves the wiki as it was.
#[test]
fn edit_refuses_what_the_policy_doesnt_let_it_write() {
    let root = workspace();
    let call = with_policy(
        "edit",
        json!({ "id": "W-0000001", "summary": "Every word." }),
        vec![FsRule::new("docs/wiki").with_read(true)],
    );

    assert_eq!(
        error_message(run_call(&root, &call)),
        "W-0000001 is unchanged: access denied: update on docs/wiki/0000001-glossary.md."
    );
    assert_eq!(read(&root, "docs/wiki/0000001-glossary.md"), GLOSSARY);
}

/// A rename rewrites the links the policy lets it rewrite, names the files
/// still linking to the old name, and says it couldn't search the rest of the
/// workspace.
#[test]
fn a_rename_names_the_links_it_could_not_rewrite() {
    let root = workspace();
    let mut rules = the_wiki();
    rules.push(FsRule::new("docs/rfd").with_read(true));
    let call = with_policy(
        "edit",
        json!({ "id": "W-0000001", "title": "Terms" }),
        rules,
    );

    let out = content(run_call(&root, &call));

    assert_eq!(out, indoc! {"
        Edited docs/wiki/0000001-terms.md
          updated links in docs/wiki/0000002-child.md
        Warning: These files still link to 0000001-glossary, and weren't rewritten:
          docs/rfd/100-tickets.md (access denied: update on docs/rfd/100-tickets.md)
        Warning: References outside docs/ were not checked: the access policy doesn't allow reading the workspace.
    "});
    assert!(read(&root, "docs/wiki/0000002-child.md").contains("![Turn](0000001-terms.md#turn)"));
    assert!(read(&root, "docs/rfd/100-tickets.md").contains("../wiki/0000001-glossary.md"));
}

#[test]
fn an_unknown_tool_names_the_ones_served() {
    let root = workspace();

    assert_eq!(
        error_message(run_tool(&root, "frobnicate", json!({}))),
        "`jp wiki` serves no tool named `frobnicate`. It serves: create, edit, list, show."
    );
}

#[test]
fn every_served_tool_is_dispatched() {
    let root = workspace();

    for name in TOOLS {
        if let Outcome::Error { message, .. } = run_tool(&root, name, json!({ "id": "W-0000001" }))
        {
            assert!(
                !message.contains("serves no tool named"),
                "{name} is listed but not dispatched"
            );
        }
    }
}

#[test]
fn a_tool_without_a_preview_says_so() {
    let root = workspace();
    let mut call = call("list", json!({}));
    call.action = ToolAction::FormatArguments;

    assert_eq!(
        error_message(run_call(&root, &call)),
        "`list` has no preview of its own; style its parameters as `function_call` or `json` \
         rather than `tool`."
    );
}

#[test]
fn an_unreadable_access_policy_refuses_the_call() {
    let root = workspace();
    let mut call = call("list", json!({}));
    call.access = Some(json!({ "fs": "everything" }));

    let message = error_message(run_call(&root, &call));

    assert!(
        message.starts_with("The access policy for this call can't be read: "),
        "{message}"
    );
}

/// A tool reaches only the workspace, wherever the wiki is configured.
#[test]
fn a_wiki_outside_the_workspace_is_refused() {
    let root = workspace();
    let elsewhere = Utf8TempDir::new().unwrap();
    let options = Options {
        dir: elsewhere.path().to_path_buf(),
        ..options()
    };

    assert_eq!(
        error_message(run(&init(&root), &call("list", json!({})), &options)),
        format!(
            "The wiki directory {} is outside the workspace, which is all a tool reaches.",
            elsewhere.path()
        )
    );
}

/// The wiki directory has to be readable for any page to be found.
#[test]
fn a_wiki_the_policy_hides_is_refused() {
    let root = workspace();
    let call = with_policy("list", json!({}), vec![
        FsRule::new("docs/rfd").with_read(true),
    ]);

    assert_eq!(
        error_message(run_call(&root, &call)),
        "The wiki directory can't be read: access denied: read on docs/wiki."
    );
}
