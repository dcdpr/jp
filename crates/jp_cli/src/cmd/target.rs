//! Conversation targeting and resolution.
//!
//! Commands declare what conversations they need via
//! [`ConversationLoadRequest`]. The startup pipeline in `run_inner` resolves
//! that request into concrete [`ConversationHandle`]s and optionally loads
//! per-conversation config.
//!
//! The resolution rules for the `targets` field:
//!
//! - `None`: command doesn't need any conversations. Zero handles returned.
//! - `Some([])`: need one, resolve from session mapping → picker → error.
//! - `Some([target])`: single target (keyword, ID, or picker).
//! - `Some([id1, id2, ...])`: multiple explicit IDs.
//!
//! See also [`super::conversation_id`] for the shared clap argument types
//! that produce [`ConversationTarget`] values.

use std::{
    collections::HashMap,
    io::{self, IsTerminal as _},
};

use jp_config::conversation::DefaultConversationId;
use jp_conversation::ConversationId;
use jp_workspace::{ConversationHandle, Workspace, session::Session};

use crate::error::{Error, Result};

/// A command's declaration of what conversations it needs.
///
/// Returned by each command's `conversation_load_request()` method. The
/// startup pipeline resolves this into `Vec<ConversationHandle>`.
pub(crate) struct ConversationLoadRequest {
    /// Parsed conversation targets from CLI arguments.
    ///
    /// - `None`: no conversations needed (e.g. `jp c ls`, `jp query --new`)
    /// - `Some(vec![])`: need one, resolve from session/picker
    /// - `Some(vec![target])`: single target (keyword, ID, or picker)
    /// - `Some(vec![id1, id2])`: multiple explicit IDs
    ///
    /// When all targets are literal `Id` values, they are resolved in
    /// accumulation mode (all IDs collected). When any target is a keyword
    /// or `Picker`, the list is treated as a fallback chain (first success
    /// wins, errors skip to the next target).
    pub targets: Option<Vec<ConversationTarget>>,

    /// Which resolved handle (by index) should be used for config loading.
    ///
    /// `None` means no per-conversation config. `Some(0)` is the common case
    /// for commands that need config from their target conversation.
    pub config_conversation: Option<usize>,
}

impl ConversationLoadRequest {
    /// No conversations needed.
    pub fn none() -> Self {
        Self {
            targets: None,
            config_conversation: None,
        }
    }

    /// Need one conversation resolved from session/picker, no config loading.
    pub fn from_session() -> Self {
        Self {
            targets: Some(vec![]),
            config_conversation: None,
        }
    }

    /// Need one conversation resolved from session/picker, with config loading.
    pub fn from_session_with_config() -> Self {
        Self {
            targets: Some(vec![]),
            config_conversation: Some(0),
        }
    }

    /// Explicit targets (single or list), no config loading.
    pub fn explicit(targets: Vec<ConversationTarget>) -> Self {
        Self {
            targets: Some(targets),
            config_conversation: None,
        }
    }

    /// Explicit targets with config loading from the first handle.
    pub fn explicit_with_config(targets: Vec<ConversationTarget>) -> Self {
        Self {
            targets: Some(targets),
            config_conversation: Some(0),
        }
    }

    /// Use explicit targets if non-empty, otherwise resolve from
    /// session/picker.
    pub fn explicit_or_session(targets: &[ConversationTarget]) -> Self {
        if targets.is_empty() {
            Self::from_session()
        } else {
            Self::explicit(targets.to_vec())
        }
    }

    /// Use explicit targets if non-empty, otherwise resolve from
    /// session/picker with config loading.
    pub fn explicit_or_session_with_config(targets: &[ConversationTarget]) -> Self {
        if targets.is_empty() {
            Self::from_session_with_config()
        } else {
            Self::explicit_with_config(targets.to_vec())
        }
    }

    /// Use explicit targets if non-empty, otherwise no conversations needed.
    pub fn explicit_or_none(targets: &[ConversationTarget]) -> Self {
        if targets.is_empty() {
            Self::none()
        } else {
            Self::explicit(targets.to_vec())
        }
    }

    /// Use explicit targets if non-empty, otherwise try the session's
    /// previous conversation, falling back to the interactive picker.
    pub fn explicit_or_previous(targets: &[ConversationTarget]) -> Self {
        if targets.is_empty() {
            Self::explicit(vec![
                ConversationTarget::Previous,
                ConversationTarget::Picker,
            ])
        } else {
            Self::explicit(targets.to_vec())
        }
    }
}

/// Resolve a [`ConversationLoadRequest`] into concrete handles.
///
/// This is the single resolution entry point called by `run_inner`.
///
/// `default_id` is the configured fallback from `conversation.default_id`,
/// consulted when no session mapping exists and no explicit `--id` is given.
pub(crate) fn resolve_request(
    request: &ConversationLoadRequest,
    workspace: &Workspace,
    session: Option<&Session>,
    default_id: DefaultConversationId,
) -> Result<Vec<ConversationHandle>> {
    let Some(targets) = &request.targets else {
        return Ok(vec![]);
    };

    let ids = resolve_targets(targets, workspace, session, default_id)?;

    ids.iter()
        .map(|id| workspace.acquire_conversation(id).map_err(Into::into))
        .collect()
}

/// A parsed conversation target from a CLI argument.
///
/// Represents the user's intent before resolution against the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConversationTarget {
    /// A specific conversation ID.
    Id(ConversationId),

    /// The most recently activated conversation (any session).
    /// `last`, `last-active`, `last-activated`, `l`
    LastActivated,

    /// The most recently created conversation.
    /// `last-created`
    LastCreated,

    /// The session's previously active conversation.
    /// `previous`, `prev`, `p`
    Previous,

    /// The current session's active conversation.
    /// `current`, `c`
    Current,

    /// All conversations in the current session's history.
    /// `session`
    Session,

    /// Interactive picker (bare `--id` or no positional arg).
    Picker,
}

impl ConversationTarget {
    /// Parse a raw string into a conversation target.
    ///
    /// Empty string triggers picker mode. Recognized keywords resolve to their
    /// respective variants. Anything else is parsed as a conversation ID.
    pub(crate) fn parse(s: &str) -> Result<Self> {
        match s {
            "" => Ok(Self::Picker),
            "last" | "last-active" | "last-activated" | "l" => Ok(Self::LastActivated),
            "last-created" => Ok(Self::LastCreated),
            "previous" | "prev" | "p" => Ok(Self::Previous),
            "current" | "c" => Ok(Self::Current),
            "session" => Ok(Self::Session),
            _ => Ok(Self::Id(s.parse::<ConversationId>()?)),
        }
    }

    /// The keyword name for this target, if it is a keyword.
    pub(crate) fn keyword_name(&self) -> Option<&'static str> {
        match self {
            Self::Id(_) | Self::Picker => None,
            Self::LastActivated => Some("last"),
            Self::LastCreated => Some("last-created"),
            Self::Previous => Some("previous"),
            Self::Current => Some("current"),
            Self::Session => Some("session"),
        }
    }

    /// Resolve this target to concrete conversation IDs.
    ///
    /// Returns an empty vec for `Picker` — the caller must handle interactive
    /// selection separately. Returns multiple IDs for `Session`.
    pub(crate) fn resolve(
        &self,
        workspace: &Workspace,
        session: Option<&Session>,
    ) -> Result<Vec<ConversationId>> {
        match self {
            Self::Id(id) => Ok(vec![*id]),
            Self::LastActivated => {
                let id = workspace
                    .conversations()
                    .max_by_key(|(_, c)| c.last_activated_at)
                    .map(|(id, _)| *id)
                    .ok_or_else(|| {
                        Error::NotFound("conversation", "no conversations exist".into())
                    })?;
                Ok(vec![id])
            }
            Self::LastCreated => {
                let id = workspace
                    .conversations()
                    .max_by_key(|(id, _)| id.timestamp())
                    .map(|(id, _)| *id)
                    .ok_or_else(|| {
                        Error::NotFound("conversation", "no conversations exist".into())
                    })?;
                Ok(vec![id])
            }
            Self::Previous => {
                let id = session
                    .and_then(|s| workspace.session_previous_conversation(s))
                    .ok_or_else(|| {
                        Error::NotFound(
                            "conversation",
                            "no previous conversation in this session".into(),
                        )
                    })?;
                Ok(vec![id])
            }
            Self::Current => {
                let id = session
                    .and_then(|s| workspace.session_active_conversation(s))
                    .ok_or_else(|| {
                        Error::NotFound(
                            "conversation",
                            "no active conversation in this session".into(),
                        )
                    })?;
                Ok(vec![id])
            }
            Self::Session => {
                let ids = session
                    .map(|s| workspace.session_conversation_ids(s))
                    .unwrap_or_default();
                if ids.is_empty() {
                    return Err(Error::NotFound(
                        "conversation",
                        "no conversations in this session".into(),
                    ));
                }
                Ok(ids)
            }
            Self::Picker => Ok(vec![]),
        }
    }
}

/// Resolve parsed targets into concrete conversation IDs.
///
/// - 0 targets: session mapping → picker → error
/// - All `Id` targets: accumulate all (multi-target mode)
/// - Any keyword/Picker targets: fallback chain (first success wins)
fn resolve_targets(
    targets: &[ConversationTarget],
    workspace: &Workspace,
    session: Option<&Session>,
    default_id: DefaultConversationId,
) -> Result<Vec<ConversationId>> {
    if targets.is_empty() {
        let id = resolve_from_session_or_picker(workspace, session, default_id)?;
        return Ok(vec![id]);
    }

    let all_ids = targets
        .iter()
        .all(|t| matches!(t, ConversationTarget::Id(_)));
    let any_ids = targets
        .iter()
        .any(|t| matches!(t, ConversationTarget::Id(_)));

    // Mixing literal IDs with keywords/Picker is a logic error — user input
    // is validated by `validate_multi` in the clap types, so this can only
    // happen from a buggy programmatic construction.
    debug_assert!(
        all_ids || !any_ids,
        "cannot mix literal conversation IDs with keywords: {targets:?}"
    );

    // When all targets are literal IDs, resolve all of them.
    if all_ids {
        return targets
            .iter()
            .flat_map(|t| t.resolve(workspace, session).into_iter().flatten())
            .map(Ok)
            .collect();
    }

    // Otherwise treat the list as a fallback chain: try each target in order,
    // return the first successful resolution.
    let mut last_err = None;
    for target in targets {
        match target.resolve(workspace, session) {
            Ok(v) if v.is_empty() => {
                // Picker — resolve interactively.
                return resolve_picker(workspace, session).map(|id| vec![id]);
            }
            Ok(v) => return Ok(v),
            Err(e) => last_err = Some(e),
        }
    }

    Err(last_err.expect("targets is non-empty"))
}

/// Try session mapping first, then configured default, then picker/error.
fn resolve_from_session_or_picker(
    workspace: &Workspace,
    session: Option<&Session>,
    default_id: DefaultConversationId,
) -> Result<ConversationId> {
    if let Some(id) = session.and_then(|s| workspace.session_active_conversation(s)) {
        return Ok(id);
    }

    // Try the configured default before falling through to the picker.
    if let Some(id) = resolve_default_id(default_id, workspace, session) {
        return Ok(id);
    }

    resolve_picker(workspace, session)
}

/// Resolve the `conversation.default_id` config value to a concrete ID.
///
/// Returns `None` for `Ask` or unset (fall through to picker), or if the target
/// cannot be resolved (e.g. `Previous` with no session history).
fn resolve_default_id(
    default_id: DefaultConversationId,
    workspace: &Workspace,
    session: Option<&Session>,
) -> Option<ConversationId> {
    let target = match default_id {
        DefaultConversationId::Ask => return None,
        DefaultConversationId::LastActivated => ConversationTarget::LastActivated,
        DefaultConversationId::LastCreated => ConversationTarget::LastCreated,
        DefaultConversationId::Previous => ConversationTarget::Previous,
        DefaultConversationId::Id(id) => id.parse::<_>().ok().map(ConversationTarget::Id)?,
    };

    // Swallow resolution errors (e.g. "no conversations exist", "no previous
    // conversation") — the caller falls through to the picker instead.
    target.resolve(workspace, session).ok()?.into_iter().next()
}

/// Show the interactive picker, or error if non-interactive.
fn resolve_picker(workspace: &Workspace, session: Option<&Session>) -> Result<ConversationId> {
    if !io::stdin().is_terminal() {
        return Err(Error::NoConversationTarget);
    }

    pick_conversation(workspace, session).ok_or(Error::NoConversationTarget)
}

/// Show an interactive conversation picker.
///
/// Conversations are sorted by `last_activated_at` (most recent first).
/// The session's active conversation (if any) is pinned to the top.
///
/// Returns `Some(id)` on selection, `None` if the list is empty or the
/// user cancels.
fn pick_conversation(workspace: &Workspace, session: Option<&Session>) -> Option<ConversationId> {
    let mut items: Vec<_> = workspace
        .conversations()
        .map(|(id, c)| {
            let label = match &c.title {
                Some(t) => format!("{id}  {t}"),
                None => id.to_string(),
            };
            (*id, label)
        })
        .collect();

    if items.is_empty() {
        return None;
    }

    // Sort most-recently-activated first.
    let meta: HashMap<_, _> = workspace
        .conversations()
        .map(|(id, c)| (*id, c.last_activated_at))
        .collect();

    items.sort_by(|a, b| meta[&b.0].cmp(&meta[&a.0]));

    // Pin the session's active conversation to the top.
    if let Some(s) = session
        && let Some(active) = workspace.session_active_conversation(s)
        && let Some(pos) = items.iter().position(|(id, _)| *id == active)
    {
        let entry = items.remove(pos);
        items.insert(0, entry);
    }

    let labels: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
    let mut writer = io::stderr();
    let selected = inquire::Select::new("Select a conversation", labels)
        .prompt_with_writer(&mut writer)
        .ok()?;

    items
        .iter()
        .find(|(_, l)| *l == selected)
        .map(|(id, _)| *id)
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
