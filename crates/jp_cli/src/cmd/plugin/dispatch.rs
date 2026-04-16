//! Host-side plugin message loop.
//!
//! Spawns the plugin binary, sends `init`, and relays workspace queries until
//! the plugin sends `exit` or the process terminates.

use std::{
    collections::HashSet,
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use camino::{Utf8Path, Utf8PathBuf};
use jp_config::{
    AppConfig,
    plugins::{
        PluginsConfig,
        command::{CommandPluginConfig, RunPolicy},
    },
};
use jp_inquire::{InlineOption, InlineSelect};
use jp_plugin::{
    PROTOCOL_VERSION,
    message::{
        ConfigResponse, ConversationSummary, ConversationsResponse, DescribeResponse,
        ErrorResponse, EventsResponse, HostToPlugin, InitMessage, LogMessage, PathsInfo,
        PluginToHost, WorkspaceInfo,
    },
};
use jp_workspace::Workspace;
use serde_json::Value;
use tracing::{debug, error, trace, warn};

use super::registry;
use crate::{Ctx, cmd, signals::SignalPair};

/// Run a plugin binary, handling the full protocol lifecycle.
///
/// `binary` is the path to the plugin executable. `args` are the remaining CLI
/// arguments to forward.
pub(crate) fn run_plugin(
    name: &str,
    binary: &Utf8Path,
    args: &[String],
    workspace: &Workspace,
    storage_path: Option<&Utf8Path>,
    user_storage_path: Option<&Utf8Path>,
    config: &Arc<AppConfig>,
    signals: &SignalPair,
    log_level: u8,
) -> Result<(), cmd::Error> {
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

    let home = std::env::home_dir().and_then(|p| camino::Utf8PathBuf::from_path_buf(p).ok());

    let init = HostToPlugin::Init(InitMessage {
        version: PROTOCOL_VERSION,
        workspace: WorkspaceInfo {
            root: workspace.root().to_owned(),
            storage: storage_path.to_owned(),
            id: workspace.id().to_string(),
        },
        paths: PathsInfo {
            user_data: jp_workspace::user_data_dir().ok(),
            user_config: jp_config::fs::user_global_config_path(home.as_deref()),
            user_workspace: user_storage_path.map(ToOwned::to_owned),
        },
        config: config_json.clone(),
        options,
        args: args.to_vec(),
        log_level,
    });

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

    // Wrap stdin so the shutdown thread can write to it too.
    let stdin = Arc::new(Mutex::new(child_stdin));

    // Forward stderr to tracing in a background thread.
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

    // Shutdown thread: sends `Shutdown` directly to the plugin's stdin
    // when a signal arrives. If the plugin doesn't exit within the grace
    // period, sends SIGKILL.
    let shutdown_sent = Arc::new(AtomicBool::new(false));
    let shutdown_writer = stdin.clone();
    let shutdown_flag = shutdown_sent.clone();
    let child_id = child.id();
    let mut shutdown_rx = signals.receiver.resubscribe();
    let shutdown_handle = thread::spawn(move || {
        if futures::executor::block_on(shutdown_rx.recv()).is_err() {
            return;
        }

        // Send Shutdown over the protocol.
        if let Ok(mut writer) = shutdown_writer.lock() {
            drop(write_message(&mut *writer, &HostToPlugin::Shutdown));
        }
        shutdown_flag.store(true, Ordering::Release);

        // Grace period: wait in short intervals so we don't block cleanup
        // if the plugin exits promptly.
        for _ in 0..50 {
            thread::sleep(std::time::Duration::from_millis(100));
            if !is_process_alive(child_id) {
                return;
            }
        }

        kill_child(child_id);
    });

    // Send init.
    {
        let mut writer = stdin.lock().expect("stdin lock poisoned");
        write_message(&mut *writer, &init)
            .map_err(|e| cmd::Error::from(format!("failed to send init: {e}")))?;
    }

    // Read messages from plugin.
    let reader = BufReader::new(stdout);
    let result = message_loop(reader, &stdin, workspace, &config_json, &shutdown_sent);

    // Always clean up, even on error.
    drop(child.wait());
    drop(stderr_handle.join());
    drop(shutdown_handle);

    result
}

/// The main message loop: reads plugin requests and sends responses.
fn message_loop(
    reader: BufReader<impl std::io::Read>,
    stdin: &Mutex<impl Write>,
    workspace: &Workspace,
    config_json: &Value,
    shutdown_sent: &AtomicBool,
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

        let mut writer = stdin.lock().expect("stdin lock poisoned");

        match msg {
            PluginToHost::Ready => {
                debug!("Plugin signaled ready.");
            }

            PluginToHost::ListConversations(req) => {
                let response = handle_list_conversations(workspace, req.id);
                write_message(&mut *writer, &response)?;
            }

            PluginToHost::ReadEvents(req) => {
                let response = handle_read_events(workspace, &req.conversation, req.id);
                write_message(&mut *writer, &response)?;
            }

            PluginToHost::ReadConfig(req) => {
                let response = handle_read_config(config_json, req.path, req.id);
                write_message(&mut *writer, &response)?;
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

            PluginToHost::Exit(exit) => {
                debug!(code = exit.code, "Plugin exited.");
                if exit.code == 0 {
                    return Ok(());
                }
                return match exit.reason {
                    Some(reason) => Err(cmd::Error::from((exit.code, reason))),
                    None => Err(cmd::Error::from(exit.code)),
                };
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

fn handle_list_conversations(workspace: &Workspace, req_id: Option<String>) -> HostToPlugin {
    let data: Vec<ConversationSummary> = workspace
        .conversations()
        .map(|(id, meta)| ConversationSummary {
            id: id.as_deciseconds().to_string(),
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
    let conv_id = match jp_conversation::ConversationId::try_from_deciseconds_str(conversation_id) {
        Ok(id) => id,
        Err(e) => {
            return HostToPlugin::Error(ErrorResponse {
                id: req_id,
                request: Some("read_events".to_owned()),
                message: format!("invalid conversation ID: {e}"),
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
/// Used as a last resort when the plugin doesn't exit within the grace
/// period after receiving `Shutdown`.
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
/// For `["serve"]`, looks for `jp-serve`. For `["conversation", "export"]`,
/// looks for `jp-conversation-export`.
pub(crate) fn find_plugin_binary(segments: &[&str]) -> Option<Utf8PathBuf> {
    let name = format!("jp-{}", segments.join("-"));
    which::which(&name)
        .ok()
        .and_then(|p| Utf8PathBuf::from_path_buf(p).ok())
}

/// Find any existing plugin binary without downloading or prompting.
///
/// Checks the install directory first, then `$PATH`. Used for non-mutating
/// operations like help requests.
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
/// The `plugins_config` drives installation and execution policy. Per-plugin
/// settings override the defaults from the registry (official vs third-party).
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
/// Spawns the binary, sends `{"type":"describe"}`, reads one response line,
/// and returns the parsed [`DescribeResponse`]. Returns `None` if the plugin
/// doesn't support describe or fails to respond.
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
/// `error:` prefix, usage line, help hint). The message includes our
/// plugin-specific context. Returns exit code 2 (clap's convention for usage
/// errors) with no message, since the output was already written.
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
/// Resolves the plugin binary, then runs the protocol loop. Called from
/// `Commands::run()` after the normal startup flow.
pub(crate) async fn run_external(args: &[String], ctx: &Ctx) -> cmd::Output {
    let (subcommand, plugin_args) = args
        .split_first()
        .ok_or("no subcommand provided for plugin dispatch")?;

    // Handle help without downloading or approval.
    if plugin_args.iter().any(|a| a == "-h" || a == "--help") {
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

    run_plugin(
        subcommand,
        &binary,
        plugin_args,
        &ctx.workspace,
        ctx.storage_path(),
        ctx.user_storage_path(),
        &config,
        &ctx.signals,
        ctx.term.args.verbose,
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;
