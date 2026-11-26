//! Apply the `query` command's CLI flags to the turn configuration and record
//! durable changes in the conversation stream.
//!
//! [`IntoPartialAppConfig::apply_cli_config`] applies the flags to the partial
//! config.
//! [`turn_config_delta`] and [`persist_config_reset`] record config changes in
//! the conversation stream so they persist across turns.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use jp_config::{
    AppConfig, PartialAppConfig, PartialConfig as _, PartialConfigDelta as _,
    assignment::AssignKeyValue as _,
    assistant::tool_choice::ToolChoice,
    conversation::tool::{Enable, PartialEnableConfig, PartialToolConfig, ToggleScope},
    fs::load_partial,
    model::{
        id::{PartialModelIdOrAliasConfig, ProviderId},
        parameters::{
            PartialCustomReasoningConfig, PartialReasoningConfig, ReasoningConfig, ServiceTier,
        },
    },
    providers::llm::AuthEntry,
    style::reasoning::ReasoningDisplayConfig,
};
use jp_conversation::{
    ConversationStream,
    stream::{ApplyDelta, ResetDelta},
};
use jp_tool::Error as ToolError;
use jp_workspace::{ConversationHandle, ConversationLock, Workspace};

use super::{Query, ToolDirective, mounts::apply_mounts, tool};
use crate::{
    cmd::attachment::attach_urls,
    config_pipeline::{ConfigReset, ConfigResetEvents},
    ctx::IntoPartialAppConfig,
    error::Result,
    parser::AttachmentUrlOrPath,
};

pub(super) type BoxedResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Append a `--cfg` reset keyword's events to a conversation stream.
///
/// Persists the reset-then-layer sequence from [RFD 038]: a [`ResetDelta`]
/// marking the reset point, then the workspace partial for `WORKSPACE` resets,
/// then whatever state this invocation layered on top of the reset point.
/// Layers carrying nothing are skipped by
/// [`ConversationStream::add_config_reset`].
///
/// [RFD 038]: https://jp.computer/rfd/038
pub(super) fn persist_config_reset(
    events: &mut ConversationStream,
    reset: ConfigResetEvents,
    timestamp: DateTime<Utc>,
) {
    let mut layers = Vec::with_capacity(2);

    if let ConfigReset::Workspace(delta) = reset.reset {
        layers.push(ApplyDelta::new(timestamp, delta));
    }

    layers.push(ApplyDelta::new(timestamp, reset.post));

    events.add_config_reset(ResetDelta { timestamp }, layers);
}

/// Compute the config delta needed for the conversation stream to match `cfg`.
///
/// Compares the stream's resolved config against the effective config `cfg` for
/// this turn (including `--cfg` files, command flags such as `--model`, and
/// named compaction settings).
/// Recording this delta makes the change persist for subsequent turns.
/// Returns `None` when the stream's config already matches `cfg`.
///
/// Any field whose merge strategy cannot produce the new value is replaced in
/// full and has its path recorded in the returned `unsets`, clearing the field
/// before the delta merges.
pub(crate) fn turn_config_delta(
    cfg: &AppConfig,
    lock: &ConversationLock,
) -> Result<Option<ApplyDelta>> {
    let partial = lock
        .events()
        .config()
        .map(|c| c.to_partial())
        .map_err(jp_conversation::Error::from)?;

    let mut unsets = Vec::new();
    let partial = partial.delta_with_unsets(cfg.to_partial(), "", &mut unsets);

    if partial.is_empty() && unsets.is_empty() {
        return Ok(None);
    }

    Ok(Some(ApplyDelta::with_unsets(Utc::now(), partial, unsets)))
}

impl IntoPartialAppConfig for Query {
    fn apply_cli_config(
        &self,
        workspace: Option<&Workspace>,
        mut partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        // Must run before tool-enable processing, which reads the injected
        // `enable` blocks.
        inject_builtin_tools(&mut partial)?;

        let Self {
            model,
            auth,
            template: _,
            schema: _,
            replay: _,
            new_conversation: _,
            local: _,
            no_local: _,
            attachments,
            edit: _,
            no_edit: _,
            query: _,
            quote: _,
            tool_use,
            no_tool_use: _,
            parameters,
            hide_reasoning,
            hide_tool_calls,
            tool_directives,
            reasoning,
            no_reasoning,
            service_tier,
            no_service_tier,
            expires_in: _,
            target: _,
            fork: _,
            compact: _,
            title: _,
            no_title: _,
            mount,
        } = self;

        apply_model(&mut partial, model.as_deref(), merged_config);
        apply_auth(&mut partial, auth, merged_config)?;
        apply_enable_tools(&mut partial, tool_directives, merged_config)?;
        validate_tool_use(&partial, tool_use.as_ref().map(Option::as_deref))?;
        apply_attachments(&mut partial, attachments, workspace)?;
        apply_mounts(&mut partial, mount, workspace, merged_config)?;
        apply_reasoning(&mut partial, reasoning.as_ref(), *no_reasoning);

        // `off` rather than `None`: a partial's `None` means "no opinion", so
        // it would leave a tier an earlier layer asked for standing, and the
        // conversation layer is gap-filled from the file layer on later turns.
        if let Some(tier) = no_service_tier
            .then_some(ServiceTier::Off)
            .or(*service_tier)
        {
            partial.assistant.model.parameters.service_tier = Some(tier);
        }

        for kv in parameters.clone() {
            partial.assistant.model.parameters.assign(kv)?;
        }

        if *hide_reasoning {
            partial.style.reasoning.display = Some(ReasoningDisplayConfig::Hidden);
        }

        if *hide_tool_calls {
            partial.style.tool_call.show = Some(false);
        }

        Ok(partial)
    }
}

/// Load the conversation's config layer from `handle`, merge it over `partial`,
/// and return the merged partial along with any explicitly unset paths.
///
/// # Errors
///
/// Returns an error when the conversation's events or configuration cannot be
/// read.
pub(in crate::cmd) fn conversation_config(
    workspace: &Workspace,
    partial: PartialAppConfig,
    handle: &ConversationHandle,
) -> BoxedResult<(PartialAppConfig, Vec<String>)> {
    let events = workspace.events(handle)?;
    let config = events.config()?.to_partial();
    let unsets = events.config_unsets()?;

    Ok((load_partial(partial, config)?, unsets))
}

/// Write `--auth` to the `auth` chain of the provider serving this turn.
///
/// Runs after [`apply_model`], since the provider comes from the turn's model.
/// A provider that cannot be determined is an error: writing the chain to the
/// wrong one would silently do nothing.
fn apply_auth(
    partial: &mut PartialAppConfig,
    auth: &[AuthEntry],
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    if auth.is_empty() {
        return Ok(());
    }

    let provider = active_provider(partial, merged_config).ok_or_else(|| {
        format!(
            "--auth needs to know which provider to bill, and the model for this turn does not \
             name one; pass `--model <provider>/<name>`, or set the chain directly with `--cfg \
             providers.llm.<provider>.auth={}`",
            auth.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
    })?;

    let auth = auth.to_vec();
    let llm = &mut partial.providers.llm;
    match provider {
        ProviderId::Anthropic => llm.anthropic.auth = Some(auth),
        ProviderId::Cerebras => llm.cerebras.auth = Some(auth),
        ProviderId::Deepseek => llm.deepseek.auth = Some(auth),
        ProviderId::Google => llm.google.auth = Some(auth),
        ProviderId::Openai => llm.openai.auth = Some(auth),
        ProviderId::Openrouter => llm.openrouter.auth = Some(auth),
        ProviderId::Vllm => llm.vllm.auth = Some(auth),

        provider @ (ProviderId::Llamacpp
        | ProviderId::Ollama
        | ProviderId::Test
        | ProviderId::Xai) => {
            return Err(format!(
                "--auth is not supported for `{provider}`: it needs no credential"
            )
            .into());
        }
    }

    Ok(())
}

/// The provider serving this turn, if the config says which.
///
/// Reads the CLI's `--model` first, then the config layers.
fn active_provider(
    partial: &PartialAppConfig,
    merged_config: Option<&PartialAppConfig>,
) -> Option<ProviderId> {
    let aliases = merged_config.map_or(&partial.providers.llm.aliases, |merged| {
        &merged.providers.llm.aliases
    });

    [Some(partial), merged_config]
        .into_iter()
        .flatten()
        .find_map(|config| {
            PartialModelIdOrAliasConfig::finalize(&config.assistant.model.id, aliases)
                .ok()?
                .provider
        })
}

/// Apply the CLI model configuration to the partial configuration.
///
/// `model` is the raw `--model` value: an alias or a full `provider/name` ID.
/// It is stored unresolved; the config pipeline resolves aliases when it builds
/// the final [`AppConfig`].
pub(in crate::cmd) fn apply_model(
    partial: &mut PartialAppConfig,
    model: Option<&str>,
    _: Option<&PartialAppConfig>,
) {
    let Some(id) = model else { return };

    partial.assistant.model.id = id.into();
}

fn apply_enable_tools(
    partial: &mut PartialAppConfig,
    directives: &[ToolDirective],
    merged_config: Option<&PartialAppConfig>,
) -> BoxedResult<()> {
    if directives.is_empty() {
        return Ok(());
    }

    let existing_tools = merged_config.map_or(&partial.conversation.tools.tools, |v| {
        &v.conversation.tools.tools
    });

    // Validate all named tools exist.
    let missing: HashSet<_> = directives
        .iter()
        .filter_map(ToolDirective::as_single)
        .filter(|name| !existing_tools.contains_key(*name))
        .collect();

    if missing.len() == 1 {
        return Err(ToolError::NotFound {
            name: missing.iter().next().unwrap().to_string(),
        }
        .into());
    } else if !missing.is_empty() {
        return Err(ToolError::NotFoundN {
            names: missing.into_iter().map(ToString::to_string).collect(),
        }
        .into());
    }

    // Apply directives left-to-right. Each directive only flips a tool's
    // `state`; the tool's `allow_toggle` policy decides whether the flip is
    // permitted for the directive's scope (see `Enable::accepts`). A named
    // directive a policy forbids is an error; a bulk directive is skipped.
    let defaults = partial
        .conversation
        .tools
        .defaults
        .enable
        .clone()
        .unwrap_or_default();
    for d in directives {
        match d {
            ToolDirective::EnableAll => {
                for (name, tool) in partial.conversation.tools.tools.iter_mut() {
                    apply_directive_to_tool(name, tool, &defaults, ToggleScope::Bulk, true)?;
                }
            }
            ToolDirective::DisableAll => {
                for (name, tool) in partial.conversation.tools.tools.iter_mut() {
                    apply_directive_to_tool(name, tool, &defaults, ToggleScope::Bulk, false)?;
                }
            }
            ToolDirective::Enable(name) => {
                if let Some(tool) = partial.conversation.tools.tools.get_mut(name.as_str()) {
                    apply_directive_to_tool(name, tool, &defaults, ToggleScope::Named, true)?;
                }
            }
            ToolDirective::Disable(name) => {
                if let Some(tool) = partial.conversation.tools.tools.get_mut(name.as_str()) {
                    apply_directive_to_tool(name, tool, &defaults, ToggleScope::Named, false)?;
                }
            }
        }
    }

    Ok(())
}

/// Resolve a partial tool's effective [`Enable`] against the global `*`
/// defaults (and the hardcoded fallback).
pub(super) fn effective_enable(
    enable: Option<&PartialEnableConfig>,
    defaults: &PartialEnableConfig,
) -> Enable {
    match enable {
        Some(enable) => enable.effective(defaults),
        None => PartialEnableConfig::default().effective(defaults),
    }
}

impl Query {
    /// The tool choice for this turn.
    ///
    /// `-u`/`-U` are invocation-scoped, so they are read here rather than
    /// written into the config: forcing a tool binds this turn and no other.
    /// With neither flag, the conversation's configured `assistant.tool_choice`
    /// applies.
    ///
    /// `-t`/`-T` are the durable counterparts and do reach the config, through
    /// [`apply_enable_tools`].
    pub(super) fn effective_tool_choice(&self, cfg: &AppConfig) -> ToolChoice {
        if self.no_tool_use {
            return ToolChoice::None;
        }

        match self.tool_use.as_ref().map(Option::as_deref) {
            None => cfg.assistant.tool_choice.clone(),
            Some(None | Some("true")) => ToolChoice::Required,
            Some(Some("false")) => ToolChoice::None,
            Some(Some(name)) => ToolChoice::Function(name.to_owned()),
        }
    }
}

/// Apply a single tool directive to one tool's partial `enable`.
///
/// `scope` identifies the directive kind (bulk vs named) and `desired_state` is
/// the state the directive wants.
/// The flip is applied only when the tool's effective `allow_toggle` policy
/// accepts `scope`; the tool's existing `allow_toggle` is always preserved.
///
/// Returns an error only for a *named* directive whose target's policy forbids
/// the flip (a locked tool).
/// Bulk directives skip such tools silently.
fn apply_directive_to_tool(
    name: &str,
    tool: &mut PartialToolConfig,
    defaults: &PartialEnableConfig,
    scope: ToggleScope,
    desired_state: bool,
) -> BoxedResult<()> {
    let effective = effective_enable(tool.enable.as_ref(), defaults);

    // Already in the desired state: nothing to do, and never an error.
    if effective.state == desired_state {
        return Ok(());
    }

    if effective.accepts(scope) {
        // Flip only `state`; the tool's own `allow_toggle` is preserved.
        tool.enable
            .get_or_insert_with(PartialEnableConfig::default)
            .state = Some(desired_state);
        return Ok(());
    }

    if scope == ToggleScope::Named {
        let verb = if desired_state { "enable" } else { "disable" };
        let lock = if effective.state {
            "locked-on"
        } else {
            "locked-off"
        };
        return Err(format!("cannot {verb} `{name}`: this tool is configured as {lock}").into());
    }

    Ok(())
}

/// Merge the built-in tool configs into the partial configuration.
///
/// The built-in block is the *lower*-priority side of each merge: a user who
/// sets one field (`conversation.tools.describe_tools.result = "ask"`) keeps
/// the built-in's `source`, `enable`, `style`, and parameter schema, rather
/// than replacing the whole block with a one-field partial.
///
/// Merging in place preserves each tool's position in the map, which is the
/// order tools are presented to the provider.
///
/// An entry that names a built-in's key but declares a different `source`
/// merges the same way: the user's `source` wins and their executor runs, but
/// the built-in's `run`, `style`, `enable`, and `parameters` survive unless the
/// user overrides those too (RFD 083).
fn inject_builtin_tools(partial: &mut PartialAppConfig) -> BoxedResult<()> {
    for (name, defaults) in tool::builtins::all() {
        let tools = &mut partial.conversation.tools.tools;
        match tools.get_mut(&name) {
            Some(user) => {
                let mut merged = defaults;
                merged
                    .merge(&(), std::mem::take(user))
                    .map_err(|error| error.to_full_string())?;
                *user = merged;
            }
            None => {
                tools.insert(name, defaults);
            }
        }
    }

    Ok(())
}

/// Reject a `-u NAME` the turn could not honour.
///
/// Writes nothing: `-u`/`-U` bind a single turn, so the choice is read from the
/// flags at query time (see [`Query::effective_tool_choice`]) rather than
/// layered into the conversation's config.
///
/// Forcing a tool implies running it even when it is disabled, which
/// [`tool_definitions`] already allows.
/// A tool whose `allow_toggle` policy locks it off cannot be forced, and naming
/// it here surfaces that before MCP boot and the editor, instead of leaving the
/// tool to be dropped silently from the tool list later.
///
/// [`tool_definitions`]: jp_mcp::server::tool_definitions
fn validate_tool_use(
    partial: &PartialAppConfig,
    tool_choice: Option<Option<&str>>,
) -> BoxedResult<()> {
    let Some(Some(name)) = tool_choice else {
        return Ok(());
    };

    if name == "true" || name == "false" {
        return Ok(());
    }

    let defaults = partial
        .conversation
        .tools
        .defaults
        .enable
        .clone()
        .unwrap_or_default();

    let Some(tool) = partial.conversation.tools.tools.get(name) else {
        return Err(format!("tool choice '{name}' does not match any known tools").into());
    };

    // Probe a copy so the flip is discarded: this only asks whether the policy
    // would permit it, and `apply_directive_to_tool` stays the single place
    // that decides what a locked tool refuses. A tool already enabled is a
    // no-op there, so a locked-*on* tool (e.g. a builtin) stays selectable.
    apply_directive_to_tool(name, &mut tool.clone(), &defaults, ToggleScope::Named, true)
}

/// Apply the CLI attachments to the partial configuration.
fn apply_attachments(
    partial: &mut PartialAppConfig,
    attachments: &[AttachmentUrlOrPath],
    workspace: Option<&Workspace>,
) -> Result<()> {
    let root = workspace.map(Workspace::root);
    let attachments = attachments
        .iter()
        .map(|v| v.parse(root))
        .collect::<Result<Vec<_>>>()?;

    attach_urls(partial, attachments)
}

/// Apply the CLI reasoning configuration to the partial configuration.
fn apply_reasoning(
    partial: &mut PartialAppConfig,
    reasoning: Option<&ReasoningConfig>,
    no_reasoning: bool,
) {
    if no_reasoning {
        partial.assistant.model.parameters.reasoning = Some(PartialReasoningConfig::Off);
        return;
    }

    let Some(reasoning) = reasoning else {
        return;
    };

    partial.assistant.model.parameters.reasoning = Some(match reasoning {
        ReasoningConfig::Off => PartialReasoningConfig::Off,
        ReasoningConfig::Auto => PartialReasoningConfig::Auto,
        ReasoningConfig::Custom(custom) => PartialCustomReasoningConfig {
            effort: Some(custom.effort),
            exclude: Some(custom.exclude),
        }
        .into(),
    });
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
