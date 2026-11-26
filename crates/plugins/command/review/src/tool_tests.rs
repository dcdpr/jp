use std::fs::{self, File};

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{OutputFormat, PathsInfo, WorkspaceInfo};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::{
    model::{Comment, Reply, Status},
    pass::NewPass,
    snapshot, store,
};

/// What the file under review holds.
const SOURCE: &str = "# Title\n\nThe server binds to every interface.\nBy default.\n";

/// The conversation the review's first pass runs in.
const CONVERSATION: &str = "jp-c17629384710";

/// A workspace holding `docs/a.md`, and a review of it whose first pass is
/// running in [`CONVERSATION`], sending the reviewer's comment `R-...#1` on
/// line 3.
///
/// The pass runs as long as the returned [`File`], which holds its `.pass` lock
/// the way the process running a pass does, is kept.
fn reviewed() -> (Utf8TempDir, ReviewId, File) {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();
    fs::write(root.path().join("docs/a.md"), SOURCE).unwrap();

    let store = Store::new(root.path().join(".user/review"));
    let at = now();
    let mut review = Review::new(
        "Review".to_owned(),
        "Jane Doe <jane@example.com>".to_owned(),
        at,
        vec!["docs/a.md".to_owned()],
    );
    let contents = snapshot::working_tree(root.path(), &review.paths).unwrap();
    review.record(store::manifest(&contents), None, at);
    let id = store.create(&review, &[SOURCE]).unwrap();

    store
        .update(id, |review, _| {
            review
                .add_comment(NewComment {
                    role: Role::Reviewer,
                    author: "Jane Doe <jane@example.com>".to_owned(),
                    created_at: at,
                    content: Content {
                        anchor: Anchor {
                            version: 1,
                            target: Target::Range("docs/a.md".to_owned(), "3".parse().unwrap()),
                        },
                        body: "Loopback only.".to_owned(),
                        suggestion: None,
                    },
                })
                .map_err(|refusal| refusal.message(id))?;
            review
                .submit(NewPass {
                    author: "Jane Doe <jane@example.com>".to_owned(),
                    created_at: at,
                    version: 1,
                    only: vec![],
                    message: None,
                    conversation: None,
                })
                .map_err(|refusal| refusal.message(id))?;
            review.passes.created(1, CONVERSATION);

            Ok::<_, String>(())
        })
        .unwrap();
    let pass = store.lock_pass(id).unwrap().unwrap();

    (root, id, pass)
}

fn init(root: &Utf8Path, options: Value) -> InitMessage {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    InitMessage {
        version: jp_plugin::PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: root.to_path_buf(),
            storage: root.join(".jp"),
            id: "test".to_owned(),
        },
        paths: PathsInfo {
            user_workspace: Some(root.join(".user")),
            ..PathsInfo::default()
        },
        config: json!({}),
        options,
        args: vec![],
        log_level: 0,
        output_format: OutputFormat::Text,
        tool: None,
    }
}

fn call(name: &str, arguments: Value, conversation: &str) -> ToolCall {
    let Value::Object(arguments) = arguments else {
        panic!("arguments are an object");
    };

    ToolCall {
        action: ToolAction::Run,
        name: name.to_owned(),
        arguments,
        answers: Map::new(),
        options: Map::new(),
        access: None,
        conversation: conversation.to_owned(),
    }
}

/// Run a call from the review's conversation, with `options`.
fn run_with(root: &Utf8Path, options: Value, call: &ToolCall) -> Outcome {
    let init = init(root, options);
    let options = Options::read(&init.options).unwrap();

    run(&init, call, &options)
}

fn run_call(root: &Utf8Path, name: &str, arguments: Value) -> Outcome {
    run_with(root, json!({}), &call(name, arguments, CONVERSATION))
}

fn success(content: &str) -> Outcome {
    Outcome::Success {
        content: content.to_owned(),
    }
}

fn error(message: &str) -> Outcome {
    Outcome::Error {
        message: message.to_owned(),
        trace: vec![],
        transient: true,
    }
}

fn stored(root: &Utf8Path, id: ReviewId) -> Review {
    Store::new(root.join(".user/review")).peek(id).unwrap()
}

fn comment_of(review: &Review, number: u32) -> &Comment {
    review.comment(number).unwrap()
}

#[test]
fn a_reply_is_stored_with_its_verdict_and_its_pass() {
    let (root, id, _pass) = reviewed();

    let outcome = run_call(
        root.path(),
        "reply",
        json!({ "comment": format!("{id}#1"), "verdict": "accept", "body": " Fixed. " }),
    );

    assert_eq!(outcome, success(&format!("Replied to {id}#1, accept.")));
    let review = stored(root.path(), id);
    let reply = &comment_of(&review, 1).replies[0];
    assert_eq!(
        (
            reply.role,
            reply.author.as_str(),
            reply.body.as_str(),
            reply.verdict,
            reply.pass
        ),
        (
            Role::Assistant,
            "assistant",
            "Fixed.",
            Some(Verdict::Accept),
            Some(1)
        )
    );
}

#[test]
fn a_reply_to_the_reviewers_comment_needs_a_verdict() {
    let (root, id, _pass) = reviewed();

    let outcome = run_call(
        root.path(),
        "reply",
        json!({ "comment": format!("{id}#1"), "body": "Fixed." }),
    );

    assert_eq!(
        outcome,
        error(&format!(
            "A reply to {id}#1, the reviewer's comment, needs a verdict: accept, amend, decline, \
             defer, or ask."
        ))
    );
}

#[test]
fn a_reply_is_refused_on_a_draft_or_a_missing_comment() {
    let (root, id, _pass) = reviewed();
    Store::new(root.path().join(".user/review"))
        .update(id, |review, _| {
            review
                .add_comment(NewComment {
                    role: Role::Reviewer,
                    author: "Jane".to_owned(),
                    created_at: now(),
                    content: Content {
                        anchor: Anchor {
                            version: 1,
                            target: Target::Review,
                        },
                        body: "Later.".to_owned(),
                        suggestion: None,
                    },
                })
                .map_err(|refusal| refusal.message(id))
        })
        .unwrap();
    let reply = |number: u32| {
        run_call(
            root.path(),
            "reply",
            json!({ "comment": format!("{id}#{number}"), "verdict": "ask", "body": "Why?" }),
        )
    };

    assert_eq!(
        reply(2),
        error(&format!("{id}#2 is a draft, not yet sent."))
    );
    assert_eq!(reply(9), error(&format!("No comment {id}#9.")));
}

#[test]
fn a_body_longer_than_max_chars_is_refused() {
    let (root, id, _pass) = reviewed();
    let reply = call(
        "reply",
        json!({ "comment": format!("{id}#1"), "verdict": "decline", "body": "Twelve chars" }),
        CONVERSATION,
    );

    let outcome = run_with(root.path(), json!({ "max_chars": 11 }), &reply);

    assert_eq!(
        outcome,
        error(
            "`body` holds 12 characters, and a reply or comment holds at most 11. Say it in fewer \
             words."
        )
    );
    assert_eq!(comment_of(&stored(root.path(), id), 1).replies, []
        as [Reply; 0]);
}

#[test]
fn a_call_from_another_conversation_is_refused() {
    let (root, id, _pass) = reviewed();
    let arguments = json!({ "comment": format!("{id}#1"), "verdict": "accept", "body": "Done." });

    let elsewhere = run_with(
        root.path(),
        json!({}),
        &call("reply", arguments.clone(), "jp-c1"),
    );
    let nowhere = run_with(root.path(), json!({}), &call("reply", arguments, ""));

    assert_eq!(
        elsewhere,
        error(&format!(
            "{id} runs its passes in {CONVERSATION}, and this call comes from jp-c1."
        ))
    );
    assert_eq!(
        nowhere,
        error(&format!(
            "{id} runs its passes in {CONVERSATION}, and this call comes from no conversation."
        ))
    );
}

#[test]
fn a_comment_is_narrowed_to_its_quote() {
    let (root, id, _pass) = reviewed();

    let outcome = run_call(
        root.path(),
        "comment",
        json!({
            "review": id.to_string(),
            "path": "docs/a.md",
            "lines": "3-4",
            "quote": "every interface",
            "body": "Loopback?"
        }),
    );

    assert_eq!(
        outcome,
        success(&format!("Opened {id}#2 on docs/a.md:3:21-3:36."))
    );
    let review = stored(root.path(), id);
    let opened = comment_of(&review, 2);
    assert_eq!(
        (opened.role, opened.pass, opened.content.anchor.version),
        (Role::Assistant, Some(1), 1)
    );
    assert_eq!(
        opened.status(),
        Status::Open,
        "an assistant's comment awaits the reviewer"
    );
}

/// The assistant may have edited the file before commenting, and its lines
/// count in the file as it is now.
#[test]
fn a_comment_records_the_working_tree_it_counts_lines_in() {
    let (root, id, _pass) = reviewed();
    fs::write(
        root.path().join("docs/a.md"),
        "# Title\n\nIntro.\n\nThe server binds to loopback.\n",
    )
    .unwrap();

    let outcome = run_call(
        root.path(),
        "comment",
        json!({ "review": id.to_string(), "path": "./docs/a.md", "lines": "5", "body": "Say why." }),
    );

    assert_eq!(outcome, success(&format!("Opened {id}#2 on docs/a.md:5.")));
    let review = stored(root.path(), id);
    assert_eq!(review.latest().map(|latest| latest.number), Some(2));
    assert_eq!(comment_of(&review, 2).content.anchor.version, 2);
}

#[test]
fn a_comment_on_the_review_or_a_file_needs_no_lines() {
    let (root, id, _pass) = reviewed();

    let on_review = run_call(
        root.path(),
        "comment",
        json!({ "review": id.to_string(), "body": "Two ids." }),
    );
    let on_file = run_call(
        root.path(),
        "comment",
        json!({ "review": id.to_string(), "path": "docs/a.md", "body": "Long." }),
    );

    assert_eq!(on_review, success(&format!("Opened {id}#2 on the review.")));
    assert_eq!(on_file, success(&format!("Opened {id}#3 on docs/a.md.")));
}

#[test]
fn a_place_the_review_does_not_hold_is_refused() {
    let (root, id, _pass) = reviewed();
    let comment = |arguments: Value| run_call(root.path(), "comment", arguments);
    let review = id.to_string();

    assert_eq!(
        comment(json!({ "review": review, "lines": "3", "body": "x" })),
        error("`lines` needs `path`, the file they are in.")
    );
    assert_eq!(
        comment(json!({ "review": review, "path": "docs/a.md", "quote": "x", "body": "x" })),
        error("`quote` narrows `lines`; give `lines` too.")
    );
    assert_eq!(
        comment(json!({ "review": review, "path": "docs/b.md", "body": "x" })),
        error("`docs/b.md` is not a file in the review, which tracks docs/a.md.")
    );
    assert_eq!(
        comment(json!({ "review": review, "path": "docs/a.md", "lines": "9", "body": "x" })),
        error("docs/a.md:9 lies outside docs/a.md as version 1 holds it.")
    );
    assert_eq!(
        comment(json!({ "review": review, "path": "docs/a.md", "lines": "3:1-3:4", "body": "x" })),
        error(
            "`lines` takes a line, `42`, or lines, `42-45`, not `3:1-3:4`; narrow them to the \
             text you mean with `quote`."
        )
    );
    assert_eq!(
        comment(
            json!({ "review": review, "path": "docs/a.md", "lines": "3", "quote": "in", "body": "x" })
        ),
        error("`quote` occurs 2 times in lines 3; quote more of the line to name one of them.")
    );
}

#[test]
fn arguments_that_do_not_fit_the_tool_are_refused() {
    let (root, id, _pass) = reviewed();

    let outcome = run_call(root.path(), "comment", json!({ "review": id.to_string() }));

    assert_eq!(
        outcome,
        error("The arguments don't fit the tool: missing field `body`.")
    );
}

#[test]
fn an_unknown_tool_and_a_preview_are_refused() {
    let (root, _, _pass) = reviewed();
    let mut preview = call("reply", json!({}), CONVERSATION);
    preview.action = ToolAction::FormatArguments;

    assert_eq!(
        run_call(root.path(), "resolve", json!({})),
        error("`jp review` serves no tool named `resolve`. It serves: comment, reply.")
    );
    assert_eq!(
        run_with(root.path(), json!({}), &preview),
        error(
            "`reply` has no preview of its own; style its parameters as `function_call` or `json` \
             rather than `tool`."
        )
    );
}

/// A store inside the workspace is held to the call's access policy, as the
/// files a tool writes there are the workspace's.
#[test]
fn a_store_inside_the_workspace_is_held_to_the_access_policy() {
    let (root, id, pass) = reviewed();
    let user = root.path().join(".user/review");
    let inside = root.path().join("reviews");
    drop(pass);
    fs::rename(&user, &inside).unwrap();
    let _pass = Store::new(inside.clone()).lock_pass(id).unwrap().unwrap();
    let options = json!({ "dir": "reviews" });
    let mut reply = call(
        "reply",
        json!({ "comment": format!("{id}#1"), "verdict": "accept", "body": "Done." }),
        CONVERSATION,
    );

    reply.access = Some(json!({ "fs": [{ "lexical_path": "docs", "read": true }] }));
    let denied = run_with(root.path(), options.clone(), &reply);
    assert!(
        matches!(&denied, Outcome::Error { message, .. }
            if message.starts_with("`reply` can't write to the review store: ")),
        "{denied:?}"
    );

    reply.access = Some(json!({
        "fs": [{ "lexical_path": "reviews", "read": true, "update": true }]
    }));
    let allowed = run_with(root.path(), options, &reply);
    assert_eq!(allowed, success(&format!("Replied to {id}#1, accept.")));
    assert_eq!(
        Store::new(inside)
            .peek(id)
            .unwrap()
            .comment(1)
            .unwrap()
            .replies
            .len(),
        1
    );
}

#[test]
fn whole_lines_are_a_line_or_a_range_of_them() {
    assert_eq!(
        whole_lines("42-45"),
        Ok(Range::Lines {
            first: 42,
            last: 45
        })
    );
    assert!(whole_lines("x").is_err());
}

#[test]
fn a_body_is_trimmed_and_bounded() {
    assert_eq!(bounded(" ok ", 2), Ok("ok".to_owned()));
    assert_eq!(bounded("éé", 2), Ok("éé".to_owned()));
    assert_eq!(bounded(" ", 2), Err("`body` is empty.".to_owned()));
}
