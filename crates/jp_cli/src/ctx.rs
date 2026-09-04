use std::{
    collections::HashSet,
    io::{self, IsTerminal as _},
    sync::Arc,
    time::Duration,
};

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use jp_config::{AppConfig, PartialAppConfig, conversation::tool::ToolSource};
use jp_mcp::{StartupSet, id::McpServerId};
use jp_printer::Printer;
use jp_storage::backend::FsStorageBackend;
use jp_task::TaskHandler;
use jp_workspace::{Workspace, session::Session};
use tokio::runtime::{Handle, Runtime};

use crate::{
    Globals, Result, bootstrap::ExecutionContext, config_pipeline::ConfigResetEvents,
    signals::SignalRouter,
};

/// Context for the CLI application
pub(crate) struct Ctx {
    /// The bootstrap-resolved execution context: launch cwd, selected checkout
    /// root, and the working directory for spawned children (RFD 087).
    pub(crate) exec: ExecutionContext,

    /// The workspace.
    pub(crate) workspace: Workspace,

    /// Filesystem-specific backend for path queries, config loading, and
    /// file-level cleanup.
    /// `None` when running with an in-memory backend.
    pub(crate) fs_backend: Option<Arc<FsStorageBackend>>,

    /// Merged file/CLI configuration.
    config: Arc<AppConfig>,

    /// Global CLI arguments.
    pub(crate) term: Term,

    /// The resolved terminal session identity, if any.
    ///
    /// `None` when no session could be detected (e.g. no controlling terminal,
    /// no `$JP_SESSION`, and no recognized terminal env vars).
    pub(crate) session: Option<Session>,

    /// The printer for output.
    pub(crate) printer: Arc<Printer>,

    /// MCP client for interacting with MCP servers.
    pub(crate) mcp_client: jp_mcp::Client,

    pub(crate) task_handler: jp_task::TaskHandler,

    /// Routes OS signals: Ctrl-C escalation, scoped interrupt handlers, and the
    /// root shutdown token.
    pub(crate) signals: SignalRouter,

    /// A `--cfg` reset keyword's persistence payload ([RFD 038]).
    ///
    /// `Some` when the invocation's `--cfg` list contained `NONE` or
    /// `WORKSPACE`; the query command appends the corresponding events to a
    /// continuing conversation's stream.
    ///
    /// [RFD 038]: https://jp.computer/rfd/038
    pub(crate) config_reset: Option<ConfigResetEvents>,

    runtime: Runtime,

    #[cfg(test)]
    pub(crate) stubbed_now: DateTime<Utc>,
}

pub(crate) struct Term {
    /// Global CLI arguments.
    pub(crate) args: Globals,

    /// Whether or not stdout is connected to a TTY.
    ///
    /// If you pipe (|) or redirect (\>) the output, stdout is connected to a
    /// pipe or a regular file, respectively.
    /// These are not managed by the TTY subsystem.
    ///
    /// Answers "can the consumer of my output handle ANSI escapes?" — output
    /// format resolution, spinners, cursor control, OSC sequences.
    /// For "can a user answer a prompt?", use [`Self::interactive`] ([RFD
    /// 048]).
    ///
    /// [RFD 048]: https://jp.computer/rfd/048
    pub(crate) is_tty: bool,

    /// Whether a user is present to answer prompts.
    ///
    /// Gates tool permission and result prompts, label resolution, title
    /// selection, lock-timeout handling, plugin approval, and the editor's
    /// re-open confirmation.
    /// Distinct from [`Self::is_tty`], which asks whether output can carry ANSI
    /// escapes ([RFD 048]).
    ///
    /// Not every prompt consults it: `conversation rm` and `conversation
    /// archive` gate their confirmations on `--force` and `--confirm` instead.
    ///
    /// [RFD 048]: https://jp.computer/rfd/048
    pub(crate) interactive: bool,

    /// Width in columns to lay output out against.
    ///
    /// The controlling terminal's width when stdout is a TTY, or the width the
    /// caller declared with `--width`.
    /// `None` when stdout is piped or redirected without a declared width, so
    /// list output keeps its full width for machine consumption rather than
    /// wrapping to a guessed size.
    pub(crate) width: Option<u16>,
}

impl Ctx {
    /// Create a new context with the given workspace
    pub(crate) fn new(
        exec: ExecutionContext,
        workspace: Workspace,
        fs_backend: Option<Arc<FsStorageBackend>>,
        runtime: Runtime,
        args: Globals,
        config: impl Into<Arc<AppConfig>>,
        session: Option<Session>,
        printer: Printer,
    ) -> Self {
        let config = config.into();
        let escalation_cooldown =
            Duration::from_secs(config.interrupt.escalation_cooldown_secs.into());
        let mcp_client = jp_mcp::Client::new(config.providers.mcp.clone())
            .with_child_cwd(exec.child_cwd().map(|cwd| cwd.as_std_path().to_path_buf()));

        let is_tty = io::stdout().is_terminal();
        let width = printer.output_width().columns();

        // Same derivation as `is_tty` for now; the two diverge when the
        // promptability signal moves to `/dev/tty` availability.
        let interactive = is_tty;

        Self {
            exec,
            workspace,
            fs_backend,
            config,
            term: Term {
                args,
                is_tty,
                interactive,
                width,
            },
            session,
            printer: Arc::new(printer),
            mcp_client,
            task_handler: TaskHandler::default(),
            signals: SignalRouter::new(&runtime, escalation_cooldown),
            config_reset: None,
            runtime,

            #[cfg(test)]
            stubbed_now: DateTime::<Utc>::UNIX_EPOCH,
        }
    }

    #[cfg(not(test))]
    #[expect(clippy::unused_self)]
    pub(crate) fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }

    #[cfg(test)]
    pub(crate) fn now(&self) -> DateTime<Utc> {
        self.stubbed_now
    }

    #[cfg(test)]
    pub(crate) fn set_now(&mut self, now: DateTime<Utc>) {
        self.stubbed_now = now;
    }

    /// Returns the storage path, if filesystem storage is configured.
    pub(crate) fn storage_path(&self) -> Option<&Utf8Path> {
        self.fs_backend
            .as_deref()
            .map(FsStorageBackend::storage_path)
    }

    /// Returns the user storage path, if filesystem storage is configured.
    pub(crate) fn user_storage_path(&self) -> Option<&Utf8Path> {
        self.fs_backend
            .as_deref()
            .and_then(FsStorageBackend::user_storage_path)
    }

    /// Get immutable access to the configuration.
    ///
    /// NOTE: There is *NO* mutable access to the configuration *after*
    /// configuration initialization.
    /// This is to simplify the cognetive complexity of configuration lifecycle
    /// management throughout the lifetime of the CLI application.
    ///
    /// Any changes to the configuration should be done using the "partial
    /// configuration" API in [`jp_config`] *before* constructing the final
    /// [`AppConfig`] object.
    pub(crate) fn config(&self) -> Arc<AppConfig> {
        self.config.clone()
    }

    /// Get a runtime handle.
    pub(crate) fn handle(&self) -> &Handle {
        self.runtime.handle()
    }

    /// Activate and deactivate MCP servers based on the active conversation
    /// context.
    ///
    /// `forced_tool` names a tool the turn runs even though it is disabled (`jp
    /// query -u NAME`).
    /// Its backing server has to start regardless of the tool's enable state,
    /// or `tool_definitions` drops the tool again as unreachable and the forced
    /// choice cannot be satisfied.
    pub(crate) async fn configure_active_mcp_servers(
        &mut self,
        forced_tool: Option<&str>,
    ) -> Result<StartupSet> {
        let mut server_ids = HashSet::new();

        for (name, cfg) in self.config.conversation.tools.iter() {
            if !cfg.is_enabled() && forced_tool != Some(name) {
                continue;
            }

            let ToolSource::Mcp { server, .. } = &cfg.source() else {
                continue;
            };

            server_ids.insert(McpServerId::new(server));
        }

        self.mcp_client
            .run_services(server_ids, self.handle().clone())
            .await
            .map_err(Into::into)
    }
}

/// A trait for converting any type into a partial [`AppConfig`].
pub(crate) trait IntoPartialAppConfig {
    /// Apply CLI flag overrides to the partial config.
    ///
    /// `merged_config` may contain the full configuration for validation when
    /// `partial` is incomplete.
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>>;

    #[expect(unused_variables)]
    fn apply_conversation_config(
        &self,
        workspace: &Workspace,
        partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
        handle: &jp_workspace::ConversationHandle,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        Ok(partial)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.printer.shutdown();
    }
}
