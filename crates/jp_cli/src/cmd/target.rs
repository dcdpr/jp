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

use crate::{
    cmd::conversation_id::ConversationIds,
    error::{Error, Result},
};

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

    /// Whether the command accepts multiple conversations.
    ///
    /// When true, `?` opens a multi-select picker instead of a single-select.
    pub multi: bool,

    /// Whether the command supports session-based keywords.
    ///
    /// Used for help text display — controls whether session-related keywords
    /// and multi-target keywords are shown.
    pub session: bool,

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
            multi: false,
            session: true,
            config_conversation: None,
        }
    }

    /// Need one conversation resolved from session/picker, no config loading.
    pub fn from_session() -> Self {
        Self {
            targets: Some(vec![]),
            multi: false,
            session: true,
            config_conversation: None,
        }
    }

    /// Need one conversation resolved from session/picker, with config loading.
    pub fn from_session_with_config() -> Self {
        Self {
            targets: Some(vec![]),
            multi: false,
            session: true,
            config_conversation: Some(0),
        }
    }

    /// Explicit targets (single or list), no config loading.
    pub fn explicit(targets: Vec<ConversationTarget>) -> Self {
        Self {
            targets: Some(targets),
            multi: false,
            session: true,
            config_conversation: None,
        }
    }

    /// Explicit targets with config loading from the first handle.
    pub fn explicit_with_config(targets: Vec<ConversationTarget>) -> Self {
        Self {
            targets: Some(targets),
            multi: false,
            session: true,
            config_conversation: Some(0),
        }
    }

    /// Use explicit targets if non-empty, otherwise resolve from
    /// session/picker.
    pub fn explicit_or_session(args: &dyn ConversationIds) -> Self {
        if args.ids().is_empty() {
            Self::from_session()
        } else {
            let mut req = Self::explicit(args.ids().to_vec());
            req.multi = args.is_multi();
            req.session = args.supports_session();
            req
        }
    }

    /// Use explicit targets if non-empty, otherwise resolve from
    /// session/picker with config loading.
    pub fn explicit_or_session_with_config(args: &dyn ConversationIds) -> Self {
        if args.ids().is_empty() {
            Self::from_session_with_config()
        } else {
            let mut req = Self::explicit_with_config(args.ids().to_vec());
            req.multi = args.is_multi();
            req.session = args.supports_session();
            req
        }
    }

    /// Use explicit targets if non-empty, otherwise no conversations needed.
    pub fn explicit_or_none(args: &dyn ConversationIds) -> Self {
        if args.ids().is_empty() {
            Self::none()
        } else {
            let mut req = Self::explicit(args.ids().to_vec());
            req.multi = args.is_multi();
            req.session = args.supports_session();
            req
        }
    }

    /// Use explicit targets if non-empty, otherwise try the session's
    /// previous conversation, falling back to the interactive picker.
    pub fn explicit_or_previous(args: &dyn ConversationIds) -> Self {
        if args.ids().is_empty() {
            Self::explicit(vec![
                ConversationTarget::SessionPrevious,
                ConversationTarget::Picker(PickerFilter::default()),
            ])
        } else {
            let mut req = Self::explicit(args.ids().to_vec());
            req.multi = args.is_multi();
            req.session = args.supports_session();
            req
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

    let ids = resolve_targets(
        targets,
        workspace,
        session,
        default_id,
        request.multi,
        request.session,
    )?;

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

    /// The most recently activated conversation in the workspace.
    /// `latest`, `l`
    Latest,

    /// The most recently created conversation.
    /// `newest`, `n`
    Newest,

    /// The most recently pinned conversation.
    /// `pinned`, `p`
    LatestPinned,

    /// The session's previously active conversation.
    /// `session`, `s`
    SessionPrevious,

    /// All conversations in the current session's history.
    /// `+session`, `+s`
    AllSession,

    /// All pinned conversations.
    /// `+pinned`, `+p`
    AllPinned,

    /// Interactive picker, optionally filtered.
    /// `?`, `?p`, `?pinned`, `?s`, `?session`
    Picker(PickerFilter),

    /// Print keyword help and exit.
    /// `help`
    Help,
}

/// Filters applied to the interactive conversation picker.
///
/// Each field restricts the picker to conversations matching that criterion.
/// All fields default to `false` (no filter = show everything).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PickerFilter {
    /// Show only pinned conversations.
    pub pinned: bool,

    /// Show only conversations from the current session.
    pub session: bool,

    /// Pre-populate the picker's filter input with this text.
    pub query: Option<String>,
}

impl PickerFilter {
    /// Test whether a conversation passes this filter.
    fn matches(&self, c: &jp_conversation::Conversation, is_session_conversation: bool) -> bool {
        if self.pinned && !c.is_pinned() {
            return false;
        }
        if self.session && !is_session_conversation {
            return false;
        }
        true
    }
}

impl ConversationTarget {
    /// Parse a raw string into a conversation target.
    ///
    /// Empty string triggers picker mode. Recognized keywords resolve to their
    /// respective variants. Anything else is parsed as a conversation ID.
    pub(crate) fn parse(s: &str) -> Self {
        match s {
            // Interactive pickers
            "?" | "" => Self::Picker(PickerFilter::default()),
            "?p" | "?pinned" => Self::Picker(PickerFilter {
                pinned: true,
                ..Default::default()
            }),
            "?s" | "?session" => Self::Picker(PickerFilter {
                session: true,
                ..Default::default()
            }),

            // Conversation aliases (single target)
            "newest" | "n" => Self::Newest,
            "latest" | "l" => Self::Latest,
            "pinned" | "p" => Self::LatestPinned,
            "session" | "s" => Self::SessionPrevious,

            // Multi-target keywords
            "+session" | "+s" => Self::AllSession,
            "+pinned" | "+p" => Self::AllPinned,

            "help" => Self::Help,

            // Try as conversation ID, fall back to fuzzy picker.
            _ => s.parse::<ConversationId>().map_or_else(
                |_| {
                    Self::Picker(PickerFilter {
                        query: Some(s.to_owned()),
                        ..Default::default()
                    })
                },
                Self::Id,
            ),
        }
    }

    /// Whether this target requires session state to resolve.
    pub(crate) fn requires_session(&self) -> bool {
        matches!(
            self,
            Self::SessionPrevious
                | Self::AllSession
                | Self::Picker(PickerFilter { session: true, .. })
        )
    }

    /// The keyword name for this target, if it is a keyword.
    pub(crate) fn keyword_name(&self) -> Option<&'static str> {
        match self {
            Self::Picker(f) if f.pinned => Some("pinned"),
            Self::Id(_) | Self::Picker(_) | Self::Help => None,
            Self::Newest => Some("newest"),
            Self::Latest => Some("latest"),
            Self::LatestPinned => Some("pinned"),
            Self::SessionPrevious => Some("session"),
            Self::AllSession => Some("+session"),
            Self::AllPinned => Some("+pinned"),
        }
    }

    /// Resolve this target to concrete conversation IDs.
    ///
    /// Returns an empty vec for `Picker` — the caller must handle interactive
    /// selection separately. Returns multiple IDs for `AllSession`.
    pub(crate) fn resolve(
        &self,
        workspace: &Workspace,
        session: Option<&Session>,
    ) -> Result<Vec<ConversationId>> {
        match self {
            Self::Id(id) => Ok(vec![*id]),
            Self::Latest => {
                let id = workspace
                    .conversations()
                    .max_by_key(|(_, c)| c.last_activated_at)
                    .map(|(id, _)| *id)
                    .ok_or_else(|| {
                        Error::NotFound("conversation", "no conversations exist".into())
                    })?;
                Ok(vec![id])
            }
            Self::Newest => {
                let id = workspace
                    .conversations()
                    .max_by_key(|(id, _)| id.timestamp())
                    .map(|(id, _)| *id)
                    .ok_or_else(|| {
                        Error::NotFound("conversation", "no conversations exist".into())
                    })?;
                Ok(vec![id])
            }
            Self::LatestPinned => {
                let id = workspace
                    .conversations()
                    .filter(|(_, c)| c.is_pinned())
                    .max_by_key(|(_, c)| c.pinned_at)
                    .map(|(id, _)| *id)
                    .ok_or_else(|| {
                        Error::NotFound("conversation", "no pinned conversations".into())
                    })?;
                Ok(vec![id])
            }
            Self::SessionPrevious => {
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
            Self::AllSession => {
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
            Self::AllPinned => {
                let ids: Vec<_> = workspace
                    .conversations()
                    .filter(|(_, c)| c.is_pinned())
                    .map(|(id, _)| *id)
                    .collect();
                if ids.is_empty() {
                    return Err(Error::NotFound(
                        "conversation",
                        "no pinned conversations".into(),
                    ));
                }
                Ok(ids)
            }
            Self::Picker(_) | Self::Help => Ok(vec![]),
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
    multi: bool,
    supports_session: bool,
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
        if matches!(target, ConversationTarget::Help) {
            return Err(Error::TargetHelp {
                session: supports_session,
                multi,
            });
        }

        match target.resolve(workspace, session) {
            Ok(v) if v.is_empty() => {
                let filter = match target {
                    ConversationTarget::Picker(f) => f,
                    _ => &PickerFilter::default(),
                };
                return if multi {
                    resolve_multi_picker(workspace, session, filter)
                } else {
                    resolve_picker(workspace, session, filter).map(|id| vec![id])
                };
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

    resolve_picker(workspace, session, &PickerFilter::default())
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
        DefaultConversationId::LastActivated => ConversationTarget::Latest,
        DefaultConversationId::LastCreated => ConversationTarget::Newest,
        DefaultConversationId::Previous => ConversationTarget::SessionPrevious,
        DefaultConversationId::Id(id) => id.parse::<_>().ok().map(ConversationTarget::Id)?,
    };

    // Swallow resolution errors (e.g. "no conversations exist", "no previous
    // conversation") — the caller falls through to the picker instead.
    target.resolve(workspace, session).ok()?.into_iter().next()
}

fn resolve_picker(
    workspace: &Workspace,
    session: Option<&Session>,
    filter: &PickerFilter,
) -> Result<ConversationId> {
    if !io::stdin().is_terminal() {
        return Err(Error::NoConversationTarget);
    }

    pick_conversation(workspace, session, filter).ok_or(Error::NoConversationTarget)
}

fn resolve_multi_picker(
    workspace: &Workspace,
    session: Option<&Session>,
    filter: &PickerFilter,
) -> Result<Vec<ConversationId>> {
    if !io::stdin().is_terminal() {
        return Err(Error::NoConversationTarget);
    }

    let ids = pick_conversations(workspace, session, filter);
    if ids.is_empty() {
        return Err(Error::NoConversationTarget);
    }
    Ok(ids)
}

/// Build the sorted, labeled conversation list for the picker.
///
/// Pinned conversations sort first (by `pinned_at` descending), then
/// non-pinned by `last_activated_at` descending. The session's active
/// conversation is forced to the top.
fn build_picker_items(
    workspace: &Workspace,
    session: Option<&Session>,
    filter: &PickerFilter,
) -> Vec<(ConversationId, String)> {
    let session_ids: Vec<_> = session
        .map(|s| workspace.session_conversation_ids(s))
        .unwrap_or_default();

    let mut items: Vec<_> = workspace
        .conversations()
        .filter(|(id, c)| filter.matches(c, session_ids.contains(id)))
        .map(|(id, c)| {
            let label = match &c.title {
                Some(t) => format!("{id}  {t}"),
                None => id.to_string(),
            };
            (*id, label)
        })
        .collect();

    // Sort pinned conversations first (by pinned_at descending),
    // then non-pinned by last_activated_at descending.
    let meta: HashMap<_, _> = workspace
        .conversations()
        .map(|(id, c)| (*id, (c.last_activated_at, c.pinned_at)))
        .collect();

    items.sort_by(|a, b| {
        let (a_activated, a_pinned_at) = meta[&a.0];
        let (b_activated, b_pinned_at) = meta[&b.0];
        match (a_pinned_at, b_pinned_at) {
            (Some(a_pin), Some(b_pin)) => b_pin.cmp(&a_pin),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => b_activated.cmp(&a_activated),
        }
    });

    // Pin the session's active conversation to the very top.
    if let Some(s) = session
        && let Some(active) = workspace.session_active_conversation(s)
        && let Some(pos) = items.iter().position(|(id, _)| *id == active)
    {
        let entry = items.remove(pos);
        items.insert(0, entry);
    }

    items
}

/// Single-select interactive conversation picker.
fn pick_conversation(
    workspace: &Workspace,
    session: Option<&Session>,
    filter: &PickerFilter,
) -> Option<ConversationId> {
    let items = build_picker_items(workspace, session, filter);
    if items.is_empty() {
        return None;
    }

    let labels: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
    let mut writer = io::stderr();
    let mut prompt = inquire::Select::new("Select a conversation", labels);
    if let Some(query) = &filter.query {
        prompt = prompt.with_starting_filter_input(query);
    }
    let selected = prompt.prompt_with_writer(&mut writer).ok()?;

    items
        .iter()
        .find(|(_, l)| *l == selected)
        .map(|(id, _)| *id)
}

/// Multi-select interactive conversation picker.
fn pick_conversations(
    workspace: &Workspace,
    session: Option<&Session>,
    filter: &PickerFilter,
) -> Vec<ConversationId> {
    let items = build_picker_items(workspace, session, filter);
    if items.is_empty() {
        return vec![];
    }

    let labels: Vec<String> = items.iter().map(|(_, l)| l.clone()).collect();
    let mut writer = io::stderr();
    let mut prompt = inquire::MultiSelect::new("Select conversations", labels);
    if let Some(query) = &filter.query {
        prompt = prompt.with_starting_filter_input(query);
    }
    let Ok(selected) = prompt.prompt_with_writer(&mut writer) else {
        return vec![];
    };

    items
        .iter()
        .filter(|(_, l)| selected.contains(l))
        .map(|(id, _)| *id)
        .collect()
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
