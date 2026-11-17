//! Table filtering, sorting, and tree layout.
//!
//! [`Table::display`] flattens visible rows and their expanded children into a
//! sequence of [`Display`] lines: top-level worktree and pull request rows,
//! expanded terminal tabs, session conversations, conversation operations, and
//! non-selectable [`Display::Note`] lines.

use std::{cmp::Reverse, collections::HashSet};

use chrono::{DateTime, Utc};

use crate::{
    model::{self, JpSession, Row, RowId, SessionConversation, Terminal},
    queue::QueueState,
};

/// Action that can be run on a conversation in its terminal tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Query,
    Archive,
    Delete,
}

impl Operation {
    pub const ALL: [Self; 3] = [Self::Query, Self::Archive, Self::Delete];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Query => "start query",
            Self::Archive => "archive conversation",
            Self::Delete => "delete conversation",
        }
    }

    /// Return the `jp` command for this operation on conversation `id`.
    #[must_use]
    pub fn command(self, id: &str) -> String {
        match self {
            Self::Query => format!("jp query --id={id}"),
            Self::Archive => format!("jp conversation archive {id}"),
            Self::Delete => format!("jp conversation rm {id}"),
        }
    }
}

/// Selectable item in the table tree.
#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    /// A worktree row or an unchecked-out pull request row.
    Row(&'a Row),

    /// A terminal tab under an expanded worktree row.
    Tab {
        row: &'a Row,
        tab: &'a Terminal,

        /// Zero-based index of the tab within the worktree.
        index: usize,
    },

    /// A conversation belonging to a JP session in an expanded worktree.
    Conversation {
        row: &'a Row,
        tab: Option<&'a Terminal>,
        conversation: &'a SessionConversation,
    },

    /// An [`Operation`] entry listed under an expanded conversation.
    Operation {
        row: &'a Row,
        tab: Option<&'a Terminal>,
        session: &'a JpSession,
        conversation: &'a SessionConversation,
        operation: Operation,
    },
}

impl<'a> Item<'a> {
    #[must_use]
    pub const fn row(&self) -> &'a Row {
        match *self {
            Self::Row(row)
            | Self::Tab { row, .. }
            | Self::Conversation { row, .. }
            | Self::Operation { row, .. } => row,
        }
    }

    pub(super) fn key(&self) -> Key {
        let child = match self {
            Self::Row(_) => None,
            Self::Tab { tab, .. } => Some(Child::Tab(tab.id.clone())),
            Self::Conversation {
                tab, conversation, ..
            } => Some(Child::Conversation {
                tab: tab.map(|tab| tab.id.clone()),
                id: conversation.id.clone(),
            }),
            Self::Operation {
                tab,
                conversation,
                operation,
                ..
            } => Some(Child::Operation {
                tab: tab.map(|tab| tab.id.clone()),
                id: conversation.id.clone(),
                operation: *operation,
            }),
        };
        Key {
            row: self.row().id.clone(),
            child,
        }
    }

    /// Return the [`Key`] of the conversation this item represents or belongs
    /// to.
    pub(super) fn conversation_key(&self) -> Option<Key> {
        match self {
            Self::Conversation {
                tab, conversation, ..
            }
            | Self::Operation {
                tab, conversation, ..
            } => Some(conversation_key(&self.row().id, *tab, conversation)),
            Self::Row(_) | Self::Tab { .. } => None,
        }
    }
}

fn conversation_key(
    row: &RowId,
    tab: Option<&Terminal>,
    conversation: &SessionConversation,
) -> Key {
    Key {
        row: row.clone(),
        child: Some(Child::Conversation {
            tab: tab.map(|tab| tab.id.clone()),
            id: conversation.id.clone(),
        }),
    }
}

/// Tree indentation level for lines rendered under an expanded row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// Terminal tab line (`last` is `true` for the final tab of the row).
    Tab { last: bool },

    /// Line nested under a terminal tab (`last_tab` is `true` when the parent
    /// tab is the final tab of the row).
    InTab { last_tab: bool },

    /// Line nested directly under the row without a parent tab.
    InRow,

    /// Operation line nested under a conversation (`last_tab` is `Some` when
    /// the conversation is inside a tab, `None` when directly under the row).
    Operation { last_tab: Option<bool> },
}

impl Indent {
    const fn operation(self) -> Self {
        match self {
            Self::InTab { last_tab } => Self::Operation {
                last_tab: Some(last_tab),
            },
            Self::Tab { .. } | Self::InRow | Self::Operation { .. } => {
                Self::Operation { last_tab: None }
            }
        }
    }
}

/// A rendered line in the table body.
#[derive(Debug, Clone)]
pub enum Display<'a> {
    Item {
        /// Index of this item in [`Table::items`].
        index: usize,
        item: Item<'a>,
        indent: Option<Indent>,
    },

    /// Non-selectable informational line attached to a preceding item.
    Note {
        /// Index in [`Table::items`] of the item this note belongs to.
        owner: usize,
        text: String,
        indent: Indent,
    },
}

/// Sort order for top-level table rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Worktrees alphabetically by name, followed by unchecked-out pull
    /// requests by number.
    #[default]
    Name,

    /// By branch name.
    Branch,

    /// Newest pull request first.
    PullRequest,

    /// Conversations waiting on user input first, then running turns, then most
    /// recent event.
    Activity,

    /// Failing checks first, then running, then passing.
    Checks,

    /// Queued pull requests in merge order, then held-back labeled pull
    /// requests, then the rest.
    Queue,
}

impl Sort {
    /// All sort orders in menu display order.
    pub const ALL: [Self; 6] = [
        Self::Name,
        Self::Branch,
        Self::PullRequest,
        Self::Activity,
        Self::Checks,
        Self::Queue,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Worktree name",
            Self::Branch => "Branch name",
            Self::PullRequest => "Pull request, newest first",
            Self::Activity => "Recent JP activity",
            Self::Checks => "CI, failing first",
            Self::Queue => "Merge queue order",
        }
    }

    pub(super) fn position(self) -> usize {
        Self::ALL.iter().position(|sort| *sort == self).unwrap_or(0)
    }
}

/// Substring filter applied to worktree and branch names.
#[derive(Debug)]
pub struct Filter {
    pub query: String,

    /// Whether the user is actively typing into the filter prompt.
    pub editing: bool,

    /// Previous filter query, restored if editing is cancelled.
    pub(super) previous: Option<String>,
}

/// Stable identifier for a selectable table item across refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct Key {
    pub(super) row: RowId,
    pub(super) child: Option<Child>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) enum Child {
    Tab(String),
    Conversation {
        tab: Option<String>,
        id: String,
    },
    Operation {
        tab: Option<String>,
        id: String,
        operation: Operation,
    },
}

/// Table state: rows, active filter and sort order, and expanded tree nodes.
#[derive(Debug, Default)]
pub struct Table {
    /// All rows (worktrees followed by unchecked-out pull requests).
    pub(super) rows: Vec<Row>,

    pub sort: Sort,

    /// Whether to include idle worktrees when no filter is active.
    pub show_all: bool,

    pub filter: Option<Filter>,

    /// Row IDs whose tabs and conversations are expanded.
    pub(super) expanded: HashSet<RowId>,

    /// Conversation keys whose operations are expanded.
    pub(super) opened: HashSet<Key>,
}

impl Table {
    /// Return the active non-empty filter query, if any.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.filter
            .as_ref()
            .map(|filter| filter.query.as_str())
            .filter(|query| !query.is_empty())
    }

    /// Return the visible rows in the active sort order.
    ///
    /// When a filter query is active, returns every row (including idle
    /// worktrees) whose worktree or branch name matches the query.
    /// Otherwise, returns active rows (or all rows when `show_all` is set).
    #[must_use]
    pub fn visible(&self) -> Vec<&Row> {
        let mut rows: Vec<&Row> = match self.query() {
            Some(query) => {
                let query = query.to_lowercase();
                self.rows
                    .iter()
                    .filter(|row| matches(row, &query))
                    .collect()
            }
            None => self
                .rows
                .iter()
                .filter(|row| self.show_all || row.is_active())
                .collect(),
        };
        sort(&mut rows, self.sort);
        rows
    }

    /// Build the display lines for all visible rows and their expanded
    /// children.
    #[must_use]
    pub fn display(&self) -> Vec<Display<'_>> {
        let mut lines = Lines {
            opened: &self.opened,
            lines: vec![],
            items: 0,
        };

        for row in self.visible() {
            lines.item(Item::Row(row), None);
            if self.expanded.contains(&row.id) {
                lines.children(row);
            }
        }

        lines.lines
    }

    /// Return the selectable items in display order.
    #[must_use]
    pub fn items(&self) -> Vec<Item<'_>> {
        self.display()
            .into_iter()
            .filter_map(|line| match line {
                Display::Item { item, .. } => Some(item),
                Display::Note { .. } => None,
            })
            .collect()
    }

    /// Return the total number of worktrees, whether visible or hidden.
    #[must_use]
    pub fn total(&self) -> usize {
        self.rows.iter().filter(|row| row.is_worktree()).count()
    }

    /// Return the total number of rows (worktrees and unchecked-out pull
    /// requests).
    #[must_use]
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// Open the filter input prompt, pre-filled with any existing query.
    pub(super) fn edit_filter(&mut self) {
        let previous = self.filter.take().map(|filter| filter.query);
        self.filter = Some(Filter {
            query: previous.clone().unwrap_or_default(),
            editing: true,
            previous,
        });
    }

    /// Collapse row `id`, closing any expanded conversations within it.
    pub(super) fn collapse(&mut self, id: &RowId) {
        self.expanded.remove(id);
        self.opened.retain(|key| key.row != *id);
    }
}

/// Builder that accumulates [`Display`] lines and assigns sequential item
/// indices.
struct Lines<'a> {
    /// Conversation keys whose operations are expanded.
    opened: &'a HashSet<Key>,

    lines: Vec<Display<'a>>,
    items: usize,
}

impl<'a> Lines<'a> {
    fn item(&mut self, item: Item<'a>, indent: Option<Indent>) {
        self.lines.push(Display::Item {
            index: self.items,
            item,
            indent,
        });
        self.items += 1;
    }

    /// Append a note attached to the most recently added item.
    fn note(&mut self, text: String, indent: Indent) {
        self.lines.push(Display::Note {
            owner: self.items.saturating_sub(1),
            text,
            indent,
        });
    }

    /// Append the expanded lines for `row`: its PR title, its tabs and their
    /// session conversations, and any sessions not matched to an open tab.
    fn children(&mut self, row: &'a Row) {
        if let Some(pr) = &row.pr {
            self.note(format!("#{} {}", pr.number, pr.title), Indent::InRow);
        }

        let placed = model::place(row);
        let last = row.terminals.len().saturating_sub(1);
        for (index, (tab, sessions)) in row.terminals.iter().zip(&placed.tabs).enumerate() {
            let last_tab = index == last;
            self.item(
                Item::Tab { row, tab, index },
                Some(Indent::Tab { last: last_tab }),
            );
            for session in sessions {
                self.session(row, Some(tab), session, Indent::InTab { last_tab });
            }
        }

        if !placed.other.is_empty() {
            self.note(
                "In shells no tab could be found for:".to_owned(),
                Indent::InRow,
            );
            for session in &placed.other {
                self.session(row, None, session, Indent::InRow);
            }
        }

        if row.is_worktree() && row.terminals.is_empty() && placed.other.is_empty() {
            self.note(
                "No tab is open here: enter opens one.".to_owned(),
                Indent::InRow,
            );
        }
    }

    /// Append `session`'s conversations (and any expanded operations) under
    /// `tab` or the row.
    fn session(
        &mut self,
        row: &'a Row,
        tab: Option<&'a Terminal>,
        session: &'a JpSession,
        indent: Indent,
    ) {
        for conversation in &session.conversations {
            let item = Item::Conversation {
                row,
                tab,
                conversation,
            };
            self.item(item, Some(indent));

            if !self
                .opened
                .contains(&conversation_key(&row.id, tab, conversation))
            {
                continue;
            }
            for operation in Operation::ALL {
                let item = Item::Operation {
                    row,
                    tab,
                    session,
                    conversation,
                    operation,
                };
                self.item(item, Some(indent.operation()));
            }
        }
        if session.more > 0 {
            self.note(format!("… {} older", session.more), indent);
        }
    }
}

/// Return `true` if `row`'s worktree or branch name contains the lowercase
/// `query`.
fn matches(row: &Row, query: &str) -> bool {
    row.name.to_lowercase().contains(query)
        || row
            .branch
            .as_ref()
            .is_some_and(|branch| branch.to_lowercase().contains(query))
}

/// Sort `rows` according to `by`, placing rows without the sort key last and
/// breaking ties by worktree name and PR number.
fn sort(rows: &mut [&Row], by: Sort) {
    let pr_number = |row: &Row| row.pr.as_ref().map(|pr| pr.number);
    let tiebreak = |row: &Row| (!row.is_worktree(), row.name.clone(), pr_number(row));

    match by {
        Sort::Name => rows.sort_by_key(|row| tiebreak(row)),
        Sort::Branch => {
            rows.sort_by_key(|row| (row.branch.is_none(), row.branch.clone(), tiebreak(row)));
        }
        Sort::PullRequest => {
            rows.sort_by_key(|row| {
                let number = pr_number(row);
                (number.is_none(), number.map(Reverse), tiebreak(row))
            });
        }
        Sort::Activity => {
            rows.sort_by_key(|row| {
                let activity = activity(row);
                (activity.is_none(), activity.map(Reverse), tiebreak(row))
            });
        }
        Sort::Checks => rows.sort_by_key(|row| (checks_rank(row), tiebreak(row))),
        Sort::Queue => rows.sort_by_key(|row| (queue_rank(row), tiebreak(row))),
    }
}

/// Rank a row for [`Sort::Queue`]: the front entry first, then waiting entries
/// by position, then held-back entries, then unqueued rows (ordered from bottom
/// to top within a stack).
fn queue_rank(row: &Row) -> (u8, usize, usize) {
    let Some(pr) = &row.pr else {
        return (2, 0, 0);
    };
    let in_stack = pr.stack.as_ref().map_or(0, |place| place.position);

    match &pr.queue {
        Some(QueueState::Front(_)) => (0, 0, in_stack),
        Some(QueueState::Waiting { position }) => (0, *position, in_stack),
        Some(_) => (1, 0, in_stack),
        None => (2, 0, 0),
    }
}

/// Return the activity sort key for `row`: waiting turns rank first, then
/// running turns, then most recent event timestamp.
fn activity(row: &Row) -> Option<(bool, bool, Option<DateTime<Utc>>)> {
    let waiting = row.runs.iter().any(|run| run.waiting_since.is_some());
    let running = !row.runs.is_empty();
    let last = row.last_activity();

    (running || last.is_some()).then_some((waiting, running, last))
}

/// CI status sort rank, ordered from highest priority (`Failed`) to lowest
/// (`NoPullRequest`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ChecksRank {
    Failed,
    Pending,
    Passed,
    NoChecks,
    NoPullRequest,
}

fn checks_rank(row: &Row) -> ChecksRank {
    let Some(pr) = &row.pr else {
        return ChecksRank::NoPullRequest;
    };
    let checks = pr.checks;

    if checks.failed > 0 {
        ChecksRank::Failed
    } else if checks.pending > 0 {
        ChecksRank::Pending
    } else if checks.total() > 0 {
        ChecksRank::Passed
    } else {
        ChecksRank::NoChecks
    }
}
