use std::{
    fs,
    io::{self, BufReader},
};

use camino_tempfile::Utf8TempDir;
use jp_plugin::message::{
    ErrorResponse, InterruptAction, InterruptRequest, QueryCompleteResponse, QueryOutcome,
    QueryRequest,
};
use jp_review_api::{NewCommentRequest, NewReview};
use pretty_assertions::assert_eq;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::*;
use crate::{
    model::{Reply as Answered, Role, Verdict},
    options::Options,
    pass::{Ended, Outcome, Pass},
    store::{Store, now},
    tests::test_support::{CONVERSATION, Host, Written, eventually, within},
};

/// A review of `docs/a.md` with one draft comment, and the passes of a server
/// whose host the test plays.
struct Fixture {
    _root: Utf8TempDir,
    id: ReviewId,
    store: Store,
    workspace: Arc<Workspace>,
    passes: Arc<Passes>,
    written: Written,
    host: Host,
    _shutdown: watch::Receiver<bool>,
}

fn fixture() -> Fixture {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs")).unwrap();
    fs::write(root.path().join("docs/a.md"), "# Title\n\nFirst line.\n").unwrap();
    let workspace = Arc::new(Workspace {
        root: root.path().to_owned(),
        config: json!({ "user": { "name": "Jane Doe <jane@example.com>" } }),
        options: Options::default(),
        store: Store::new(root.path().join(".user/review")),
    });

    let reply = api::create(&workspace.context(), &NewReview {
        files: vec!["docs/a.md".to_owned()],
        title: None,
    })
    .unwrap();
    let id: ReviewId = serde_json::from_str::<Value>(&reply.json).unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    let draft: NewCommentRequest = serde_json::from_value(json!({ "body": "Too long." })).unwrap();
    api::comment(&workspace.context(), id, draft).unwrap();

    let written = Written::default();
    let (reader, host) = io::pipe().unwrap();
    let client = Client::new(Arc::new(Mutex::new(Box::new(written.clone()))));
    let shutdown = client.listen(BufReader::new(reader)).unwrap();

    Fixture {
        _root: root,
        id,
        store: workspace.store.clone(),
        workspace,
        passes: Arc::new(Passes::new(client)),
        written,
        host: Host(Some(host)),
        _shutdown: shutdown,
    }
}

impl Fixture {
    /// Start a pass, in `conversation` when given, as a task of its own.
    fn start(&self, conversation: Option<&str>) -> task::JoinHandle<Result<Reply, ApiError>> {
        let passes = Arc::clone(&self.passes);
        let workspace = Arc::clone(&self.workspace);
        let request = Request::new(
            self.id,
            vec![],
            None,
            conversation.map(|conversation| conversation.parse().unwrap()),
            vec![],
        );

        tokio::spawn(async move { passes.start(workspace, request).await })
    }

    /// How pass 1 ended, once its outcome is recorded.
    async fn ended(&self) -> Ended {
        let store = self.store.clone();
        let id = self.id;
        eventually(move || {
            store.peek(id).is_ok_and(|review| {
                review
                    .passes
                    .get(1)
                    .is_some_and(|pass| pass.ended.is_some())
            })
        })
        .await;

        self.store
            .peek(self.id)
            .unwrap()
            .passes
            .get(1)
            .unwrap()
            .ended
            .clone()
            .unwrap()
    }
}

fn body(reply: &Reply) -> Value {
    serde_json::from_str(&reply.json).unwrap()
}

/// The whole of a first pass: the host creates the conversation, the assistant
/// replies through its tool while the turn runs, and the turn ends.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_pass_starts_a_conversation_and_records_how_it_ended() {
    let mut f = fixture();
    let starting = f.start(None);
    let query = f.written.first_query().await;
    assert!(query.new);
    assert_eq!(query.title, Some(format!("Review {}: docs/a.md", f.id)));

    f.host.created(&query);
    let reply = within(starting).await.unwrap().unwrap();

    assert_eq!(reply.status, StatusCode::ACCEPTED);
    assert_eq!(body(&reply)["number"], 1);
    assert_eq!(body(&reply)["conversation"], CONVERSATION);
    assert_eq!(f.passes.running(f.id), Some(1));

    f.store
        .update(f.id, |review, _| {
            review
                .answer(1, Answered {
                    role: Role::Assistant,
                    author: "jp".to_owned(),
                    created_at: now(),
                    body: "Shortened.".to_owned(),
                    verdict: Some(Verdict::Accept),
                    pass: None,
                })
                .map_err(|refusal| refusal.message(f.id))
        })
        .unwrap();
    f.host.completed(&query, QueryOutcome::Finished);

    let ended = within(f.ended()).await;
    assert_eq!((ended.outcome, ended.error), (Outcome::Completed, None));
    let review = f.store.peek(f.id).unwrap();
    assert_eq!(review.passes.conversation.as_deref(), Some(CONVERSATION));
    assert_eq!(review.comments[0].replies[0].pass, Some(1));
    eventually(|| f.passes.running(f.id).is_none()).await;
    assert!(
        f.store.lock_pass(f.id).unwrap().is_some(),
        ".pass is let go"
    );
}

/// A pass in a conversation the review has gets no `created`, so it returns
/// once the host has the request, and can be stopped at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_in_a_known_conversation_is_stopped_where_it_runs() {
    let mut f = fixture();
    let reply = within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();
    let query = f.written.first_query().await;
    assert!(!query.new);
    assert_eq!(query.conversation, CONVERSATION);
    assert_eq!(body(&reply)["conversation"], CONVERSATION);

    assert_eq!(f.passes.stop(f.id, None), Ok(Stop::Stopping(1)));
    let interrupts = f.written.interrupts();
    assert_eq!(
        (interrupts[0].conversation.as_str(), interrupts[0].action),
        (CONVERSATION, InterruptAction::Stop)
    );

    f.host.completed(&query, QueryOutcome::Stopped);
    assert_eq!(within(f.ended()).await.outcome, Outcome::Stopped);
}

/// The conversation a stop has to name doesn't exist yet, so the stop waits for
/// the host to create it.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_before_the_conversation_exists_waits_for_it() {
    let mut f = fixture();
    let starting = f.start(None);
    let query = f.written.first_query().await;

    assert_eq!(f.passes.stop(f.id, Some(1)), Ok(Stop::Stopping(1)));
    assert_eq!(f.written.interrupts(), [] as [InterruptRequest; 0]);

    f.host.created(&query);
    within(starting).await.unwrap().unwrap();
    let written = f.written.clone();
    eventually(move || !written.interrupts().is_empty()).await;
    assert_eq!(f.written.interrupts()[0].conversation, CONVERSATION);

    f.host.completed(&query, QueryOutcome::Stopped);
    assert_eq!(within(f.ended()).await.outcome, Outcome::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pass_the_host_refuses_fails() {
    let mut f = fixture();
    let starting = f.start(None);
    let query = f.written.first_query().await;

    f.host.answer(&HostToPlugin::Error(ErrorResponse {
        id: query.id.clone(),
        request: Some("query".to_owned()),
        message: "unknown configuration `skill/review`".to_owned(),
    }));

    let refused = within(starting).await.unwrap().unwrap_err();
    assert_eq!(
        refused,
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            format!(
                "Pass 1 of {} failed: unknown configuration `skill/review`",
                f.id
            )
        )
    );
    let ended = within(f.ended()).await;
    assert_eq!(ended.outcome, Outcome::Failed);
    assert_eq!(
        ended.error.as_deref(),
        Some("unknown configuration `skill/review`")
    );
    assert_eq!(f.store.peek(f.id).unwrap().passes.conversation, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_review_runs_one_pass_at_a_time() {
    let f = fixture();
    within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();

    let second = within(f.start(Some(CONVERSATION))).await.unwrap();

    assert_eq!(
        second,
        Err(ApiError::conflict(format!(
            "A pass of {} is running; wait for it to end, or stop it where it runs.",
            f.id
        )))
    );
}

/// Nothing waits on the assistant, and no message was given.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_with_nothing_to_send_is_refused_and_leaves_no_lock() {
    let f = fixture();
    f.store
        .update(f.id, |review, _| {
            review
                .withdraw(1, None)
                .map_err(|refusal| refusal.message(f.id))
        })
        .unwrap();

    let refused = within(f.start(None)).await.unwrap();

    assert_eq!(refused.unwrap_err().status, StatusCode::CONFLICT);
    assert!(f.store.lock_pass(f.id).unwrap().is_some());
}

/// The server is shutting down: every pass is stopped, and each records how it
/// ended before the server exits.
#[tokio::test(flavor = "multi_thread")]
async fn shutting_down_stops_every_pass_and_waits_for_each() {
    let mut f = fixture();
    within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();
    let query = f.written.first_query().await;

    f.passes.stop_all();
    let finishing = tokio::spawn({
        let passes = Arc::clone(&f.passes);
        async move { passes.wait().await }
    });
    let written = f.written.clone();
    eventually(move || !written.interrupts().is_empty()).await;
    assert!(!finishing.is_finished(), "it waits for the pass to end");

    f.host.completed(&query, QueryOutcome::Stopped);
    within(finishing).await.unwrap();

    assert_eq!(
        f.store
            .peek(f.id)
            .unwrap()
            .passes
            .get(1)
            .unwrap()
            .ended
            .as_ref()
            .map(|ended| ended.outcome),
        Some(Outcome::Stopped)
    );
}

/// A pass started then would be stopped as soon as it began, its provider
/// request spent for nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_submitted_while_shutting_down_is_refused() {
    let f = fixture();
    f.passes.stop_all();

    let refused = within(f.start(None)).await.unwrap();

    assert_eq!(
        refused,
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The server is shutting down, and starts no more passes."
        ))
    );
    assert_eq!(f.written.queries(), [] as [QueryRequest; 0]);
    assert_eq!(f.store.peek(f.id).unwrap().passes.list, [] as [Pass; 0]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pass_whose_host_goes_away_is_interrupted() {
    let mut f = fixture();
    within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();

    f.host.leave();

    let ended = within(f.ended()).await;
    assert_eq!(ended.outcome, Outcome::Interrupted);
    assert_eq!(
        ended.error.as_deref(),
        Some("the connection to the host closed")
    );
}

#[test]
fn a_review_without_a_pass_here_has_none_to_stop() {
    let f = fixture();

    assert_eq!(f.passes.stop(f.id, None), Ok(Stop::None));
    assert_eq!(f.passes.running(f.id), None);
}

/// A page that missed the end of pass 1 asks to stop it, while pass 2 runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_naming_another_pass_stops_nothing() {
    let mut f = fixture();
    within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();
    let query = f.written.first_query().await;

    assert_eq!(
        f.passes.stop(f.id, Some(2)),
        Ok(Stop::Other {
            running: 1,
            asked: 2,
        })
    );
    assert_eq!(f.written.interrupts(), [] as [InterruptRequest; 0]);

    f.host.completed(&query, QueryOutcome::Finished);
    assert_eq!(within(f.ended()).await.outcome, Outcome::Completed);
}

/// The host reports the last thing the assistant said, and the pass keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn a_pass_records_the_closing_message_of_its_turn() {
    let mut f = fixture();
    within(f.start(Some(CONVERSATION))).await.unwrap().unwrap();
    let query = f.written.first_query().await;

    f.host
        .answer(&HostToPlugin::QueryComplete(QueryCompleteResponse {
            id: query.id.clone(),
            conversation: CONVERSATION.to_owned(),
            outcome: QueryOutcome::Finished,
            data: None,
            answer: Some("Shortened the title.".to_owned()),
        }));

    let ended = within(f.ended()).await;
    assert_eq!(
        (ended.outcome, ended.answer.as_deref()),
        (Outcome::Completed, Some("Shortened the title."))
    );
}
