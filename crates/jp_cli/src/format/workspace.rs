use std::fmt;

use camino::Utf8Path;
use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_term::table::{DetailItem, DetailRow, details};
use jp_workspace::Id;
use serde_json::json;

/// Details view for `jp w show`, mirroring the conversation [`DetailsFmt`].
///
/// [`DetailsFmt`]: super::conversation::DetailsFmt
pub struct DetailsFmt {
    /// The ID of the workspace.
    pub id: Id,

    /// The user-workspace directory slug; cosmetic display name (RFD 031).
    pub slug: Option<String>,

    /// How the subject was resolved, for the readout.
    pub resolved: &'static str,

    /// Session-level sticky state.
    /// `None` (subject is not the session-active workspace) hides the row.
    pub sticky: Option<bool>,

    /// Live checkouts, most recently used first.
    pub checkouts: Vec<DetailItem>,

    /// Union conversation count across the user-local durable store and every
    /// live checkout.
    /// `None` (no checkout could be loaded) hides the row.
    pub conversations: Option<usize>,

    /// The session's active conversation in this workspace, with its title.
    pub active_conversation: Option<(ConversationId, Option<String>)>,

    /// Pretty-print the output.
    pub pretty: bool,
}

impl DetailsFmt {
    #[must_use]
    pub fn new(id: Id, resolved: &'static str) -> Self {
        Self {
            id,
            slug: None,
            resolved,
            sticky: None,
            checkouts: vec![],
            conversations: None,
            active_conversation: None,
            pretty: true,
        }
    }

    #[must_use]
    pub fn with_slug(mut self, slug: Option<impl Into<String>>) -> Self {
        self.slug = slug.map(Into::into);
        self
    }

    #[must_use]
    pub fn with_sticky(self, sticky: Option<bool>) -> Self {
        Self { sticky, ..self }
    }

    #[must_use]
    pub fn with_checkouts(mut self, checkouts: Vec<DetailItem>) -> Self {
        self.checkouts = checkouts;
        self
    }

    #[must_use]
    pub fn with_conversations(self, conversations: Option<usize>) -> Self {
        Self {
            conversations,
            ..self
        }
    }

    #[must_use]
    pub fn with_active_conversation(
        self,
        active_conversation: Option<(ConversationId, Option<String>)>,
    ) -> Self {
        Self {
            active_conversation,
            ..self
        }
    }

    /// Use color in the output.
    #[must_use]
    pub fn with_pretty_printing(self, pretty: bool) -> Self {
        Self { pretty, ..self }
    }

    /// Return the title of the workspace: its cosmetic slug, when known.
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.slug.as_deref()
    }

    /// The stable machine-readable payload for `jp w show`.
    ///
    /// Keys are a fixed contract, deliberately decoupled from the display
    /// labels in [`Self::rows`]: labels can be reworded freely, these keys
    /// cannot change without breaking consumers.
    /// Rows the display hides serialize as `null` instead of disappearing, so
    /// consumers can rely on key presence: `sticky` is `null` when the subject
    /// is not the session-active workspace, and `conversations` is `null` when
    /// no checkout could be loaded.
    /// Timestamps are RFC 3339 in UTC.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        json!({
            "id": self.id.to_string(),
            "slug": self.slug,
            "resolved_from": self.resolved,
            "sticky": self.sticky,
            "checkouts": self
                .checkouts
                .iter()
                .map(|item| item.json.clone())
                .collect::<Vec<_>>(),
            "conversations": self.conversations,
            "active_conversation": self.active_conversation.as_ref().map(|(id, title)| {
                json!({
                    "id": id.to_string(),
                    "title": title,
                })
            }),
        })
    }

    /// Return rows for a table displaying the workspace details.
    #[must_use]
    pub fn rows(&self) -> Vec<DetailRow> {
        let mut rows = vec![];

        rows.push(self.scalar("ID", self.id.to_string()));
        rows.push(self.scalar("Resolved From", self.resolved.to_owned()));

        if let Some(sticky) = self.sticky {
            let value = if sticky && self.pretty {
                "Yes".bold().yellow().to_string()
            } else if sticky {
                "Yes".to_owned()
            } else {
                "No".to_owned()
            };
            rows.push(self.scalar("Sticky", value));
        }

        if self.checkouts.is_empty() {
            rows.push(self.scalar("Checkouts", "(no live checkouts)".to_owned()));
        } else {
            rows.push(DetailRow::list(
                self.styled_label("Checkouts"),
                self.checkouts.clone(),
            ));
        }

        if let Some(count) = self.conversations {
            rows.push(self.scalar("Conversations", count.to_string()));
        }

        if let Some((id, title)) = &self.active_conversation {
            let value = match title {
                Some(title) => format!("{id}: {title}"),
                None => id.to_string(),
            };
            rows.push(self.scalar("Active Conversation", value));
        }

        rows
    }

    /// Bold the label when pretty-printing is enabled.
    fn styled_label(&self, label: &str) -> String {
        if self.pretty {
            label.bold().to_string()
        } else {
            label.to_owned()
        }
    }

    fn scalar(&self, label: &str, value: String) -> DetailRow {
        DetailRow::scalar(self.styled_label(label), value)
    }
}

impl fmt::Display for DetailsFmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", details(self.title(), self.rows()))
    }
}

/// Build a list item for a live checkout root.
///
/// The terminal text is the path, marked `(active)` when the session's active
/// selection points at this checkout.
/// The JSON form carries the canonical `path`, the `active` flag, and the roots
/// registry's `last_used` timestamp.
pub(crate) fn checkout_detail_item(
    path: &Utf8Path,
    last_used: DateTime<Utc>,
    active: bool,
    pretty: bool,
) -> DetailItem {
    let text = match (active, pretty) {
        (true, true) => format!("{path} {}", "(active)".green().bold()),
        (true, false) => format!("{path} (active)"),
        (false, _) => path.to_string(),
    };

    DetailItem::new(
        text,
        json!({
            "path": path,
            "active": active,
            "last_used": last_used,
        }),
    )
}
