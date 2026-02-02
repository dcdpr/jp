use std::{
    collections::HashSet,
    io::{self, IsTerminal as _},
    sync::Arc,
};

use jp_config::{AppConfig, PartialAppConfig, conversation::tool::ToolSource};
use jp_mcp::id::{McpServerId, McpToolId};
use jp_printer::Printer;
use jp_task::TaskHandler;
use jp_workspace::Workspace;
use time::UtcDateTime;
use tokio::{
    runtime::{Handle, Runtime},
    task::JoinSet,
};

use crate::{Globals, Result, signals::SignalPair};

/// Context for the CLI application
pub(crate) struct Ctx {
    /// The workspace.
    pub(crate) workspace: Workspace,

    /// Merged file/CLI configuration.
    config: Arc<AppConfig>,

    /// Global CLI arguments.
    pub(crate) term: Term,

    /// The printer for output.
    pub(crate) printer: Arc<Printer>,

    /// MCP client for interacting with MCP servers.
    pub(crate) mcp_client: jp_mcp::Client,

    pub(crate) task_handler: jp_task::TaskHandler,

    pub(crate) signals: SignalPair,

    runtime: Runtime,

    #[cfg(test)]
    pub(crate) stubbed_now: UtcDateTime,
}

pub(crate) struct Term {
    /// Global CLI arguments.
    pub(crate) args: Globals,

    /// Whether or not stdout is connected to a TTY.
    ///
    /// If you pipe (|) or redirect (>) the output, stdout is connected to a
    /// pipe or a regular file, respectively. These are not managed by the TTY
    /// subsystem.
    pub(crate) is_tty: bool,
}

impl Ctx {
    /// Create a new context with the given workspace
    pub(crate) fn new(
        workspace: Workspace,
        runtime: Runtime,
        args: Globals,
        config: AppConfig,
        printer: Printer,
    ) -> Self {
        let mcp_client = jp_mcp::Client::new(config.providers.mcp.clone());

        Self {
            workspace,
            config: Arc::new(config),
            term: Term {
                args,
                is_tty: io::stdout().is_terminal(),
            },
            printer: Arc::new(printer),
            mcp_client,
            task_handler: TaskHandler::default(),
            signals: SignalPair::new(&runtime),
            runtime,

            #[cfg(test)]
            stubbed_now: UtcDateTime::UNIX_EPOCH,
        }
    }

    #[cfg(not(test))]
    #[expect(clippy::unused_self)]
    pub(crate) fn now(&self) -> UtcDateTime {
        UtcDateTime::now()
    }

    #[cfg(test)]
    pub(crate) fn now(&self) -> UtcDateTime {
        self.stubbed_now
    }

    #[cfg(test)]
    pub(crate) fn set_now(&mut self, now: UtcDateTime) {
        self.stubbed_now = now;
    }

    /// Get immutable access to the configuration.
    ///
    /// NOTE: There is *NO* mutable access to the configuration *after*
    /// configuration initialization. This is to simplify the cognetive
    /// complexity of configuration lifecycle management throughout the lifetime
    /// of the CLI application.
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
    pub(crate) async fn configure_active_mcp_servers(
        &mut self,
    ) -> Result<JoinSet<std::result::Result<(), jp_mcp::Error>>> {
        let mut server_ids = HashSet::new();

        for (name, cfg) in self.config.conversation.tools.iter() {
            if !cfg.enable() {
                continue;
            }

            let ToolSource::Mcp { server, tool } = &cfg.source() else {
                continue;
            };

            let tool_name = tool.as_deref().unwrap_or(name);
            let server_id = match server.as_deref() {
                Some(server) => McpServerId::new(server),
                None => {
                    self.mcp_client
                        .get_tool_server_id(&McpToolId::new(tool_name), None)
                        .await?
                }
            };

            server_ids.insert(server_id);
        }

        self.mcp_client
            .run_services(server_ids, self.handle().clone())
            .await
            .map_err(Into::into)
    }
}

/// A trait for converting any type into a partial [`AppConfig`].
pub(crate) trait IntoPartialAppConfig {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,

        // Whenever called the `partial` argument might be empty, or contain
        // any subset of the full configuration. This might prevent validating
        // certain fields before applying them. In these situations, the
        // `merged_config` argument can be used to provide the full
        // configuration, and the partial configuration can be validated against
        // it.
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>>;

    #[expect(unused_variables)]
    fn apply_conversation_config(
        &self,
        workspace: Option<&Workspace>,
        partial: PartialAppConfig,

        // Whenever called the `partial` argument might be empty, or contain
        // any subset of the full configuration. This might prevent validating
        // certain fields before applying them. In these situations, the
        // `merged_config` argument can be used to provide the full
        // configuration, and the partial configuration can be validated against
        // it.
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        Ok(partial)
    }
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.printer.shutdown();
    }
}
