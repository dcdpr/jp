#![expect(
    clippy::unused_async_trait_impl,
    reason = "test doubles implement async traits synchronously"
)]

use std::future;

use jp_config::model::id::Name;
use jp_conversation::{Conversation, ConversationId};
use jp_llm::{Error as LlmError, EventStream, model::ModelDetails, query::ChatQuery};
use jp_mcp::{Startup, id::McpServerId};
use jp_printer::OutputFormat;
use jp_workspace::Workspace;
use tokio::{
    sync::{Notify, broadcast, oneshot::error::TryRecvError},
    task::JoinSet,
};

use super::*;
use crate::signals::testing::{detached_router, test_router};

/// A stop request that lands while MCP servers are starting ends the turn
/// before it runs.
///
/// Starting a server can mean compiling one, so this is the longest stretch
/// between taking the conversation lock and the turn loop reading its first
/// event.
/// A handler registered only once that loop starts would leave a stop request
/// with nothing to reach for the whole of it.
///
/// The startup here never completes, so the only way out of `run` is the
/// interrupt: without it the test hangs, and the timeout reports that as a
/// failure rather than letting it run forever.
#[tokio::test]
async fn an_interrupt_during_mcp_startup_stops_the_turn_before_it_runs() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    let conversation_id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
    )
    .unwrap();
    let base_config = Arc::new(AppConfig::new_test());
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    // Held open for the life of the test: the server never finishes starting,
    // so the startup wait never returns on its own.
    let (_release, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (_lines, stderr) = broadcast::channel(1);
    let mcp_servers = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm")],
        stderr,
    };

    let (router, signals) = test_router();
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);

    let inputs = TurnInputs {
        config: Arc::clone(&base_config),
        signals: router.clone(),
        mcp_client: jp_mcp::Client::default(),
        workspace_root: tmp.path().to_path_buf(),
        interactive: false,
        attachments: vec![],
        printer: Arc::new(printer),
        approvals: Arc::new(crate::access::approvals::ApprovalStore::default()),
        command_plugins: CommandPlugins::default(),
        chat_request: ChatRequest::from("hello"),
        workspace_id: workspace.id().clone(),
        pending_trim: PendingStreamTrim::default(),
        mcp_servers,
        // Ctrl-C is how this turn is interrupted, and this test drives that
        // through the router rather than over a channel.
        interrupts: TurnInterrupts::none(),
        stubbed_provider: None,
    };

    // Registered where the lock is taken, so the press reaches it even though
    // the turn has not started. It is the only handler on the stack at this
    // point, so it is the one a Ctrl-C finds, and the notice waits on the
    // channel until `run` polls it.
    let interrupt = router.turn_interrupt();
    signals.interrupt().await;

    let stream = lock.events().clone();
    let ending = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inputs.run(&lock, stream, interrupt),
    )
    .await
    .expect("the interrupt ends the wait; a hang here means it was never seen")
    .expect("stopping before the turn starts is not an error");

    assert_eq!(ending, TurnEnding::Stopped);
    assert!(
        lock.events().is_empty(),
        "the turn was interrupted before its request was appended"
    );
}

/// A client's stop reaches the turn while MCP servers are starting, the same as
/// a Ctrl-C does.
///
/// A client with no terminal has only this channel: the web UI's Cancel is sent
/// in exactly this window, while the message it pulls back has not been
/// recorded yet.
/// A reply sent in the same window is held for the turn rather than refused or
/// acted on early, so it is still unacknowledged when the stop ends the turn,
/// and refused then.
///
/// The startup never completes, so the only way out of `run` is the stop.
#[tokio::test]
async fn a_client_stop_during_mcp_startup_stops_the_turn_before_it_runs() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    let conversation_id = ConversationId::try_from(
        chrono::DateTime::<chrono::Utc>::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000),
    )
    .unwrap();
    let base_config = Arc::new(AppConfig::new_test());
    workspace.create_conversation_with_id(
        conversation_id,
        Conversation::default(),
        Arc::clone(&base_config),
    );
    let handle = workspace.acquire_conversation(&conversation_id).unwrap();
    let lock = workspace.test_lock(handle);

    let (_release, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = tokio::task::JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (_lines, stderr) = broadcast::channel(1);
    let mcp_servers = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm")],
        stderr,
    };

    // Nothing presses Ctrl-C in this test; the router is only there because a
    // turn always has one.
    let router = detached_router();
    let (printer, _out, _err) = Printer::memory(OutputFormat::TextPretty);
    let (client, interrupts) = TurnInterrupts::channel();

    let inputs = TurnInputs {
        config: Arc::clone(&base_config),
        signals: router.clone(),
        mcp_client: jp_mcp::Client::default(),
        workspace_root: tmp.path().to_path_buf(),
        interactive: false,
        attachments: vec![],
        printer: Arc::new(printer),
        approvals: Arc::new(crate::access::approvals::ApprovalStore::default()),
        command_plugins: CommandPlugins::default(),
        chat_request: ChatRequest::from("hello"),
        workspace_id: workspace.id().clone(),
        pending_trim: PendingStreamTrim::default(),
        mcp_servers,
        interrupts,
        stubbed_provider: None,
    };

    let mut reply = client
        .try_send(InterruptAction::Reply {
            content: "and in Rust".to_owned(),
            echo: true,
        })
        .unwrap();
    let stop = client.try_send(InterruptAction::Stop).unwrap();

    let interrupt = router.turn_interrupt();
    let stream = lock.events().clone();
    let ending = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inputs.run(&lock, stream, interrupt),
    )
    .await
    .expect("the stop ends the wait; a hang here means it was never seen")
    .expect("stopping before the turn starts is not an error");

    assert_eq!(ending, TurnEnding::Stopped);
    assert!(
        lock.events().is_empty(),
        "the turn was stopped before its request was appended"
    );
    assert_eq!(stop.await, Ok(()), "the stop was acted on");
    assert_eq!(
        reply.try_recv(),
        Err(TryRecvError::Closed),
        "the reply was never acted on, so it is refused rather than reported delivered"
    );
}

#[tokio::test]
async fn a_client_abort_during_mcp_startup_ends_the_turn_as_aborted() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    let base_config = Arc::new(AppConfig::new_test());
    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::clone(&base_config), None)
        .unwrap();

    let (_release, release_rx) = tokio::sync::oneshot::channel::<()>();
    let mut joins = JoinSet::new();
    joins.spawn(async move {
        release_rx.await.ok();
        Ok(Startup::Ready(McpServerId::new("bookworm")))
    });
    let (_lines, stderr) = broadcast::channel(1);
    let mcp_servers = StartupSet {
        joins,
        pending: vec![McpServerId::new("bookworm")],
        stderr,
    };

    let router = detached_router();
    let (client, interrupts) = TurnInterrupts::channel();
    let inputs = turn_inputs(
        &workspace,
        base_config,
        &router,
        mcp_servers,
        interrupts,
        None,
    );

    let abort = client.try_send(InterruptAction::Abort).unwrap();

    let interrupt = router.turn_interrupt();
    let stream = lock.events().clone();
    let ending = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inputs.run(&lock, stream, interrupt),
    )
    .await
    .expect("the abort ends the wait; a hang here means it was never seen")
    .expect("aborting before the turn starts is not an error");

    assert_eq!(ending, TurnEnding::Aborted);
    assert_eq!(abort.await, Ok(()), "the abort was acted on");
    assert!(
        lock.events().is_empty(),
        "the turn was aborted before its request was appended"
    );
}

/// Waiting on `asked` ensures MCP startup has already completed and the turn is
/// blocked in `provider.model_details` when the interrupt fires.
#[tokio::test]
async fn an_interrupt_during_the_model_lookup_stops_the_turn_before_it_runs() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    let base_config = Arc::new(AppConfig::new_test());
    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::clone(&base_config), None)
        .unwrap();

    let asked = Arc::new(Notify::new());
    let provider: Arc<dyn DynProvider> = Arc::new(UnansweredLookup {
        asked: Arc::clone(&asked),
    });

    let (router, signals) = test_router();
    let inputs = turn_inputs(
        &workspace,
        base_config,
        &router,
        no_mcp_servers(),
        TurnInterrupts::none(),
        Some(provider),
    );

    let interrupt = router.turn_interrupt();
    let press = tokio::spawn(async move {
        asked.notified().await;
        signals.interrupt().await;
    });

    let stream = lock.events().clone();
    let ending = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inputs.run(&lock, stream, interrupt),
    )
    .await
    .expect("the interrupt ends the lookup; a hang here means it was never seen")
    .expect("stopping before the turn starts is not an error");

    press.await.unwrap();
    assert_eq!(ending, TurnEnding::Stopped);
    assert!(
        lock.events().is_empty(),
        "the turn was interrupted before its request was appended"
    );
}

/// Waiting on `asked` ensures MCP startup has already completed and the turn is
/// blocked in `provider.model_details` when the client abort arrives.
#[tokio::test]
async fn a_client_abort_during_the_model_lookup_ends_the_turn_as_aborted() {
    let tmp = camino_tempfile::tempdir().unwrap();
    let mut workspace = Workspace::in_memory(tmp.path());
    let base_config = Arc::new(AppConfig::new_test());
    let lock = workspace
        .create_and_lock_conversation(Conversation::default(), Arc::clone(&base_config), None)
        .unwrap();

    let asked = Arc::new(Notify::new());
    let provider: Arc<dyn DynProvider> = Arc::new(UnansweredLookup {
        asked: Arc::clone(&asked),
    });

    let router = detached_router();
    let (client, interrupts) = TurnInterrupts::channel();
    let inputs = turn_inputs(
        &workspace,
        base_config,
        &router,
        no_mcp_servers(),
        interrupts,
        Some(provider),
    );

    let abort = tokio::spawn(async move {
        asked.notified().await;
        client
            .try_send(InterruptAction::Abort)
            .expect("the turn is still running")
            .await
    });

    let interrupt = router.turn_interrupt();
    let stream = lock.events().clone();
    let ending = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        inputs.run(&lock, stream, interrupt),
    )
    .await
    .expect("the abort ends the lookup; a hang here means it was never seen")
    .expect("aborting before the turn starts is not an error");

    assert_eq!(abort.await.unwrap(), Ok(()), "the abort was acted on");
    assert_eq!(ending, TurnEnding::Aborted);
    assert!(
        lock.events().is_empty(),
        "the turn was aborted before its request was appended"
    );
}

type LlmResult<T> = std::result::Result<T, LlmError>;

/// Mock provider whose `model_details` call notifies `asked` and then hangs.
struct UnansweredLookup {
    asked: Arc<Notify>,
}

impl jp_llm::Provider for UnansweredLookup {
    async fn model_details(&self, _name: &Name) -> LlmResult<ModelDetails> {
        self.asked.notify_one();
        future::pending().await
    }

    async fn models(&self) -> LlmResult<Vec<ModelDetails>> {
        Ok(vec![])
    }

    async fn chat_completion_stream(
        &self,
        _model: &ModelDetails,
        _query: ChatQuery,
    ) -> LlmResult<EventStream> {
        unreachable!("the turn never gets past the model lookup")
    }
}

fn no_mcp_servers() -> StartupSet {
    let (_lines, stderr) = broadcast::channel(1);
    StartupSet {
        joins: JoinSet::new(),
        pending: vec![],
        stderr,
    }
}

fn turn_inputs(
    workspace: &Workspace,
    config: Arc<AppConfig>,
    router: &SignalRouter,
    mcp_servers: StartupSet,
    interrupts: TurnInterrupts,
    stubbed_provider: Option<Arc<dyn DynProvider>>,
) -> TurnInputs {
    TurnInputs {
        config,
        signals: router.clone(),
        mcp_client: jp_mcp::Client::default(),
        workspace_root: workspace.root().to_path_buf(),
        interactive: false,
        attachments: vec![],
        printer: Arc::new(Printer::sink()),
        approvals: Arc::new(crate::access::approvals::ApprovalStore::default()),
        command_plugins: CommandPlugins::default(),
        chat_request: ChatRequest::from("hello"),
        workspace_id: workspace.id().clone(),
        pending_trim: PendingStreamTrim::default(),
        mcp_servers,
        interrupts,
        stubbed_provider,
    }
}
