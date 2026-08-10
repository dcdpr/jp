//! Host-side plugin message loop.
//!
//! Spawns the plugin binary, sends `init`, and relays workspace queries until
//! the plugin sends `exit` or the process terminates.

use std::{
    collections::{BTreeSet, HashSet},
    fmt::Write as _,
    fs,
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_config::{
    AppConfig,
    fs::user_global_config_dir,
    interrupt::{StreamingInterruptAction, ToolInterruptAction},
    plugins::{
        PluginsConfig,
        command::{CommandPluginConfig, RunPolicy},
    },
    util::list_configs_in_load_path,
};
use jp_conversation::{ConversationId, ConversationStream, event::ChatRequest};
use jp_editor::{EditOutcome, EditorBackend};
use jp_inquire::{InlineOption, InlineReply, InlineSelect, ReplyOutcome};
use jp_plugin::{
    PROTOCOL_VERSION,
    message::{
        ComposeMode, ComposeOption, ComposeRequest, ComposeResponse, ConfigEntry, ConfigResponse,
        ConfigsResponse, ConversationSummary, ConversationsResponse, CreatedResponse,
        DescribeResponse, DoneResponse, DraftResponse, ErrorResponse, EventsResponse, HostToPlugin,
        InitMessage, LockState, LogMessage, OutputFormat as PluginOutputFormat, PathsInfo,
        PluginToHost, QueryCompleteResponse, QueryRequest, SetTitleRequest, WorkspaceInfo,
        WriteDraftRequest,
    },
};
use jp_printer::{OutputFormat, Printer};
use jp_storage::backend::FsStorageBackend;
use jp_workspace::{ConversationLock, LockResult, Workspace, session::Session};
use relative_path::RelativePath;
use serde_json::Value;
use sha2::{Digest as _, Sha256};
use tokio::sync::mpsc;
use tracing::{debug, error, info, trace, warn};

use super::registry;
use crate::{
    Ctx, KeyValueOrPath, cmd,
    cmd::query::{PendingStreamTrim, TurnInputs},
    config_pipeline::build_partial_over,
    editor::report_editor_failure,
    signals::SignalRouter,
};

/// Runs the prompts a plugin asks for.
///
/// Composition lives on this side of the protocol because the host owns both
/// ends of it: the plugin's stdin carries the protocol, so it has no terminal
/// to read keys from, and only the host knows which editor `Ctrl+X` opens.
///
/// Holds a handle on the printer rather than a borrow, so the message loop it
/// belongs to can take the context mutably.
pub(crate) struct Composer {
    printer: Arc<Printer>,
    editor: Option<Arc<dyn EditorBackend>>,
    is_tty: bool,
}

impl Composer {
    /// Collect what the request asks for, or nothing if the user declines.
    fn compose(&self, request: &ComposeRequest) -> ComposeResponse {
        let mut response = ComposeResponse {
            id: request.id.clone(),
            text: None,
            values: vec![],
        };

        if !self.is_tty {
            debug!("Plugin asked to compose without a terminal to ask on.");
            return response;
        }

        match &request.mode {
            ComposeMode::MultiSelect { options } => {
                response.values = self.ask_many(request, options);
            }
            _ => response.text = self.ask(request),
        }

        response
    }

    /// Pick any number of the offered options.
    fn ask_many(&self, request: &ComposeRequest, options: &[ComposeOption]) -> Vec<String> {
        if options.is_empty() {
            return vec![];
        }

        let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
        let mut writer = self.printer.prompt_writer();
        let mut prompt = inquire::MultiSelect::new(&request.message, labels);
        if let Some(help) = &request.help {
            prompt = prompt.with_help_message(help);
        }

        let Ok(chosen) = prompt.raw_prompt_with_writer(&mut writer) else {
            return vec![];
        };

        chosen
            .into_iter()
            .filter_map(|item| options.get(item.index).map(|o| o.value.clone()))
            .collect()
    }

    fn ask(&self, request: &ComposeRequest) -> Option<String> {
        match &request.mode {
            ComposeMode::Line { default } => {
                let mut writer = self.printer.prompt_writer();
                let mut prompt = inquire::Text::new(&request.message);
                if let Some(default) = default {
                    prompt = prompt.with_initial_value(default);
                }
                if let Some(help) = &request.help {
                    prompt = prompt.with_help_message(help);
                }

                prompt.prompt_with_writer(&mut writer).ok()
            }
            // Answered by `ask_many`, which `compose` routes to first.
            ComposeMode::MultiSelect { .. } => None,
            ComposeMode::Buffer { initial_text } => {
                self.buffer(request, initial_text.as_deref().unwrap_or_default())
            }
            ComposeMode::Select { options, default } => {
                if options.is_empty() {
                    return None;
                }

                let labels: Vec<&str> = options.iter().map(|o| o.label.as_str()).collect();
                let start = default
                    .as_ref()
                    .and_then(|value| options.iter().position(|o| &o.value == value))
                    .unwrap_or(0);

                let mut writer = self.printer.prompt_writer();
                let mut prompt =
                    inquire::Select::new(&request.message, labels).with_starting_cursor(start);
                if let Some(help) = &request.help {
                    prompt = prompt.with_help_message(help);
                }

                let chosen = prompt.raw_prompt_with_writer(&mut writer).ok()?;

                options.get(chosen.index).map(|o| o.value.clone())
            }
        }
    }

    /// The multi-line widget, looping so the editor escape returns to it.
    fn buffer(&self, request: &ComposeRequest, initial: &str) -> Option<String> {
        let mut buffer = initial.to_owned();

        loop {
            let mut reply = InlineReply::new(request.message.as_str())
                .with_initial_text(buffer.as_str())
                .with_editor_escape(self.editor.is_some());
            if let Some(help) = &request.help {
                reply = reply.with_help_message(help.as_str());
            }

            match reply.prompt(Box::new(self.printer.owned_prompt_writer())) {
                Ok(ReplyOutcome::Submit(text)) => return Some(text),
                Ok(ReplyOutcome::Cancelled) => return None,
                Ok(ReplyOutcome::OpenEditor { current_text }) => {
                    // Whatever was typed before `Ctrl+X` seeds the editor, and
                    // whatever comes back seeds the widget again.
                    buffer = current_text;
                    let Some(editor) = self.editor.as_ref() else {
                        continue;
                    };
                    match editor.edit_text(&buffer) {
                        Ok((EditOutcome::Saved, edited)) => buffer = edited,
                        Ok((EditOutcome::Cancelled, _)) => {}
                        Err(error) => report_editor_failure(
                            &self.printer,
                            &error,
                            "Continuing with the inline editor.",
                        ),
                    }
                }
                Err(error) => {
                    warn!(%error, "Inline composition failed.");
                    return None;
                }
            }
        }
    }
}

/// Run a plugin binary, handling the full protocol lifecycle.
///
/// `binary` is the path to the plugin executable.
/// `args` are the remaining CLI arguments to forward.
pub(crate) async fn run_plugin(
    name: &str,
    binary: &Utf8Path,
    args: &[String],
    ctx: &mut Ctx,
) -> Result<(), cmd::Error> {
    let config = ctx.config();
    let log_level = ctx.term.args.verbose;

    // Owned before the workspace is borrowed mutably: both read through `&self`,
    // so they would otherwise hold a borrow of all of `ctx`.
    let storage_path = ctx.storage_path().map(ToOwned::to_owned);
    let user_storage_path = ctx.user_storage_path().map(ToOwned::to_owned);

    let composer = Composer {
        printer: ctx.printer.clone(),
        editor: crate::editor::build_editor_backend(&config.editor),
        is_tty: ctx.term.is_tty,
    };

    let config_json = serde_json::to_value(config.as_ref().to_partial())
        .map_err(|e| cmd::Error::from(format!("failed to serialize config: {e}")))?;

    let options: serde_json::Map<String, Value> = config
        .plugins
        .command
        .get(name)
        .and_then(|c| c.options.as_ref())
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let storage_path = storage_path.ok_or("workspace has no storage configured")?;

    let init = HostToPlugin::Init(InitMessage {
        version: PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: ctx.workspace.root().to_owned(),
            storage: storage_path.clone(),
            id: ctx.workspace.id().to_string(),
        },
        paths: well_known_paths(user_storage_path.as_deref()),
        config: config_json.clone(),
        options,
        args: args.to_vec(),
        log_level,
        output_format: output_format(ctx.printer.format()),
    });

    let PluginProcess {
        mut child,
        stdin,
        stdout,
        stderr_handle,
    } = spawn_plugin(binary)?;

    // Shutdown thread: sends `Shutdown` directly to the plugin's stdin when
    // an interrupt or a graceful shutdown request arrives. If the plugin
    // doesn't exit within the grace period, sends SIGKILL.
    //
    // The guard drops when this function returns; the thread then sees the
    // notification channel close and exits.
    let (_interrupt_guard, mut interrupt_rx) = ctx.signals.push_handler();
    let shutdown_token = ctx.signals.shutdown_token();
    let shutdown_sent = Arc::new(AtomicBool::new(false));
    let shutdown_writer = stdin.clone();
    let shutdown_flag = shutdown_sent.clone();
    let child_id = child.id();
    let shutdown_handle = thread::spawn(move || {
        let interrupted = futures::executor::block_on(async {
            tokio::select! {
                notified = interrupt_rx.recv() => notified.is_some(),
                () = shutdown_token.cancelled() => true,
            }
        });

        // The plugin run completed and deregistered its handler.
        if !interrupted {
            return;
        }

        stop_plugin(&shutdown_writer, &shutdown_flag, child_id);
    });

    // Send init.
    {
        let mut writer = stdin.lock().expect("stdin lock poisoned");
        write_message(&mut *writer, &init)
            .map_err(|e| cmd::Error::from(format!("failed to send init: {e}")))?;
    }

    // Read on a thread of its own, so a turn awaiting the provider cannot stop
    // the host from noticing what the plugin says next.
    let (mut requests, reader_thread) = spawn_reader(stdout);

    let result = message_loop(
        &mut requests,
        &stdin,
        ctx,
        &config_json,
        &shutdown_sent,
        &composer,
    )
    .await;

    // Always clean up, even on error.
    drop(child.wait());
    drop(stderr_handle.join());
    drop(reader_thread);
    drop(shutdown_handle);

    result
}

/// How long a plugin gets to exit on its own after being told to stop.
const SHUTDOWN_GRACE_MS: u64 = 5_000;

/// A spawned plugin process and its wired-up pipes.
struct PluginProcess {
    child: Child,

    /// Shared, because the shutdown thread writes to it as well as the message
    /// loop.
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: ChildStdout,

    /// Joins when the plugin's stderr closes.
    stderr_handle: thread::JoinHandle<()>,
}

/// Spawn the plugin, wiring its three pipes.
///
/// Its stderr is forwarded to tracing from a thread, so a plugin that logs
/// heavily cannot fill the pipe and block on a write nobody is draining.
fn spawn_plugin(binary: &Utf8Path) -> Result<PluginProcess, cmd::Error> {
    debug!(%binary, "Spawning plugin.");

    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Prevent the child from receiving SIGINT/SIGTERM directly. The host
    // sends `Shutdown` over the protocol instead, giving the plugin a
    // chance to exit gracefully.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| cmd::Error::from(format!("failed to spawn plugin: {e}")))?;

    let child_stdin = child.stdin.take().expect("stdin piped");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");

    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            match line {
                Ok(line) => trace!(target: "plugin::stderr", "{}", line),
                Err(e) => {
                    warn!("Error reading plugin stderr: {e}");
                    break;
                }
            }
        }
    });

    Ok(PluginProcess {
        child,
        stdin: Arc::new(Mutex::new(child_stdin)),
        stdout,
        stderr_handle,
    })
}

/// Ask the plugin to stop, and kill it if it will not.
///
/// `shutdown_sent` is raised once the request is out, so a stdout that closes
/// without an `exit` is read as the plugin obeying rather than as a crash.
/// It is raised after the write for that reason: before it, a closed stdout
/// still means something went wrong.
fn stop_plugin(stdin: &Mutex<impl Write>, shutdown_sent: &AtomicBool, child_id: u32) {
    if let Ok(mut writer) = stdin.lock() {
        drop(write_message(&mut *writer, &HostToPlugin::Shutdown));
    }
    shutdown_sent.store(true, Ordering::Release);

    // Polled in short intervals so a prompt exit doesn't hold up cleanup.
    let interval = std::time::Duration::from_millis(100);
    for _ in 0..(SHUTDOWN_GRACE_MS / 100) {
        thread::sleep(interval);
        if !is_process_alive(child_id) {
            return;
        }
    }

    kill_child(child_id);
}

/// The host's output format, in the protocol's vocabulary.
///
/// Two enums rather than one shared type: the protocol should not depend on a
/// particular renderer, so it carries its own.
fn output_format(format: OutputFormat) -> PluginOutputFormat {
    match format {
        OutputFormat::Text => PluginOutputFormat::Text,
        OutputFormat::TextPretty => PluginOutputFormat::TextPretty,
        OutputFormat::Json => PluginOutputFormat::Json,
        OutputFormat::JsonPretty => PluginOutputFormat::JsonPretty,
    }
}

/// The JP directories a plugin is told about, so it needs no platform logic of
/// its own.
fn well_known_paths(user_storage_path: Option<&Utf8Path>) -> PathsInfo {
    let home = std::env::home_dir().and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok());

    PathsInfo {
        user_data: jp_workspace::user_data_dir().ok(),
        user_config: jp_config::fs::user_global_config_dir(home.as_deref()),
        user_workspace: user_storage_path.map(ToOwned::to_owned),
    }
}

/// Read plugin messages on a dedicated thread, forwarding each line.
///
/// The channel closes when the plugin's stdout does, which ends the message
/// loop.
/// A full channel blocks the reader rather than dropping messages: the plugin
/// is waiting on replies to most of what it sends, so losing one would hang it.
fn spawn_reader(
    stdout: impl std::io::Read + Send + 'static,
) -> (mpsc::Receiver<String>, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel(64);

    let handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    warn!(%error, "Error reading from plugin.");
                    break;
                }
            }
        }
    });

    (rx, handle)
}

/// The main message loop: reads plugin requests and sends responses.
///
/// Async because a `query` runs a turn, which takes as long as the assistant
/// needs.
/// The turn goes to a task of its own, and this keeps answering reads while it
/// runs.
async fn message_loop(
    requests: &mut mpsc::Receiver<String>,
    stdin: &Arc<Mutex<ChildStdin>>,
    ctx: &mut Ctx,
    config_json: &Value,
    shutdown_sent: &AtomicBool,
    composer: &Composer,
) -> Result<(), cmd::Error> {
    while let Some(line) = requests.recv().await {
        if line.trim().is_empty() {
            continue;
        }

        let msg: PluginToHost = serde_json::from_str(&line)
            .map_err(|e| cmd::Error::from(format!("invalid plugin message: {e}: {line}")))?;

        trace!(?msg, "Received plugin message.");

        // Both of these block for as long as a person or a provider takes, so
        // they run before the lock is taken: the shutdown thread needs that same
        // lock to deliver `Shutdown` if the user interrupts partway.
        match msg {
            PluginToHost::Compose(request) => {
                let response = composer.compose(&request);
                let mut writer = stdin.lock().expect("stdin lock poisoned");
                write_message(&mut *writer, &HostToPlugin::Composed(response)).map_err(|e| {
                    cmd::Error::from(format!("failed to answer a compose request: {e}"))
                })?;
            }

            PluginToHost::Query(request) => {
                // `None` means the turn is running and will answer for itself.
                if let Some(response) = run_query(ctx, request, stdin).await {
                    let mut writer = stdin.lock().expect("stdin lock poisoned");
                    write_message(&mut *writer, &response)
                        .map_err(|e| cmd::Error::from(format!("failed to answer a query: {e}")))?;
                }
            }

            msg => {
                let config = ctx.config();
                let fs_backend = ctx.fs_backend.clone();
                let session = ctx.session.clone();
                let signals = ctx.signals.clone();
                let mut writer = stdin.lock().expect("stdin lock poisoned");

                if handle_request(
                    msg,
                    &mut *writer,
                    &mut ctx.workspace,
                    config_json,
                    session.as_ref(),
                    fs_backend.as_deref(),
                    &config,
                    &signals,
                )? == Flow::Stop
                {
                    return Ok(());
                }
            }
        }
    }

    // Plugin's stdout closed without an `exit` message. If we sent a
    // shutdown, this is expected (the child exited after receiving it).
    if shutdown_sent.load(Ordering::Acquire) {
        debug!("Plugin exited after shutdown.");
        return Ok(());
    }

    error!("Plugin exited without sending exit message.");
    Err(cmd::Error::from((
        1u8,
        "plugin exited unexpectedly without sending exit message",
    )))
}

/// Run a turn on the plugin's behalf.
///
/// The turn is the same one `jp query` runs: it locks the conversation, calls
/// the provider, executes tools, and persists the events.
///
/// `None` means the turn is under way and will send its own reply, correlated
/// by the request's id.
/// Anything returned is a failure that happened before the turn started.
async fn run_query(
    ctx: &mut Ctx,
    request: QueryRequest,
    stdin: &Arc<Mutex<ChildStdin>>,
) -> Option<HostToPlugin> {
    let reply_id = request.id.clone();
    let failed = |message: String| Some(query_error(reply_id.clone(), message));

    if request.content.trim().is_empty() {
        return failed("query content is empty".to_owned());
    }

    let new = request.new;
    let lock = match lock_for_query(ctx, &request) {
        Ok(lock) => lock,
        Err(error) => return failed(error),
    };

    // The turn runs under the conversation's own config, not the host's.
    //
    // A host resolved its config once at startup with no conversation in view, so
    // its persona, skills and enabled tools are whatever the bare workspace has.
    // The conversation carries all of that in its stored deltas, and running a
    // turn under anything else silently answers with the wrong model and no
    // tools.
    let config = match conversation_config(ctx, &lock, &request.cfg) {
        Ok(mut config) => {
            // A delegated turn has no terminal, so nothing can answer a prompt.
            //
            // An interrupt runs the same escalation as Ctrl-C, and the default
            // streaming action is to show the interrupt menu. With no keyboard
            // attached that menu blocks on a read nobody can satisfy: the turn
            // keeps the conversation locked, the next request is refused as
            // already-locked, and the host's terminal sits at a prompt meant for
            // someone who is elsewhere.
            //
            // Stopping is the only interpretation available here, so it is the
            // configured one. The reply and abort variants need a person.
            config.interrupt.streaming.action = StreamingInterruptAction::Stop;
            config.interrupt.tool_call.action = ToolInterruptAction::Stop;
            Arc::new(config)
        }
        Err(error) => return failed(error),
    };

    debug!(
        conversation = %lock.id(),
        model = %config.assistant.model.id.resolved(),
        "Running a delegated query.",
    );

    // Swapped around collecting only, because that is the part that reads the
    // context. The turn itself carries the config it was given.
    let host_config = ctx.swap_config(Arc::clone(&config));
    let prepared = prepare_turn(ctx, config, &lock, request.content).await;
    ctx.swap_config(host_config);

    let (inputs, stream) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => return failed(error.to_string()),
    };

    // Read from the lock, not the request: a new conversation was named by the
    // host, and the plugin has no other way to learn its id.
    let conversation = lock.id().to_string();

    // Answered before the turn, because a caller that only needs somewhere to
    // send the user cannot wait minutes to find out where that is.
    if new {
        let mut writer = stdin.lock().expect("stdin lock poisoned");
        drop(write_message(
            &mut *writer,
            &HostToPlugin::Created(CreatedResponse {
                id: reply_id.clone(),
                conversation: conversation.clone(),
            }),
        ));
    }

    // Hand the turn to its own task. It owns everything it needs and the lock owns
    // itself, so nothing here is borrowed for the minutes a turn can take, which
    // is what keeps the message loop answering reads while it runs.
    let stdin = Arc::clone(stdin);

    tokio::spawn(async move {
        let outcome = inputs.run(&lock, stream).await;

        // Reported through tracing rather than to the terminal. These are facts
        // about the host, not content: the turn's output belongs to the
        // conversation, which is where whoever asked for it is reading.
        let reply = match outcome {
            Ok(()) => {
                info!(%conversation, "A delegated turn finished.");
                HostToPlugin::QueryComplete(QueryCompleteResponse {
                    id: reply_id,
                    conversation,
                })
            }
            Err(error) => {
                // The full chain, not the outermost label: `cmd::Error` renders as
                // "LLM error" and everything that matters hangs off its sources.
                let detail = error_chain(&error);
                warn!(%conversation, %detail, "A delegated turn failed.");
                query_error(reply_id, detail)
            }
        };

        let mut writer = stdin.lock().expect("stdin lock poisoned");
        drop(write_message(&mut *writer, &reply));
    });

    None
}

/// Take exclusive hold of the conversation a query names, creating it if asked.
///
/// The lock comes before anything else a turn needs: it is the turn's proof of
/// exclusive access, and it owns what it needs, which is what lets the turn run
/// away from the message loop.
fn lock_for_query(ctx: &mut Ctx, request: &QueryRequest) -> Result<ConversationLock, String> {
    if request.new {
        // Resolved before the conversation exists, so a name that does not resolve
        // leaves nothing behind to clean up.
        let config = new_conversation_config(ctx, &request.cfg)?;

        let conversation = jp_conversation::Conversation {
            title: request.title.clone().filter(|t| !t.trim().is_empty()),
            ..jp_conversation::Conversation::default()
        };

        return ctx
            .workspace
            .create_and_lock_conversation(conversation, config, ctx.session.as_ref())
            .map_err(|error| format!("failed to create the conversation: {error}"));
    }

    let id = parse_conversation_id(&request.conversation)?;

    let handle = ctx
        .workspace
        .acquire_conversation(&id)
        .map_err(|error| format!("conversation {}: {error}", request.conversation))?;

    match ctx
        .workspace
        .lock_conversation(handle, ctx.session.as_ref())
    {
        Ok(LockResult::Acquired(lock)) => Ok(lock),
        Ok(LockResult::AlreadyLocked(_)) => {
            Err("another process is working on this conversation".to_owned())
        }
        Err(error) => Err(format!("failed to lock the conversation: {error}")),
    }
}

/// The configuration an existing conversation's next turn runs under.
///
/// The conversation's own configuration with any named `cfg` layered over it.
/// With nothing named, this is what the stream already resolves to.
///
/// The base layer is the one frozen when the conversation was created, so edits
/// to config files since then are not picked up.
/// Layering stored deltas over freshly read files needs the config pipeline,
/// which belongs with the caller that owns startup.
fn conversation_config(
    ctx: &Ctx,
    lock: &ConversationLock,
    cfg: &[String],
) -> Result<AppConfig, String> {
    let stored = lock
        .events()
        .config()
        .map_err(|error| format!("the conversation's config is invalid: {error}"))?;

    if cfg.is_empty() {
        return Ok(stored);
    }

    let args = cfg
        .iter()
        .map(|arg| arg.parse::<KeyValueOrPath>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("invalid configuration argument: {error}"))?;

    let partial = build_partial_over(
        stored.to_partial(),
        &args,
        Some(&ctx.workspace),
        ctx.fs_backend.as_deref(),
    )
    .map_err(|error| error.to_string())?;

    jp_config::util::build(partial)
        .map_err(|error| format!("the resolved configuration is invalid: {error}"))
}

/// The configuration a conversation created by a query starts from.
///
/// The files layer, every config file and its `extends` chain plus the
/// environment, read fresh, with the named configurations on top.
/// A host that stays up for hours should start a conversation from the
/// configuration as it is now, not as it was when the process booted.
fn new_conversation_config(ctx: &Ctx, cfg: &[String]) -> Result<Arc<AppConfig>, String> {
    let base = crate::load_base_partial(ctx.fs_backend.as_deref())
        .map_err(|error| format!("failed to read the workspace configuration: {error}"))?;

    let args = cfg
        .iter()
        .map(|arg| arg.parse::<KeyValueOrPath>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("invalid configuration argument: {error}"))?;

    let partial = build_partial_over(base, &args, Some(&ctx.workspace), ctx.fs_backend.as_deref())
        .map_err(|error| error.to_string())?;

    jp_config::util::build(partial)
        .map(Arc::new)
        .map_err(|error| format!("the resolved configuration is invalid: {error}"))
}

/// Collect what the turn needs, which is the only part that needs the context.
///
/// Fast on purpose: the message loop is blocked for exactly this long, and
/// every other request waits on it.
/// Starting the MCP servers is a spawn, not a wait; the waiting happens inside
/// [`TurnInputs::run`], on the turn's own task.
async fn prepare_turn(
    ctx: &mut Ctx,
    config: Arc<AppConfig>,
    lock: &ConversationLock,
    content: String,
) -> Result<(TurnInputs, ConversationStream), cmd::Error> {
    let mcp_servers = ctx.configure_active_mcp_servers().await?;

    let chat_request = ChatRequest {
        content,
        author: config.user.name.clone(),
        ..ChatRequest::default()
    };

    // The message has moved from draft to request, so the draft is done. Clearing
    // it here rather than from the caller gives it one owner: a client that
    // cleared its own draft would be racing its debounced save, and losing.
    if let Some(path) = draft_path(ctx.fs_backend.as_deref(), &ctx.workspace, &lock.id(), false)
        && path.exists()
        && let Err(error) = fs::remove_file(&path)
    {
        warn!(%error, "Failed to clear the query draft.");
    }

    // Sending a message counts as using the conversation.
    //
    // Only on this path. At a terminal, activating a conversation is a deliberate
    // act with its own command, and inferring it from a message would overwrite
    // what the user said. A caller reaching in over the protocol has no such act
    // to offer, so the last message is the best evidence of when the conversation
    // was last used, which is what orders the list.
    lock.as_mut()
        .update_metadata(|meta| meta.last_activated_at = chrono::Utc::now());

    // Recorded before the turn, so the conversation carries the change that
    // produced it rather than a turn whose configuration came from nowhere.
    match crate::cmd::query::get_config_delta_from_cli(&config, lock) {
        Ok(Some(delta)) => {
            lock.as_mut()
                .update_events(|events| events.add_config_delta(delta));
        }
        Ok(None) => {}
        Err(error) => return Err(cmd::Error::from(error.to_string())),
    }

    let stream = lock.events().clone();

    let inputs = TurnInputs::collect(
        ctx,
        config,
        lock,
        chat_request,
        PendingStreamTrim::default(),
        mcp_servers,
        // A sink, because this turn has no reader at this terminal. Nobody typed
        // it here: it was asked for from somewhere else, and its output belongs to
        // the conversation, which is where whoever asked is reading. Rendering it
        // here would also braid two concurrent turns into one stream with no way
        // to tell them apart.
        Arc::new(Printer::sink()),
    )
    .await?;

    Ok((inputs, stream))
}

/// Flatten an error and its sources into one line.
///
/// JP's error types label a category and carry the cause underneath, so the
/// outermost message alone says "LLM error" where the source says what actually
/// went wrong.
/// A reader across a protocol has no way to ask for the rest, so send all of
/// it.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut out = error.to_string();
    let mut source = error.source();

    while let Some(cause) = source {
        let text = cause.to_string();
        // Wrappers often restate their source; saying it twice helps nobody.
        if !out.contains(&text) {
            let _ = write!(out, ": {text}");
        }
        source = cause.source();
    }

    out
}

/// An error response to a `query` request.
fn query_error(id: Option<String>, message: String) -> HostToPlugin {
    HostToPlugin::Error(ErrorResponse {
        id,
        request: Some("query".to_owned()),
        message,
    })
}

/// Whether the message loop carries on after a request.
#[derive(Debug, PartialEq)]
enum Flow {
    Continue,
    Stop,
}

/// Answer one request from the plugin.
///
/// Runs with the writer lock held, so everything here has to be quick: anything
/// that blocks on the user is answered by the caller, before the lock is taken.
fn handle_request(
    msg: PluginToHost,
    writer: &mut impl Write,
    workspace: &mut Workspace,
    config_json: &Value,
    session: Option<&Session>,
    fs_backend: Option<&FsStorageBackend>,
    config: &AppConfig,
    signals: &SignalRouter,
) -> Result<Flow, cmd::Error> {
    match msg {
        PluginToHost::Ready(ready) => {
            // The plugin states what it needs, so a mismatch is caught here
            // rather than several messages later, when the host hits something it
            // cannot parse and the plugin blocks on the reply.
            if ready.protocol > PROTOCOL_VERSION {
                return Err(cmd::Error::from(format!(
                    "this plugin needs protocol {}, and this `jp` speaks {PROTOCOL_VERSION}. \
                     Reinstall the two together.",
                    ready.protocol,
                )));
            }
            debug!(protocol = ready.protocol, "Plugin signaled ready.");
        }

        PluginToHost::ListConversations(req) => {
            refresh_conversations(workspace);
            let response = handle_list_conversations(workspace, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::ReadEvents(req) => {
            refresh_conversations(workspace);
            let response = handle_read_events(workspace, &req.conversation, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::ReadConfig(req) => {
            let response = handle_read_config(config_json, req.path, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::ArchiveConversation(req) => {
            let response = handle_archive(workspace, session, &req.conversation, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::SetTitle(req) => {
            let response = handle_set_title(workspace, session, req);
            write_message(writer, &response)?;
        }

        PluginToHost::ReadDraft(req) => {
            let response = handle_read_draft(fs_backend, workspace, &req.conversation, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::WriteDraft(req) => {
            let response = handle_write_draft(fs_backend, workspace, req);
            write_message(writer, &response)?;
        }

        PluginToHost::Interrupt(req) => {
            // Aimed at the named conversation, not at whatever is topmost.
            //
            // Several turns can be running at once, and the request already said
            // which one it means. Falling back to the untargeted path would stop
            // an arbitrary other turn, which is worse than stopping nothing.
            //
            // Nothing to answer: what the interrupt did lands in the
            // conversation, and the turn's own outcome is still the reply to its
            // `query`.
            let reached = parse_conversation_id(&req.conversation)
                .is_ok_and(|id| signals.interrupt_scope(id));

            debug!(
                conversation = %req.conversation,
                reached,
                "Interrupting on a plugin's behalf."
            );
        }

        PluginToHost::ListConfigs(req) => {
            let response = handle_list_configs(config, workspace, fs_backend, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::Print(print) => {
            // In Phase 1, write to stdout directly. Full printer
            // integration comes later when we thread through &Printer.
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            drop(handle.write_all(print.text.as_bytes()));
            drop(handle.flush());
        }

        PluginToHost::Log(log) => {
            emit_log(&log);
        }

        PluginToHost::Describe(_) => {
            debug!("Ignoring describe in message loop.");
        }

        // Answered by the caller, before the lock this runs under is taken.
        PluginToHost::Compose(_) | PluginToHost::Query(_) => {
            unreachable!("answered before the lock")
        }

        PluginToHost::Exit(exit) => {
            debug!(code = exit.code, "Plugin exited.");
            if exit.code != 0 {
                return match exit.reason {
                    Some(reason) => Err(cmd::Error::from((exit.code, reason))),
                    None => Err(cmd::Error::from(exit.code)),
                };
            }
            return Ok(Flow::Stop);
        }
    }

    Ok(Flow::Continue)
}

/// An error against a request that only reports whether it worked.
fn action_failed(id: Option<String>, request: &str, message: String) -> HostToPlugin {
    HostToPlugin::Error(ErrorResponse {
        id,
        request: Some(request.to_owned()),
        message,
    })
}

/// Take the lock a mutation needs, or say why it could not be had.
fn lock_for_action(
    workspace: &Workspace,
    session: Option<&Session>,
    conversation: &str,
) -> Result<ConversationLock, String> {
    let id = parse_conversation_id(conversation)?;

    let handle = workspace
        .acquire_conversation(&id)
        .map_err(|error| format!("conversation {conversation}: {error}"))?;

    match workspace.lock_conversation(handle, session) {
        Ok(LockResult::Acquired(lock)) => Ok(lock),
        Ok(LockResult::AlreadyLocked(_)) => {
            Err("another process is working on this conversation".to_owned())
        }
        Err(error) => Err(format!("failed to lock the conversation: {error}")),
    }
}

/// Move a conversation to the archive.
///
/// Held under a lock like any other mutation: archiving moves the conversation
/// on disk, and doing that under a running turn would pull the files out from
/// beneath it.
fn handle_archive(
    workspace: &mut Workspace,
    session: Option<&Session>,
    conversation: &str,
    req_id: Option<String>,
) -> HostToPlugin {
    let lock = match lock_for_action(workspace, session, conversation) {
        Ok(lock) => lock,
        Err(message) => return action_failed(req_id, "archive_conversation", message),
    };

    workspace.archive_conversation(lock.into_mut());

    HostToPlugin::Done(DoneResponse { id: req_id })
}

/// Rename a conversation.
///
/// An empty title clears it, which leaves the conversation eligible for a
/// generated one again rather than naming it the empty string.
fn handle_set_title(
    workspace: &Workspace,
    session: Option<&Session>,
    req: SetTitleRequest,
) -> HostToPlugin {
    let lock = match lock_for_action(workspace, session, &req.conversation) {
        Ok(lock) => lock,
        Err(message) => return action_failed(req.id, "set_title", message),
    };

    let title = req
        .title
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty());

    lock.as_mut().update_metadata(|meta| meta.title = title);

    // Written out when the lock drops, at the end of this function.
    HostToPlugin::Done(DoneResponse { id: req.id })
}

/// Read a conversation named by a plugin.
///
/// The canonical spelling is the one JP prints and a user can paste back into
/// it, `jp-c17000000000`.
/// Bare deciseconds are accepted as well, since that is what the wire carried
/// before the two agreed.
fn parse_conversation_id(conversation: &str) -> Result<ConversationId, String> {
    conversation
        .parse()
        .or_else(|_| ConversationId::try_from_deciseconds_str(conversation))
        .map_err(|error| format!("invalid conversation ID `{conversation}`: {error}"))
}

/// The query draft's fingerprint: a short hash of its content.
///
/// Content rather than modification time, so a rewrite with identical text is
/// not mistaken for someone else's edit.
fn draft_revision(content: &str) -> String {
    let digest = Sha256::digest(content.as_bytes());
    digest[..8].iter().fold(String::new(), |mut acc, byte| {
        let _ = write!(acc, "{byte:02x}");
        acc
    })
}

/// Where a conversation's query draft lives, if user-local storage is
/// configured.
///
/// The same file `jp query` seeds an editor from and writes on interrupt.
/// It is deliberately user-local and never projected into the workspace tree,
/// so a half-written message is not something a teammate can end up with.
///
/// With `create`, a path is derived for a conversation that has no directory
/// yet; without it, an absent directory means an absent draft.
fn draft_path(
    fs_backend: Option<&FsStorageBackend>,
    workspace: &Workspace,
    id: &ConversationId,
    create: bool,
) -> Option<Utf8PathBuf> {
    let fs = fs_backend?;

    if let Some(dir) = fs.find_user_local_conversation_dir(id) {
        return Some(dir.join(crate::editor::QUERY_FILENAME));
    }

    if !create {
        return None;
    }

    // No directory yet, so derive the one `jp query` would use, which puts the
    // conversation's title in the name.
    let title = workspace.acquire_conversation(id).ok().and_then(|handle| {
        workspace
            .metadata(&handle)
            .ok()
            .and_then(|meta| meta.title.clone())
    });

    Some(
        fs.build_conversation_dir(id, title.as_deref(), true)
            .join(crate::editor::QUERY_FILENAME),
    )
}

/// Read a conversation's query draft.
///
/// An absent draft is not an error: most conversations do not have one, and the
/// answer is an empty draft with no revision.
fn handle_read_draft(
    fs_backend: Option<&FsStorageBackend>,
    workspace: &Workspace,
    conversation: &str,
    req_id: Option<String>,
) -> HostToPlugin {
    let id = match parse_conversation_id(conversation) {
        Ok(id) => id,
        Err(message) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_draft".to_owned()),
                message,
            });
        }
    };

    let content = draft_path(fs_backend, workspace, &id, false)
        .filter(|path| path.exists())
        .and_then(|path| fs::read_to_string(path).ok());

    HostToPlugin::Draft(DraftResponse {
        id: req_id,
        conversation: conversation.to_owned(),
        revision: content.as_deref().map(draft_revision),
        content: content.unwrap_or_default(),
        conflict: false,
    })
}

/// Replace a conversation's query draft.
///
/// The `revision` names the version the caller edited.
/// A draft that has moved on since is reported back rather than overwritten:
/// the other writer's text is exactly what the caller has not seen.
fn handle_write_draft(
    fs_backend: Option<&FsStorageBackend>,
    workspace: &Workspace,
    req: WriteDraftRequest,
) -> HostToPlugin {
    let failed = |message: String| {
        HostToPlugin::Error(ErrorResponse {
            id: req.id.clone(),
            request: Some("write_draft".to_owned()),
            message,
        })
    };

    let id = match parse_conversation_id(&req.conversation) {
        Ok(id) => id,
        Err(message) => return failed(message),
    };

    let Some(path) = draft_path(fs_backend, workspace, &id, true) else {
        return failed("this workspace has no user-local storage for drafts".to_owned());
    };

    let current = fs::read_to_string(&path).ok();
    let current_revision = current.as_deref().map(draft_revision);

    if current_revision != req.revision {
        return HostToPlugin::Draft(DraftResponse {
            id: req.id,
            conversation: req.conversation,
            content: current.unwrap_or_default(),
            revision: current_revision,
            conflict: true,
        });
    }

    // An empty draft is no draft: a blank file left behind would have the CLI
    // seed an editor with nothing and treat it as a recovery copy.
    if req.content.is_empty() {
        if path.exists()
            && let Err(error) = fs::remove_file(&path)
        {
            return failed(format!("failed to remove the draft: {error}"));
        }

        return HostToPlugin::Draft(DraftResponse {
            id: req.id,
            conversation: req.conversation,
            content: String::new(),
            revision: None,
            conflict: false,
        });
    }

    if let Some(parent) = path.parent()
        && let Err(error) = fs::create_dir_all(parent)
    {
        return failed(format!("failed to create the draft directory: {error}"));
    }

    if let Err(error) = fs::write(&path, &req.content) {
        return failed(format!("failed to write the draft: {error}"));
    }

    HostToPlugin::Draft(DraftResponse {
        id: req.id,
        conversation: req.conversation,
        revision: Some(draft_revision(&req.content)),
        content: req.content,
        conflict: false,
    })
}

/// Every configuration a query can name, from the same roots `--cfg` searches.
///
/// Roots are searched independently, and a segment present in more than one is
/// still one selectable thing, because naming it merges all of them.
/// Sorted and deduplicated for that reason.
fn handle_list_configs(
    config: &AppConfig,
    workspace: &Workspace,
    fs_backend: Option<&FsStorageBackend>,
    req_id: Option<String>,
) -> HostToPlugin {
    let mut roots: Vec<Utf8PathBuf> = Vec::new();

    if let Some(dir) = user_global_config_dir(None) {
        roots.push(dir);
    }
    roots.push(workspace.root().to_owned());
    if let Some(path) =
        fs_backend.and_then(|fs| fs.user_storage_with_path(RelativePath::new("config")))
    {
        roots.push(path);
    }

    let mut segments = BTreeSet::new();
    for root in &roots {
        for load_path in &config.config_load_paths {
            let Ok(dir) = Utf8PathBuf::try_from(load_path.to_path(root)) else {
                continue;
            };

            segments.extend(list_configs_in_load_path(&dir));
        }
    }

    let data = segments
        .into_iter()
        .map(|segment| {
            let (namespace, name) = match segment.rsplit_once('/') {
                Some((namespace, name)) => (namespace.to_owned(), name.to_owned()),
                None => (String::new(), segment.clone()),
            };

            ConfigEntry {
                segment,
                namespace,
                name,
            }
        })
        .collect();

    HostToPlugin::Configs(ConfigsResponse { id: req_id, data })
}

/// Re-read the conversation index, dropping what this process has cached.
///
/// A plugin host is long-lived and does not own the store: a `jp query` in
/// another terminal, or another plugin, appends events to a conversation whose
/// metadata and stream this process loaded once and would otherwise keep
/// serving forever.
/// Re-reading the index clears both caches, so the read that follows comes from
/// disk, and conversations created or deleted since startup appear and
/// disappear.
///
/// The scan is a directory listing per storage root; metadata and streams stay
/// lazy, so only what the request actually reads is loaded again.
///
/// Deliberately not a sanitize pass: that repairs a store on startup and can
/// move broken conversations aside, which is not a thing a page view should do.
fn refresh_conversations(workspace: &mut Workspace) {
    trace!("Re-reading the conversation index for a plugin request.");
    workspace.load_conversation_index();
}

fn handle_list_conversations(workspace: &Workspace, req_id: Option<String>) -> HostToPlugin {
    let data: Vec<ConversationSummary> = workspace
        .conversations()
        .map(|(id, meta)| ConversationSummary {
            id: id.to_string(),
            title: meta.title.clone(),
            last_activated_at: meta.last_activated_at,
            events_count: meta.events_count,
        })
        .collect();

    HostToPlugin::Conversations(ConversationsResponse { id: req_id, data })
}

fn handle_read_events(
    workspace: &Workspace,
    conversation_id: &str,
    req_id: Option<String>,
) -> HostToPlugin {
    let conv_id = match parse_conversation_id(conversation_id) {
        Ok(id) => id,
        Err(message) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_events".to_owned()),
                message,
            });
        }
    };

    let handle = match workspace.acquire_conversation(&conv_id) {
        Ok(h) => h,
        Err(e) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_events".to_owned()),
                message: format!("conversation not found: {e}"),
            });
        }
    };

    let events = match workspace.events(&handle) {
        Ok(stream) => stream,
        Err(e) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_events".to_owned()),
                message: format!("failed to load events: {e}"),
            });
        }
    };

    // Serialize events to JSON values, then decode base64-encoded storage
    // fields so plugins receive plain text.
    let (_, mut event_values) = match events.to_parts() {
        Ok(parts) => parts,
        Err(e) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_events".to_owned()),
                message: format!("failed to serialize events: {e}"),
            });
        }
    };

    for value in &mut event_values {
        jp_conversation::decode_event_value(value);
    }

    // Carried here so labelling one conversation doesn't cost a plugin the whole
    // conversation list, which reads every conversation's metadata.
    let title = workspace
        .metadata(&handle)
        .ok()
        .and_then(|meta| meta.title.clone());

    HostToPlugin::Events(EventsResponse {
        id: req_id,
        conversation: conversation_id.to_owned(),
        lock: lock_state(workspace, &conv_id),
        title,
        data: event_values,
    })
}

/// Whether a turn is running on a conversation, and whose it is.
///
/// Read from the lock rather than from the transcript: a stream ending in a
/// request looks identical whether a turn is running, was interrupted, or
/// failed outright.
///
/// A lock file outlives the process that wrote it when that process is killed,
/// so a recorded holder that is no longer alive counts as no holder at all.
/// Otherwise a crashed run would leave a conversation looking busy forever.
fn lock_state(workspace: &Workspace, id: &ConversationId) -> LockState {
    workspace
        .conversation_lock_info(id)
        .filter(|info| is_process_alive(info.pid))
        .map_or(LockState::Free, |info| {
            if info.pid == std::process::id() {
                LockState::Here
            } else {
                LockState::Elsewhere
            }
        })
}

fn handle_read_config(
    config_json: &Value,
    path: Option<String>,
    req_id: Option<String>,
) -> HostToPlugin {
    let data = match &path {
        Some(path) => {
            let mut current = config_json;
            for segment in path.split('.') {
                match current.get(segment) {
                    Some(v) => current = v,
                    None => {
                        return HostToPlugin::Error(ErrorResponse {
                            id: req_id,
                            request: Some("read_config".to_owned()),
                            message: format!("config path not found: {path}"),
                        });
                    }
                }
            }
            current.clone()
        }
        None => config_json.clone(),
    };

    HostToPlugin::Config(ConfigResponse {
        id: req_id,
        path,
        data,
    })
}

fn emit_log(log: &LogMessage) {
    match log.level.as_str() {
        "trace" => trace!(target: "plugin", message = %log.message),
        "debug" => debug!(target: "plugin", message = %log.message),
        "info" => tracing::info!(target: "plugin", message = %log.message),
        "warn" => warn!(target: "plugin", message = %log.message),
        "error" => error!(target: "plugin", message = %log.message),
        _ => {
            warn!(target: "plugin", level = %log.level, message = %log.message, "unknown log level");
        }
    }
}

fn write_message(writer: &mut impl Write, msg: &HostToPlugin) -> Result<(), cmd::Error> {
    let json = serde_json::to_string(msg)
        .map_err(|e| cmd::Error::from(format!("failed to serialize message: {e}")))?;
    writeln!(writer, "{json}")
        .map_err(|e| cmd::Error::from(format!("failed to write to plugin stdin: {e}")))?;
    writer
        .flush()
        .map_err(|e| cmd::Error::from(format!("failed to flush plugin stdin: {e}")))?;
    Ok(())
}

/// Check if a process is still alive by PID.
#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    // kill with signal 0 checks existence without sending a signal.
    unsafe { libc::kill(libc::pid_t::from(pid.cast_signed()), 0) == 0 }
}

#[cfg(windows)]
fn is_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut exit_code: u32 = 0;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        ok != 0 && (exit_code as i32) == STILL_ACTIVE
    }
}

/// Send SIGKILL to a child process by PID.
///
/// Used as a last resort when the plugin doesn't exit within the grace period
/// after receiving `Shutdown`.
#[cfg(unix)]
fn kill_child(pid: u32) {
    // SAFETY: We're sending a signal to a process we spawned.
    unsafe {
        libc::kill(libc::pid_t::from(pid.cast_signed()), libc::SIGKILL);
    }
    debug!(pid, "Sent SIGKILL to plugin after grace period.");
}

#[cfg(windows)]
fn kill_child(pid: u32) {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess},
    };

    // SAFETY: We're terminating a process we spawned.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if !handle.is_null() {
            TerminateProcess(handle, 1);
            CloseHandle(handle);
        }
    }
    debug!(pid, "Sent TerminateProcess to plugin after grace period.");
}

/// Search `$PATH` for a plugin binary matching the given subcommand segments.
///
/// For `["serve"]`, looks for `jp-serve`.
/// For `["conversation", "export"]`, looks for `jp-conversation-export`.
pub(crate) fn find_plugin_binary(segments: &[&str]) -> Option<Utf8PathBuf> {
    let name = format!("jp-{}", segments.join("-"));
    which::which(&name)
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
}

/// Find any existing plugin binary without downloading or prompting.
///
/// Checks the install directory first, then `$PATH`.
/// Used for non-mutating operations like help requests.
pub(crate) fn find_any_plugin_binary(name: &str) -> Option<Utf8PathBuf> {
    if let Some(path) = registry::find_installed(name) {
        return Some(path);
    }
    let segments: Vec<&str> = name.split('-').collect();
    find_plugin_binary(&segments)
}

/// Resolve a plugin binary through multiple sources:
///
/// 1. User-local install directory (previously installed plugins)
/// 2. Plugin registry (auto-install if official, prompt if third-party)
/// 3. `$PATH` (with approval check for unapproved plugins)
///
/// The `plugins_config` drives installation and execution policy.
/// Per-plugin settings override the defaults from the registry (official vs
/// third-party).
pub(crate) async fn resolve_plugin_binary(
    name: &str,
    plugins_config: &PluginsConfig,
    is_tty: bool,
) -> Result<Option<Utf8PathBuf>, cmd::Error> {
    let plugin_cfg = plugins_config.command.get(name);

    // Explicit deny in config.
    if plugin_cfg.is_some_and(|c| c.run == Some(RunPolicy::Deny)) {
        return Err(cmd::Error::from(format!(
            "plugin `{name}` is denied by configuration (plugins.command.{name}.run = \"deny\")"
        )));
    }

    // 1. Already installed locally.
    if let Some(path) = registry::find_installed(name) {
        debug!(name, %path, "Found installed plugin.");
        verify_checksum(name, &path, plugin_cfg)?;
        return Ok(Some(path));
    }

    // 2. Check registry.
    if let Some(path) = try_registry_install(name, plugins_config, is_tty).await? {
        return Ok(Some(path));
    }

    // 3. Check $PATH with run policy.
    let segments: Vec<&str> = name.split('-').collect();
    if let Some(path) = find_plugin_binary(&segments) {
        check_run_policy(name, &path, plugin_cfg, is_tty)?;
        return Ok(Some(path));
    }

    Ok(None)
}

/// Verify a binary's checksum against the config-pinned value, if any.
fn verify_checksum(
    name: &str,
    binary_path: &Utf8Path,
    plugin_cfg: Option<&CommandPluginConfig>,
) -> Result<(), cmd::Error> {
    let Some(checksum) = plugin_cfg.and_then(|c| c.checksum.as_ref()) else {
        return Ok(());
    };

    let actual = registry::sha256_file(binary_path)?;
    if actual != checksum.value {
        return Err(cmd::Error::from(format!(
            "plugin `{name}` binary checksum mismatch.\nexpected: {}\nactual:   {actual}\nThe \
             binary at {binary_path} has changed since it was pinned. Update \
             plugins.command.{name}.checksum.value in your config to accept the new binary.",
            checksum.value,
        )));
    }

    Ok(())
}

/// Try to install a plugin from the cached registry.
async fn try_registry_install(
    name: &str,
    plugins_config: &PluginsConfig,
    is_tty: bool,
) -> Result<Option<Utf8PathBuf>, cmd::Error> {
    let Some(reg) = registry::load_cached() else {
        return Ok(None);
    };

    // Find the registry entry whose `id` matches the requested name.
    // In Phase 5, this will use the command path (registry key) for
    // multi-segment routing. For now, we match on `id`.
    let Some(plugin) = reg.plugins.values().find(|p| p.id == name) else {
        return Ok(None);
    };

    // Only handle command plugins.
    let jp_plugin::registry::PluginKind::Command { ref binaries, .. } = plugin.kind else {
        return Ok(None);
    };

    let target = registry::current_target();
    let Some(binary_info) = binaries.get(&target) else {
        return Ok(None);
    };

    let id = &plugin.id;
    let plugin_cfg = plugins_config.command.get(id);

    // Check if auto-install is allowed.
    let auto_install = plugin_cfg
        .and_then(|c| c.install)
        .unwrap_or(plugins_config.auto_install);

    if !auto_install && !plugin.official {
        return Ok(None);
    }

    // Determine run policy: config > registry default.
    let run_policy = plugin_cfg
        .and_then(|c| c.run)
        .unwrap_or(if plugin.official {
            RunPolicy::Unattended
        } else {
            RunPolicy::Ask
        });

    match run_policy {
        RunPolicy::Deny => {
            return Err(cmd::Error::from(format!(
                "plugin `{id}` is denied by configuration"
            )));
        }
        RunPolicy::Ask => {
            if !is_tty {
                return Err(cmd::Error::from(format!(
                    "plugin `{id}` requires approval. Run `jp plugin install {id}` first, or set \
                     plugins.command.{id}.run = \"unattended\" in config."
                )));
            }

            let mut writer = std::io::stderr();
            drop(writeln!(
                writer,
                "  \u{2192} Plugin `{id}` found in registry."
            ));
            let options = vec![
                InlineOption::new('y', "install and run"),
                InlineOption::new('n', "cancel"),
            ];
            let answer = InlineSelect::new("Install and run it?", options)
                .prompt(&mut writer)
                .map_err(|e| cmd::Error::from(format!("prompt failed: {e}")))?;

            if answer != 'y' {
                return Err(cmd::Error::from("plugin execution cancelled"));
            }
        }
        RunPolicy::Unattended => {}
    }

    drop(writeln!(
        std::io::stderr(),
        "  \u{2192} Installing jp-{id} for {target}..."
    ));
    let client = reqwest::Client::new();
    let data = registry::download_and_verify(&client, binary_info).await?;

    let path = registry::install_binary(id, &data)?;
    drop(writeln!(
        std::io::stderr(),
        "  \u{2192} Installed to {path}",
    ));

    // Verify against pinned checksum if configured.
    verify_checksum(id, &path, plugin_cfg)?;

    Ok(Some(path))
}

/// Check run policy for a `$PATH`-discovered plugin.
fn check_run_policy(
    name: &str,
    binary_path: &Utf8Path,
    plugin_cfg: Option<&CommandPluginConfig>,
    is_tty: bool,
) -> Result<(), cmd::Error> {
    // Verify pinned checksum first.
    verify_checksum(name, binary_path, plugin_cfg)?;

    let run_policy = plugin_cfg.and_then(|c| c.run).unwrap_or(RunPolicy::Ask);

    match run_policy {
        RunPolicy::Unattended => Ok(()),
        RunPolicy::Deny => Err(cmd::Error::from(format!(
            "plugin `{name}` is denied by configuration"
        ))),
        RunPolicy::Ask => {
            if !is_tty {
                return Err(cmd::Error::from(format!(
                    "plugin `jp-{name}` found on $PATH but requires approval. Set \
                     plugins.command.{name}.run = \"unattended\" in config, or run `jp {name}` in \
                     a terminal."
                )));
            }

            // Check existing permanent approvals.
            if let Some(approvals) = registry::load_approvals()
                && let Some(approved) = approvals.approved.get(name)
                && approved.path == binary_path
                && registry::sha256_file(binary_path).is_ok_and(|sha| sha == approved.sha256)
            {
                debug!(name, %binary_path, "Plugin previously approved.");
                return Ok(());
            }

            let mut writer = std::io::stderr();
            drop(writeln!(
                writer,
                "  \u{2192} Found jp-{name} on $PATH ({binary_path})",
            ));
            let options = vec![
                InlineOption::new('y', "run this time"),
                InlineOption::new('Y', "run and remember permanently"),
                InlineOption::new('n', "deny"),
            ];
            let answer = InlineSelect::new("Run it?", options)
                .prompt(&mut writer)
                .map_err(|e| cmd::Error::from(format!("prompt failed: {e}")))?;

            match answer {
                'y' => Ok(()),
                'Y' => {
                    registry::save_approval(name, binary_path)?;
                    Ok(())
                }
                _ => Err(cmd::Error::from("plugin execution denied")),
            }
        }
    }
}

/// Send a `Describe` request to a plugin and return its metadata.
///
/// Spawns the binary, sends `{"type":"describe"}`, reads one response line, and
/// returns the parsed [`DescribeResponse`].
/// Returns `None` if the plugin doesn't support describe or fails to respond.
pub(crate) fn describe_plugin(binary: &Utf8Path) -> Option<DescribeResponse> {
    let mut child = Command::new(binary)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;

    let mut child_stdin = child.stdin.take()?;
    let child_stdout = child.stdout.take()?;

    // Send describe request.
    let json = serde_json::to_string(&HostToPlugin::Describe).ok()?;
    writeln!(child_stdin, "{json}").ok()?;
    child_stdin.flush().ok()?;
    drop(child_stdin); // Signal no more messages.

    // Read one line response.
    let mut reader = BufReader::new(child_stdout);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    drop(child.wait());

    if line.trim().is_empty() {
        return None;
    }

    let msg: PluginToHost = serde_json::from_str(line.trim()).ok()?;
    match msg {
        PluginToHost::Describe(resp) => Some(resp),
        _ => None,
    }
}

/// Discover plugin binaries on `$PATH` and in the user-local install directory.
///
/// Returns `(subcommand_name, binary_path)` pairs, sorted by name.
/// For a binary named `jp-serve`, the subcommand name is `serve`.
/// Installed plugins take priority over `$PATH` duplicates.
pub(crate) fn discover_plugins() -> Vec<(String, Utf8PathBuf)> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let mut seen = HashSet::new();
    let mut plugins = Vec::new();

    // Scan install directory first so installed plugins take priority.
    if let Some(bin_dir) = registry::bin_dir() {
        scan_dir_for_plugins(&bin_dir, &mut seen, &mut plugins);
    }

    for dir in std::env::split_paths(&path_var) {
        let Some(dir) = Utf8Path::from_path(&dir) else {
            continue;
        };

        scan_dir_for_plugins(dir, &mut seen, &mut plugins);
    }

    plugins.sort_by(|a, b| a.0.cmp(&b.0));
    plugins
}

fn scan_dir_for_plugins(
    dir: &Utf8Path,
    seen: &mut HashSet<String>,
    plugins: &mut Vec<(String, Utf8PathBuf)>,
) {
    let Ok(entries) = dir.read_dir_utf8() else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(subcommand) = name.strip_prefix("jp-") else {
            continue;
        };

        // On Windows, strip the .exe extension.
        #[cfg(windows)]
        let subcommand = subcommand.strip_suffix(".exe").unwrap_or(subcommand);

        // On Unix, skip non-executable files.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }

        if seen.insert(subcommand.to_owned()) {
            plugins.push((subcommand.to_owned(), entry.into_path()));
        }
    }
}

/// Show a plugin's help text via the `Describe` protocol.
pub(crate) fn show_plugin_help(binary: &Utf8Path) -> cmd::Output {
    match describe_plugin(binary) {
        Some(desc) => {
            let mut out = std::io::stdout().lock();
            if let Some(help) = &desc.help {
                drop(writeln!(out, "{help}"));
            } else {
                drop(writeln!(out, "{}: {}", desc.name, desc.description));
            }
            Ok(())
        }
        None => Err(cmd::Error::from("plugin does not support describe")),
    }
}

/// Produce a clap-formatted error for an unknown subcommand.
///
/// Uses `Command::error()` to get clap's standard error chrome (colored
/// `error:` prefix, usage line, help hint).
/// The message includes our plugin-specific context.
/// Returns exit code 2 (clap's convention for usage errors) with no message,
/// since the output was already written.
fn unknown_subcommand_error(name: &str) -> cmd::Error {
    use clap::CommandFactory as _;

    let mut cmd = crate::Cli::command();
    let err = cmd.error(
        clap::error::ErrorKind::InvalidSubcommand,
        format!(
            "unrecognized subcommand '{name}'\n\n  No built-in command, registry plugin, or \
             `jp-{name}` binary found on $PATH."
        ),
    );
    drop(err.print());
    cmd::Error::from(2u8)
}

/// Dispatch an external plugin subcommand.
///
/// Resolves the plugin binary, then runs the protocol loop.
/// Called from `Commands::run()` after the normal startup flow.
pub(crate) async fn run_external(args: &[String], ctx: &mut Ctx) -> cmd::Output {
    let (subcommand, plugin_args) = args
        .split_first()
        .ok_or("no subcommand provided for plugin dispatch")?;

    // A bare `jp <plugin> --help` is answered from the plugin's self-description,
    // without downloading or approving anything.
    //
    // Help for something *within* the plugin (`jp <plugin> add --help`) is the
    // plugin's own to render, and only it knows its subcommands, so that goes
    // through normal dispatch below.
    let bare_help =
        !plugin_args.is_empty() && plugin_args.iter().all(|a| a == "-h" || a == "--help");
    if bare_help {
        let binary = find_any_plugin_binary(subcommand).ok_or_else(|| {
            cmd::Error::from(format!(
                "plugin `{subcommand}` not found. No installed plugin or `jp-{subcommand}` binary \
                 found on $PATH.",
            ))
        })?;
        return show_plugin_help(&binary);
    }

    let config = ctx.config();
    let Some(binary) = resolve_plugin_binary(subcommand, &config.plugins, ctx.term.is_tty).await?
    else {
        return Err(unknown_subcommand_error(subcommand));
    };

    debug!(%binary, subcommand, "Dispatching to plugin.");

    run_plugin(subcommand, &binary, plugin_args, ctx).await?;
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
