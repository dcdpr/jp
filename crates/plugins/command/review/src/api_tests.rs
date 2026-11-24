use std::fs::{self, File};

use camino_tempfile::Utf8TempDir;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;
use crate::{
    pass::{Ended, NewPass, Outcome},
    snapshot,
};

/// What `docs/a.md` holds when a test starts.
const MARKDOWN: &str = "# Title\n\nFirst line.\nSecond line.\n";

/// A workspace named `notes` holding `docs/a.md` and `docs/b.txt`, its reviews
/// kept in user-local storage.
fn workspace() -> (Utf8TempDir, Workspace) {
    let tmp = Utf8TempDir::new().unwrap();
    let root = tmp.path().join("notes");
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/a.md"), MARKDOWN).unwrap();
    fs::write(root.join("docs/b.txt"), "plain <text>\n").unwrap();
    let workspace = Workspace {
        store: Store::new(root.join(".user/review")),
        root,
        config: json!({ "user": { "name": "Jane Doe <jane@example.com>" } }),
        options: Options::default(),
    };

    (tmp, workspace)
}

fn body(reply: &Reply) -> Value {
    serde_json::from_str(&reply.json).unwrap()
}

/// Start a review of both files, returning its id.
fn started(workspace: &Workspace) -> ReviewId {
    let reply = create(&workspace.context(), &NewReview {
        files: vec!["docs/a.md".to_owned(), "docs/b.txt".to_owned()],
        title: None,
    })
    .unwrap();

    body(&reply)["id"].as_str().unwrap().parse().unwrap()
}

/// Record what the working tree holds as the review's next version.
fn record_tree(workspace: &Workspace, id: ReviewId) {
    workspace
        .store
        .update(id, |review, blobs| {
            snapshot::record(&workspace.root, review, blobs, now()).map(drop)
        })
        .unwrap();
}

/// Send every draft of the review in a pass, which runs until `end` is called.
///
/// The pass runs as long as the returned [`File`], which holds its `.pass` lock
/// the way the process running a pass does, is kept: a pass nobody holds the
/// lock for was left by a process that is gone.
fn send(workspace: &Workspace, id: ReviewId) -> File {
    let pass = workspace.store.lock_pass(id).unwrap().unwrap();
    workspace
        .store
        .update(id, |review, _| {
            review
                .submit(NewPass {
                    author: "Jane".to_owned(),
                    created_at: now(),
                    version: 1,
                    only: vec![],
                    message: Some("Go.".to_owned()),
                    conversation: None,
                })
                .map(drop)
                .map_err(|refusal| refusal.message(id))
        })
        .unwrap();

    pass
}

fn end(workspace: &Workspace, id: ReviewId) {
    workspace
        .store
        .update(id, |review, _| {
            review.passes.end(1, Ended {
                outcome: Outcome::Completed,
                at: now(),
                version: Some(1),
                error: None,
                answer: None,
            });
            Ok::<_, String>(())
        })
        .unwrap();
}

fn new_comment(anchor: &Value, body: &str) -> NewCommentRequest {
    serde_json::from_value(json!({ "anchor": anchor, "body": body })).unwrap()
}

#[test]
fn creating_a_review_answers_its_summary() {
    let (_tmp, workspace) = workspace();

    let reply = create(&workspace.context(), &NewReview {
        files: vec!["docs/a.md".to_owned(), "./docs/a.md".to_owned()],
        title: Some(" The title ".to_owned()),
    })
    .unwrap();

    let summary = body(&reply);
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(summary["title"], "The title");
    assert_eq!(summary["files"], json!(["docs/a.md"]));
    assert_eq!(summary["latest"], 1);
    assert_eq!(summary["author"], "Jane Doe <jane@example.com>");
}

#[test]
fn a_review_needs_files_inside_the_workspace() {
    let (_tmp, workspace) = workspace();
    let start = |files: &[&str]| {
        create(&workspace.context(), &NewReview {
            files: files.iter().map(|file| (*file).to_owned()).collect(),
            title: None,
        })
        .unwrap_err()
    };

    assert_eq!(
        start(&[]),
        ApiError::invalid("A review needs at least one file.")
    );
    assert_eq!(
        start(&["docs/c.md"]),
        ApiError::invalid("No file at docs/c.md.")
    );
    assert_eq!(
        start(&["../outside.md"]),
        ApiError::invalid("../outside.md is outside the workspace.")
    );
}

/// One review that can't be read doesn't hide the others.
#[test]
fn the_list_holds_every_review_and_those_that_cant_be_read() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let broken = started(&workspace);
    fs::write(
        workspace
            .root
            .join(".user/review")
            .join(broken.as_str())
            .join("review.json"),
        "{",
    )
    .unwrap();

    let listing = body(&list(&workspace).unwrap());

    assert_eq!(listing["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(listing["reviews"][0]["id"], id.to_string());
    assert_eq!(listing["unreadable"][0]["id"], broken.to_string());
}

/// A caller polling with the revision it has is told nothing changed, and still
/// who is running a pass.
#[test]
fn the_review_is_left_out_when_the_caller_has_its_revision() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let first = body(&detail(&workspace, id, &DetailQuery::default(), None).unwrap());
    assert_eq!(first["revision"], 1);
    assert_eq!(first["running"], Value::Null);
    assert_eq!(first["review"]["id"], id.to_string());

    let unchanged = detail(
        &workspace,
        id,
        &DetailQuery {
            revision: Some(1),
            version: None,
        },
        None,
    )
    .unwrap();

    assert_eq!(
        body(&unchanged),
        json!({
            "workspace": "notes",
            "revision": 1,
            "running": null,
            "working_tree_changed": false
        })
    );
}

/// Every poll says whether the files changed on disk, the caller holding the
/// revision or not; a pass running records them itself when it ends.
#[test]
fn the_review_says_whether_the_working_tree_changed_since_the_latest_version() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let polled = || {
        body(
            &detail(
                &workspace,
                id,
                &DetailQuery {
                    revision: Some(1),
                    version: None,
                },
                None,
            )
            .unwrap(),
        )["working_tree_changed"]
            .clone()
    };
    assert_eq!(polled(), json!(false));

    fs::write(workspace.root.join("docs/b.txt"), "edited\n").unwrap();
    assert_eq!(polled(), json!(true));

    let pass = send(&workspace, id);
    assert_eq!(polled(), json!(false));
    drop(pass);
}

#[test]
fn the_list_names_the_workspace() {
    let (_tmp, workspace) = workspace();

    assert_eq!(
        body(&list(&workspace).unwrap()),
        json!({ "workspace": "notes", "reviews": [] })
    );
}

/// The banner's button: what changed on disk becomes the latest version, once.
#[test]
fn the_working_tree_is_recorded_as_a_version_when_it_changed() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);

    let unchanged = record(&workspace.context(), id).unwrap();
    assert_eq!(
        (unchanged.status, body(&unchanged)),
        (
            StatusCode::OK,
            json!({ "review": id.to_string(), "version": 1, "recorded": false })
        )
    );

    fs::write(workspace.root.join("docs/a.md"), "# Title\n").unwrap();
    let recorded = record(&workspace.context(), id).unwrap();
    assert_eq!(
        (recorded.status, body(&recorded)),
        (
            StatusCode::CREATED,
            json!({ "review": id.to_string(), "version": 2, "recorded": true })
        )
    );
}

/// A pass whose process is gone holds no `.pass` lock, so recording the files
/// takes the lock, and records the pass as interrupted.
#[test]
fn recording_the_working_tree_settles_a_pass_whose_process_is_gone() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    drop(send(&workspace, id));
    fs::write(workspace.root.join("docs/a.md"), "# Title\n").unwrap();

    let recorded = record(&workspace.context(), id).unwrap();

    assert_eq!(recorded.status, StatusCode::CREATED);
    let review = workspace.store.peek(id).unwrap();
    assert_eq!(
        review
            .passes
            .get(1)
            .unwrap()
            .ended
            .as_ref()
            .map(|ended| ended.outcome),
        Some(Outcome::Interrupted)
    );
}

/// The version a pass records when it ends holds the files; one recorded beside
/// it would split the pass's edits in two.
#[test]
fn the_working_tree_is_not_recorded_while_a_pass_runs() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    fs::write(workspace.root.join("docs/a.md"), "# Title\n").unwrap();
    let pass = send(&workspace, id);

    assert_eq!(
        record(&workspace.context(), id),
        Err(ApiError::conflict(format!(
            "A pass of {id} is running; wait for it to end, or stop it where it runs."
        )))
    );
    drop(pass);
}

#[test]
fn the_running_pass_says_whether_this_server_runs_it() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let _pass = send(&workspace, id);

    let here = body(&detail(&workspace, id, &DetailQuery::default(), Some(1)).unwrap());
    let elsewhere = body(&detail(&workspace, id, &DetailQuery::default(), None).unwrap());

    assert_eq!(here["running"], json!({ "number": 1, "stoppable": true }));
    assert_eq!(
        elsewhere["running"],
        json!({ "number": 1, "stoppable": false })
    );
}

#[test]
fn a_review_or_version_that_isnt_there_is_not_found() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let other: ReviewId = "R-0000000".parse().unwrap();

    assert_eq!(
        review_id("nope"),
        Err(ApiError::not_found("No review nope."))
    );
    assert_eq!(
        detail(&workspace, other, &DetailQuery::default(), None),
        Err(ApiError::not_found("No review R-0000000."))
    );
    assert_eq!(
        detail(
            &workspace,
            id,
            &DetailQuery {
                revision: None,
                version: Some(9),
            },
            None
        ),
        Err(ApiError::not_found(format!(
            "{id} has no version 9; its latest is 1."
        )))
    );
    assert_eq!(
        comment_ref(id, "0"),
        Err(ApiError::not_found(format!("No comment {id}#0.")))
    );
    assert_eq!(
        version_number(id, "x"),
        Err(ApiError::not_found(format!("{id} has no version x.")))
    );
}

#[test]
fn markdown_is_rendered_as_markdown_and_other_files_as_text() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let html = |path: &str| FileQuery {
        path: path.to_owned(),
        format: Format::Html,
    };

    let markdown = body(&file(&workspace.store, id, 1, &html("docs/a.md")).unwrap());
    let text = body(&file(&workspace.store, id, 1, &html("docs/b.txt")).unwrap());

    assert_eq!(markdown["source"], MARKDOWN);
    assert!(
        markdown["html"]
            .as_str()
            .unwrap()
            .starts_with(r#"<h1><span data-s="2" data-e="7">Title</span></h1>"#),
        "{markdown}"
    );
    assert_eq!(
        text["html"],
        "<pre><span data-s=\"0\" data-e=\"12\">plain &lt;text&gt;</span>\n</pre>\n"
    );
}

#[test]
fn a_file_as_text_carries_only_its_source() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);

    let reply = file(&workspace.store, id, 1, &FileQuery {
        path: "docs/b.txt".to_owned(),
        format: Format::Text,
    })
    .unwrap();

    assert_eq!(
        body(&reply),
        json!({
            "review": id.to_string(),
            "version": 1,
            "path": "docs/b.txt",
            "source": "plain <text>\n"
        })
    );
}

#[test]
fn a_file_the_review_doesnt_track_or_the_version_lacks_is_not_found() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    fs::remove_file(workspace.root.join("docs/b.txt")).unwrap();
    record_tree(&workspace, id);
    let text = |path: &str| FileQuery {
        path: path.to_owned(),
        format: Format::Text,
    };

    assert_eq!(
        file(&workspace.store, id, 2, &text("docs/b.txt")),
        Err(ApiError::not_found("docs/b.txt is absent from version 2."))
    );
    assert_eq!(
        file(&workspace.store, id, 1, &text("docs/c.md")),
        Err(ApiError::not_found(format!(
            "`docs/c.md` is not a file in {id}, which tracks docs/a.md, docs/b.txt."
        )))
    );
}

#[test]
fn a_diff_can_be_of_one_file() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    fs::write(workspace.root.join("docs/a.md"), "# Title\n").unwrap();
    fs::write(workspace.root.join("docs/b.txt"), "changed\n").unwrap();
    record_tree(&workspace, id);

    let both = body(&diff(&workspace.store, id, &DiffQuery::default()).unwrap());
    let one = body(
        &diff(&workspace.store, id, &DiffQuery {
            from: Some(1),
            to: Some(2),
            path: Some("docs/b.txt".to_owned()),
            ..DiffQuery::default()
        })
        .unwrap(),
    );

    assert_eq!(
        (both["from"].clone(), both["to"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(both["files"].as_array().unwrap().len(), 2);
    assert_eq!(one["files"].as_array().unwrap().len(), 1);
    assert_eq!(one["files"][0]["path"], "docs/b.txt");
}

/// The changes are placed in the later version, for a page rendering it to
/// highlight: the lines removed from `docs/a.md` were at its end, and the one
/// line of `docs/b.txt` changed.
#[test]
fn a_diff_places_what_changed_in_the_later_version() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    fs::write(workspace.root.join("docs/a.md"), "# Title\n").unwrap();
    fs::write(workspace.root.join("docs/b.txt"), "changed\n").unwrap();
    record_tree(&workspace, id);

    let changes = body(&diff(&workspace.store, id, &DiffQuery::default()).unwrap());

    assert_eq!(
        (
            changes["files"][0]["added"].clone(),
            changes["files"][0]["removed"].clone()
        ),
        (json!([]), json!([8]))
    );
    assert_eq!(
        (
            changes["files"][1]["added"].clone(),
            changes["files"][1]["removed"].clone()
        ),
        (json!([[0, 7]]), json!([]))
    );
}

/// What a pass changed is from the version it was sent against to the one
/// recorded when it ended, which only an ended pass has.
#[test]
fn a_diff_can_be_of_what_a_pass_changed() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let pass = send(&workspace, id);
    let of_pass = |pass, from| DiffQuery {
        pass: Some(pass),
        from,
        ..DiffQuery::default()
    };

    assert_eq!(
        diff(&workspace.store, id, &of_pass(1, None)),
        Err(ApiError::conflict(format!(
            "Pass 1 of {id} is still running; what it changes is recorded when it ends."
        )))
    );

    fs::write(workspace.root.join("docs/b.txt"), "changed\n").unwrap();
    record_tree(&workspace, id);
    workspace
        .store
        .update(id, |review, _| {
            review.passes.end(1, Ended {
                outcome: Outcome::Completed,
                at: now(),
                version: Some(2),
                error: None,
                answer: None,
            });
            Ok::<_, String>(())
        })
        .unwrap();
    drop(pass);

    let changed = body(&diff(&workspace.store, id, &of_pass(1, None)).unwrap());
    assert_eq!(
        (changed["from"].clone(), changed["to"].clone()),
        (json!(1), json!(2))
    );
    assert_eq!(changed["files"][0]["path"], "docs/b.txt");
    assert_eq!(
        diff(&workspace.store, id, &of_pass(2, None)),
        Err(ApiError::not_found(format!("{id} has no pass 2.")))
    );
    assert_eq!(
        diff(&workspace.store, id, &of_pass(1, Some(1))),
        Err(ApiError::invalid(
            "Name either `pass`, or `from` and `to`, not both."
        ))
    );
}

/// The browser addresses the text by bytes; the review keeps lines and columns,
/// and gives the bytes back in every projection.
#[test]
fn a_comment_on_bytes_is_kept_as_lines_and_columns() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);

    let reply = comment(
        &workspace.context(),
        id,
        new_comment(
            &json!({ "version": 1, "path": "docs/a.md", "bytes": [9, 14] }),
            "Name it.",
        ),
    )
    .unwrap();

    let made = body(&reply);
    assert_eq!(reply.status, StatusCode::CREATED);
    assert_eq!(made["id"], format!("{id}#1"));
    assert_eq!(made["status"], "draft");
    assert_eq!(made["excerpt"], "First");
    assert_eq!(
        made["body_html"],
        "<p><span data-s=\"0\" data-e=\"8\">Name it.</span></p>\n"
    );
    assert_eq!(
        made["anchor"],
        json!({
            "version": 1,
            "path": "docs/a.md",
            "range": "3:1-3:6",
            "projected": { "version": 1, "range": "3:1-3:6", "bytes": [9, 14] },
            "outdated": false
        })
    );
}

#[test]
fn a_comment_without_an_anchor_is_on_the_review_in_its_latest_version() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);

    let reply = comment(
        &workspace.context(),
        id,
        serde_json::from_value(json!({ "body": "Too long." })).unwrap(),
    )
    .unwrap();

    assert_eq!(
        body(&reply)["anchor"],
        json!({ "version": 1, "projected": { "version": 1 }, "outdated": false })
    );
}

#[test]
fn a_range_names_its_file_and_the_version_it_was_read_in() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let refused =
        |anchor: Value| comment(&workspace.context(), id, new_comment(&anchor, "x")).unwrap_err();

    assert_eq!(
        refused(json!({ "version": 1, "range": "3" })),
        ApiError::invalid("A range needs `path`, the file it is in.")
    );
    assert_eq!(
        refused(json!({ "path": "docs/a.md", "range": "3" })),
        ApiError::invalid("A range needs `version`, the version of the file it was read in.")
    );
    assert_eq!(
        refused(json!({ "version": 1, "path": "docs/a.md", "range": "3", "bytes": [0, 1] })),
        ApiError::invalid("Give the range as `range` or as `bytes`, not both.")
    );
    assert_eq!(
        refused(json!({ "version": 1, "path": "docs/a.md", "range": "9" })),
        ApiError::invalid("9 lies outside docs/a.md as version 1 holds it.")
    );
    assert_eq!(
        refused(json!({ "version": 1, "path": "docs/a.md", "bytes": [0, 999] })),
        ApiError::invalid(
            "Bytes 0 to 999 lie outside docs/a.md as version 1 holds it, inside a character, or \
             end before they start."
        )
    );
    assert_eq!(
        refused(json!({ "version": 1, "path": "docs/c.md" })).status,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn a_comment_the_review_refuses_answers_with_the_status_that_fits() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);

    let empty = comment(
        &workspace.context(),
        id,
        new_comment(&json!({ "version": 1 }), " "),
    );
    close(&workspace.store, id).unwrap();
    let closed = comment(&workspace.context(), id, new_comment(&json!(null), "Late."));

    assert_eq!(empty, Err(ApiError::invalid("The body is empty.")));
    assert_eq!(closed, Err(ApiError::conflict(format!("{id} is closed."))));
}

#[test]
fn a_draft_is_edited_and_withdrawn() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    comment(
        &workspace.context(),
        id,
        new_comment(&json!(null), "Draft."),
    )
    .unwrap();
    let first = CommentRef {
        review: id,
        number: 1,
    };

    let edited = edit(
        &workspace.context(),
        first,
        serde_json::from_value(json!({
            "anchor": { "version": 1, "path": "docs/a.md", "range": "3" },
            "suggestion": "Line one."
        }))
        .unwrap(),
    )
    .unwrap();
    let withdrawn = withdraw(&workspace.store, first).unwrap();

    assert_eq!(body(&edited)["suggestion"], "Line one.");
    assert_eq!(body(&edited)["body"], "Draft.");
    assert_eq!(
        body(&withdrawn),
        json!({ "id": format!("{id}#1"), "withdrawn": "comment" })
    );
    assert_eq!(
        withdraw(&workspace.store, first),
        Err(ApiError::not_found(format!("{id}#1 was withdrawn.")))
    );
}

/// A reply goes on a sent comment, and resolving one closes it.
#[test]
fn a_sent_comment_takes_a_reply_and_is_resolved() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    comment(&workspace.context(), id, new_comment(&json!(null), "Hm.")).unwrap();
    let first = CommentRef {
        review: id,
        number: 1,
    };
    let replying = ReplyRequest {
        body: " Why? ".to_owned(),
    };

    assert_eq!(
        reply(&workspace.context(), first, &replying),
        Err(ApiError::conflict(format!(
            "{id}#1 is a draft, not yet sent."
        )))
    );
    let pass = send(&workspace, id);
    end(&workspace, id);
    drop(pass);
    let replied = reply(&workspace.context(), first, &replying).unwrap();
    assert_eq!(body(&replied)["draft_reply"]["body"], "Why?");

    withdraw(&workspace.store, first).unwrap();
    let resolved = resolve(&workspace.store, first).unwrap();
    assert_eq!(
        body(&resolved),
        json!({ "id": format!("{id}#1"), "outcome": "resolved" })
    );
}

/// The assistant answering a running pass could no longer write its replies.
#[test]
fn a_review_with_a_pass_running_is_not_closed() {
    let (_tmp, workspace) = workspace();
    let id = started(&workspace);
    let pass = send(&workspace, id);

    assert_eq!(
        close(&workspace.store, id),
        Err(ApiError::conflict(format!("Pass 1 of {id} has not ended.")))
    );
    end(&workspace, id);
    drop(pass);
    assert_eq!(
        body(&close(&workspace.store, id).unwrap())["status"],
        "closed"
    );
}

#[test]
fn a_refusal_answers_with_the_status_that_fits_it() {
    let id: ReviewId = "R-1h4k2m9".parse().unwrap();
    let status = |refusal: Refusal| ApiError::refused(id, &refusal).status;

    assert_eq!(
        status(Refusal::Comment(2, Why::Missing)),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status(Refusal::Comment(2, Why::Withdrawn)),
        StatusCode::NOT_FOUND
    );
    assert_eq!(status(Refusal::NothingToChange), StatusCode::BAD_REQUEST);
    assert_eq!(
        status(Refusal::SuggestionWithoutRange),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        status(Refusal::Comment(2, Why::Draft)),
        StatusCode::CONFLICT
    );
    assert_eq!(status(Refusal::NothingToSend), StatusCode::CONFLICT);
    assert_eq!(
        status(Refusal::Bound("jp-c1".to_owned())),
        StatusCode::CONFLICT
    );
    assert_eq!(
        ApiError::from(Failure::Other("disk full".to_owned())),
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "disk full")
    );
}

#[test]
fn an_error_is_answered_as_json() {
    assert_eq!(
        ApiError::not_found("No review R-1h4k2m9.").body(),
        r#"{"error":"No review R-1h4k2m9."}"#
    );
}
