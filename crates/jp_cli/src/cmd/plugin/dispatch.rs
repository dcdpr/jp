//! Host-side plugin message loop.
//!
//! Spawns the plugin binary, sends `init`, and relays workspace queries until
//! the plugin sends `exit` or the process terminates.

use std::{
    collections::{BTreeSet, HashSet},
    fs,
    io::{self, BufRead, BufReader, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use jp_config::{
    AppConfig,
    plugins::{
        PluginsConfig,
        command::{CommandPluginConfig, RunPolicy},
    },
    util::list_configs_in_load_path,
};
use jp_conversation::ConversationId;
use jp_editor::{EditOutcome, EditorBackend};
use jp_inquire::{
    InlineOption, InlineSelect, ReplyEditMode, ReplyOutcome,
    prompt::{PromptBackend, TerminalPromptBackend},
};
use jp_plugin::{
    PROTOCOL_VERSION,
    message::{
        ComposeMode, ComposeOption, ComposeRequest, ComposeResponse, ConfigEntry, ConfigResponse,
        ConfigsResponse, ConversationSummary, ConversationsResponse, DescribeResponse,
        DoneResponse, DraftResponse, ErrorResponse, EventsResponse, HostToPlugin, InitMessage,
        LogMessage, PathsInfo, PluginToHost, SetTitleRequest, WorkspaceInfo, WriteDraftRequest,
    },
};
use jp_printer::Printer;
use jp_storage::backend::FsStorageBackend;
use jp_workspace::{ConversationLock, LockResult, Workspace, session::Session};
use serde_json::Value;
use tracing::{debug, error, trace, warn};

use super::registry;
use crate::{
    Ctx, cmd,
    cmd::query::interrupt::reply_edit_mode,
    config_pipeline::config_search_roots,
    editor::{draft_query_text, draft_revision, report_editor_failure},
};

/// Runs the prompts a plugin asks for.
///
/// Composition lives on this side of the protocol because the host owns both
/// ends of it: the plugin's stdin carries the protocol, so it has no terminal
/// to read keys from, and only the host knows which editor `Ctrl+X` opens.
pub(crate) struct Composer<'a> {
    printer: &'a Printer,

    /// Renders the widgets, rather than the composer building them.
    ///
    /// Everything the widget owns — its keybindings, and the hints that
    /// describe them — lives behind here, so a second caller cannot quietly
    /// ship a reply buffer with the wrong edit mode or no key hints at all.
    prompts: &'a dyn PromptBackend,

    editor: Option<Arc<dyn EditorBackend>>,
    edit_mode: ReplyEditMode,
    interactive: bool,
}

impl Composer<'_> {
    /// Collect what the request asks for, or nothing if the user declines.
    fn compose(&self, request: &ComposeRequest) -> ComposeResponse {
        let mut response = ComposeResponse {
            id: request.id.clone(),
            text: None,
            values: vec![],
        };

        if !self.interactive {
            debug!("Plugin asked to compose without a user to ask.");
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
            let outcome = self.prompts.inline_reply(
                request.message.as_str(),
                buffer.as_str(),
                self.edit_mode,
                self.editor.is_some(),
                request.help.as_deref(),
                Box::new(self.printer.owned_prompt_writer()),
            );

            match outcome {
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
                            self.printer,
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

/// Where a plugin run reads and writes.
///
/// Grouped because they arrive together and are all optional paths: as separate
/// parameters, two of them could be transposed at the call site and nothing
/// would say so.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PluginPaths<'a> {
    /// The bootstrap-resolved working directory for the child (RFD 087).
    ///
    /// `Some` when JP operates on a workspace other than the launch cwd's own,
    /// `None` to inherit the process cwd.
    pub(crate) child_cwd: Option<&'a Utf8Path>,

    /// The workspace's storage directory.
    pub(crate) storage: Option<&'a Utf8Path>,

    /// The user-local storage directory for this workspace, when there is one.
    pub(crate) user_storage: Option<&'a Utf8Path>,
}

/// The `init` message a plugin is greeted with, and the config it carries.
///
/// The config is returned alongside because the message loop answers
/// `read_config` from the same value, rather than serializing it twice.
fn init_message(
    name: &str,
    args: &[String],
    workspace: &Workspace,
    paths: PluginPaths<'_>,
    config: &Arc<AppConfig>,
    log_level: u8,
) -> Result<(HostToPlugin, Value), cmd::Error> {
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

    let storage = paths.storage.ok_or("workspace has no storage configured")?;

    let init = HostToPlugin::Init(InitMessage {
        version: PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: workspace.root().to_owned(),
            storage: storage.to_owned(),
            id: workspace.id().to_string(),
        },
        paths: well_known_paths(paths.user_storage),
        config: config_json.clone(),
        options,
        args: args.to_vec(),
        log_level,
    });

    Ok((init, config_json))
}

/// Run a plugin binary, handling the full protocol lifecycle.
///
/// `binary` is the path to the plugin executable.
/// `args` are the remaining CLI arguments to forward.
pub(crate) fn run_plugin(
    name: &str,
    binary: &Utf8Path,
    args: &[String],
    ctx: &mut Ctx,
) -> Result<(), cmd::Error> {
    let config = ctx.config();

    // Owned up front: each of these reads through `&self`, so holding one would
    // borrow all of `ctx` and leave the workspace unable to be borrowed mutably
    // for the mutations a plugin can ask for.
    let child_cwd = ctx.exec.child_cwd().map(ToOwned::to_owned);
    let storage = ctx.storage_path().map(ToOwned::to_owned);
    let user_storage = ctx.user_storage_path().map(ToOwned::to_owned);
    let fs_backend = ctx.fs_backend.clone();

    let paths = PluginPaths {
        child_cwd: child_cwd.as_deref(),
        storage: storage.as_deref(),
        user_storage: user_storage.as_deref(),
    };

    let (init, config_json) = init_message(
        name,
        args,
        &ctx.workspace,
        paths,
        &config,
        ctx.term.args.verbose,
    )?;

    let prompts = TerminalPromptBackend;
    let composer = Composer {
        printer: &ctx.printer,
        prompts: &prompts,
        editor: crate::editor::build_editor_backend(&config.editor),
        // The configured mode, as every other inline reply in the CLI uses.
        edit_mode: reply_edit_mode(config.editor.inline.edit_mode),
        interactive: ctx.term.interactive,
    };

    let PluginProcess {
        mut child,
        stdin,
        stdout,
        stderr_handle,
    } = spawn_plugin(binary, paths.child_cwd)?;

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
                // Asking the plugin to shut down acts on the press, so it
                // leaves the escalation ladder; a further press then reaches
                // the router with a fresh count.
                notified = interrupt_rx.recv() => notified.is_some_and(|notice| {
                    notice.handled();
                    true
                }),
                () = shutdown_token.cancelled() => true,
            }
        });

        // The plugin run completed and deregistered its handler.
        if !interrupted {
            return;
        }

        stop_plugin(
            &shutdown_writer,
            &shutdown_flag,
            child_id,
            Duration::from_secs(5),
        );
    });

    // Send init.
    {
        let mut writer = stdin.lock().expect("stdin lock poisoned");
        write_message(&mut *writer, &init)
            .map_err(|e| cmd::Error::from(format!("failed to send init: {e}")))?;
    }

    // Read messages from plugin.
    let reader = BufReader::new(stdout);
    let result = message_loop(
        reader,
        &stdin,
        &mut ctx.workspace,
        &config_json,
        &shutdown_sent,
        &composer,
        ctx.session.as_ref(),
        fs_backend.as_deref(),
        &config,
    );

    // A plugin that asked a question the host answered with an error is still
    // waiting for the reply. Nothing here closes its stdin — this scope holds a
    // handle and so does the shutdown thread — so waiting on it without saying
    // anything first is a wait for a process that has no reason to exit, and the
    // error above never reaches the caller.
    //
    // Short grace: unlike an interrupt, there is no work in flight worth letting
    // finish.
    if result.is_err() {
        stop_plugin(&stdin, &shutdown_sent, child_id, Duration::from_secs(1));
    }

    // Always clean up, even on error.
    drop(child.wait());
    drop(stderr_handle.join());
    drop(shutdown_handle);

    result
}

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
fn spawn_plugin(
    binary: &Utf8Path,
    child_cwd: Option<&Utf8Path>,
) -> Result<PluginProcess, cmd::Error> {
    debug!(%binary, "Spawning plugin.");

    let mut cmd = Command::new(binary);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // The root-as-working-directory invariant (RFD 087): when JP operates on
    // a workspace other than the launch cwd's own, plugins run as if launched
    // from the selected workspace root.
    if let Some(cwd) = child_cwd {
        cmd.current_dir(cwd);
    }

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

/// Ask a plugin to stop, and make sure it does.
///
/// Sends `Shutdown` over the protocol, gives the plugin `grace` to act on it,
/// and kills it if it doesn't.
/// `sent` records the request having been made, so the two callers don't both
/// make it.
/// It does not record the request arriving: a write to a plugin that has
/// already exited fails, and the flag is raised either way.
///
/// Killing rather than closing stdin: the handle is shared, so no single holder
/// can produce the EOF that would let a reading plugin notice on its own.
fn stop_plugin(stdin: &Mutex<impl Write>, sent: &AtomicBool, child_id: u32, grace: Duration) {
    if !sent.swap(true, Ordering::AcqRel)
        && let Ok(mut writer) = stdin.lock()
    {
        drop(write_message(&mut *writer, &HostToPlugin::Shutdown));
    }

    // Polled in short intervals, so a plugin that goes quietly doesn't hold up
    // cleanup for the whole grace period.
    let deadline = Instant::now() + grace;
    while Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
        if !is_process_alive(child_id) {
            return;
        }
    }

    kill_child(child_id);
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

/// The main message loop: reads plugin requests and sends responses.
fn message_loop(
    reader: BufReader<impl std::io::Read>,
    stdin: &Mutex<impl Write>,
    workspace: &mut Workspace,
    config_json: &Value,
    shutdown_sent: &AtomicBool,
    composer: &Composer<'_>,
    session: Option<&Session>,
    fs_backend: Option<&FsStorageBackend>,
    config: &AppConfig,
) -> Result<(), cmd::Error> {
    for line in reader.lines() {
        let line =
            line.map_err(|e| cmd::Error::from(format!("failed to read from plugin: {e}")))?;

        if line.trim().is_empty() {
            continue;
        }

        let msg: PluginToHost = serde_json::from_str(&line)
            .map_err(|e| cmd::Error::from(format!("invalid plugin message: {e}: {line}")))?;

        trace!(?msg, "Received plugin message.");

        // Composing blocks on the user, so it runs before the lock is taken:
        // the shutdown thread needs that same lock to deliver `Shutdown` if the
        // user interrupts mid-prompt.
        let msg = match msg {
            PluginToHost::Compose(request) => {
                let response = composer.compose(&request);
                let mut writer = stdin.lock().expect("stdin lock poisoned");
                write_message(&mut *writer, &HostToPlugin::Composed(response)).map_err(|e| {
                    cmd::Error::from(format!("failed to answer a compose request: {e}"))
                })?;
                continue;
            }
            other => other,
        };

        let mut writer = stdin.lock().expect("stdin lock poisoned");

        if handle_request(
            msg,
            &mut *writer,
            workspace,
            config_json,
            session,
            fs_backend,
            config,
        )? == Flow::Stop
        {
            return Ok(());
        }
    }

    // Plugin's stdout closed without an `exit` message. A shutdown request makes
    // that expected: the child exited rather than answering.
    //
    // The flag records the request being made rather than delivered, so a plugin
    // that had already exited when the request was written reaches here too, and
    // its exit is reported as clean rather than unexpected.
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
            let response = handle_list_conversations(workspace, req.id);
            write_message(writer, &response)?;
        }

        PluginToHost::ReadEvents(req) => {
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

        // Answered before the lock this runs under is taken.
        PluginToHost::Compose(_) => unreachable!("compose is answered before the lock"),

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

/// An error against a request that has no response payload of its own.
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

    if let Err(error) = workspace.archive_conversation(lock.into_mut()) {
        return action_failed(
            req_id,
            "archive_conversation",
            format!("failed to archive the conversation: {error}"),
        );
    }

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

    let mut conv = lock.into_mut();
    conv.update_metadata(|meta| meta.title = title);

    // Flushed here rather than left to the drop, which reports a failed write to
    // stderr and has no way to hand it back. `done` has to mean the title
    // reached the store.
    if let Err(error) = conv.flush() {
        return action_failed(
            req.id,
            "set_title",
            format!("failed to save the title: {error}"),
        );
    }

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

    // A backend with no user store resolves the conversations directory to the
    // workspace tree, which is the one place a draft may not be written.
    fs.user_storage_path()?;

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

/// Read the stored draft, or `None` when there is none.
///
/// Only an absent file counts as an absent draft.
/// Anything else — unreadable permissions, a failed read, bytes that are not
/// UTF-8 — is an error, so a draft the host cannot see is never reported as
/// one that isn't there.
fn read_draft_file(path: &Utf8Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read the draft: {error}")),
    }
}

/// Replace the stored draft, leaving the old text in place if the write fails.
///
/// The content goes to a temporary file beside the target and is renamed over
/// it, so a write that runs out of disk partway cannot leave a truncated draft:
/// the text is either replaced or untouched.
fn write_draft_file(path: &Utf8Path, content: &str) -> Result<(), String> {
    let dir = path.parent().unwrap_or_else(|| Utf8Path::new("."));

    let mut tmp = NamedUtf8TempFile::new_in(dir)
        .map_err(|error| format!("failed to open a temporary draft file: {error}"))?;

    tmp.write_all(content.as_bytes())
        .map_err(|error| format!("failed to write the draft: {error}"))?;

    tmp.persist(path)
        .map_err(|error| format!("failed to replace the draft: {error}"))?;

    Ok(())
}

/// Read a conversation's query draft.
///
/// An absent draft is not an error: most conversations do not have one, and the
/// answer is an empty draft with no revision.
/// The revision covers the stored file, of which the answer's content is the
/// query section.
fn handle_read_draft(
    fs_backend: Option<&FsStorageBackend>,
    workspace: &Workspace,
    conversation: &str,
    req_id: Option<String>,
) -> HostToPlugin {
    let failed = |message: String| {
        HostToPlugin::Error(ErrorResponse {
            id: req_id.clone(),
            request: Some("read_draft".to_owned()),
            message,
        })
    };

    let id = match parse_conversation_id(conversation) {
        Ok(id) => id,
        Err(message) => return failed(message),
    };

    let stored = match draft_path(fs_backend, workspace, &id, false) {
        Some(path) => match read_draft_file(&path) {
            Ok(stored) => stored,
            Err(message) => return failed(message),
        },
        None => None,
    };

    HostToPlugin::Draft(DraftResponse {
        id: req_id,
        conversation: conversation.to_owned(),
        revision: stored.as_deref().map(draft_revision),
        content: stored
            .as_deref()
            .map(draft_query_text)
            .unwrap_or_default()
            .to_owned(),
        conflict: false,
    })
}

/// Replace a conversation's query draft.
///
/// The `revision` names the version the caller edited.
/// A draft that has moved on since is reported back rather than overwritten:
/// the other writer's text is exactly what the caller has not seen.
///
/// A conversation that does not exist is an error, rather than a draft written
/// somewhere nothing will read it.
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

    // A conversation that is gone — archived, or removed — gets no draft
    // directory of its own: `jp query` never looks there, and unarchiving one
    // deletes whatever occupies the live path.
    //
    // Asked of the store rather than of the conversation index: the index is a
    // snapshot taken when the host started, and a host that stays up for hours
    // holds it while another process archives.
    if fs_backend.is_none_or(|fs| fs.find_conversation_dir(&id).is_none()) {
        return failed(format!(
            "conversation `{}` does not exist",
            req.conversation
        ));
    }

    let current = match read_draft_file(&path) {
        Ok(current) => current,
        Err(message) => return failed(message),
    };
    let current_revision = current.as_deref().map(draft_revision);

    if current_revision != req.revision {
        return HostToPlugin::Draft(DraftResponse {
            id: req.id,
            conversation: req.conversation,
            content: current
                .as_deref()
                .map(draft_query_text)
                .unwrap_or_default()
                .to_owned(),
            revision: current_revision,
            conflict: true,
        });
    }

    // An empty draft is no draft: a blank file left behind would have the CLI
    // seed an editor with nothing and treat it as a recovery copy.
    if req.content.is_empty() {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return failed(format!("failed to remove the draft: {error}")),
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

    if let Err(message) = write_draft_file(&path, &req.content) {
        return failed(message);
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
    let roots = config_search_roots(Some(workspace), fs_backend);

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

fn handle_list_conversations(workspace: &Workspace, req_id: Option<String>) -> HostToPlugin {
    let data: Vec<ConversationSummary> = workspace
        .conversations()
        .map(|(id, meta)| ConversationSummary {
            id: id.to_string(),
            title: meta.title.clone(),
            last_activated_at: meta.last_activated_at,
            pinned_at: meta.pinned_at,
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

    HostToPlugin::Events(EventsResponse {
        id: req_id,
        conversation: conversation_id.to_owned(),
        data: event_values,
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

/// Check if a process is still alive by PID.
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

/// Terminate a child process by PID.
///
/// Used as a last resort when the plugin doesn't exit within the grace period
/// after receiving `Shutdown`.
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
    interactive: bool,
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
    if let Some(path) = try_registry_install(name, plugins_config, interactive).await? {
        return Ok(Some(path));
    }

    // 3. Check $PATH with run policy.
    let segments: Vec<&str> = name.split('-').collect();
    if let Some(path) = find_plugin_binary(&segments) {
        check_run_policy(name, &path, plugin_cfg, interactive)?;
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
    interactive: bool,
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
            if !interactive {
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
    interactive: bool,
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
            if !interactive {
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
    let Some(binary) =
        resolve_plugin_binary(subcommand, &config.plugins, ctx.term.interactive).await?
    else {
        return Err(unknown_subcommand_error(subcommand));
    };

    debug!(%binary, subcommand, "Dispatching to plugin.");

    run_plugin(subcommand, &binary, plugin_args, ctx)?;
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
