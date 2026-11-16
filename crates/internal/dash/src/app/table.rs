//! The rows of the table, which of them are shown and in what order, and the
//! lines they are drawn as.
//!
//! [`Table::display`] lays the shown rows out as a tree: a line per row and,
//! below an expanded row, its tabs, each followed by the conversations of the
//! JP sessions running in it.
//! Every row, tab, and conversation is a selectable [`Item`]; the lines between
//! them that only explain something are notes.

use std::{cmp::Reverse, collections::HashSet};

use chrono::{DateTime, Utc};

use crate::model::{self, JpSession, Row, RowId, SessionConversation, Terminal};

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

/// One selectable line of the table.
#[derive(Debug, Clone, Copy)]
pub enum Item<'a> {
    /// A worktree, or a pull request without one.
    Row(&'a Row),

    /// One of an expanded worktree's tabs.
    Tab {
        row: &'a Row,
        tab: &'a Terminal,

        /// The tab's position among the worktree's tabs, from zero.
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

    /// The key of the conversation an operation belongs to.
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

/// Where a line below a row sits in the tree, which decides the guide lines
/// drawn in front of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Indent {
    /// A tab, the last of its row's or not.
    Tab { last: bool },

    /// A line below a tab, the last of its row's or not.
    InTab { last_tab: bool },

    /// A line below the row, outside any tab.
    InRow,

    /// An operation below a conversation: in a tab (the last of its row's or
    /// not), or outside any tab.
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

/// One line of the table as drawn.
#[derive(Debug, Clone)]
pub enum Display<'a> {
    Item {
        /// The item's position in [`Table::items`].
        index: usize,
        item: Item<'a>,
        indent: Option<Indent>,
    },

    /// A line that explains something about the item before it, and cannot be
    /// selected.
    Note {
        /// The position in [`Table::items`] of the item the note belongs to.
        owner: usize,
        text: String,
        indent: Indent,
    },
}

/// The order rows are listed in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Sort {
    /// Worktrees by name, then pull requests without one by number.
    #[default]
    Name,

    /// By branch name.
    Branch,

    /// Newest pull request first.
    PullRequest,

    /// Waiting on the user first, then running, then by the most recent
    /// conversation event.
    Activity,

    /// Failing checks first, then running, then passing.
    Checks,
}

impl Sort {
    /// Every order, as the sort menu lists them.
    pub const ALL: [Self; 5] = [
        Self::Name,
        Self::Branch,
        Self::PullRequest,
        Self::Activity,
        Self::Checks,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Name => "Worktree name",
            Self::Branch => "Branch name",
            Self::PullRequest => "Pull request, newest first",
            Self::Activity => "Recent JP activity",
            Self::Checks => "CI, failing first",
        }
    }

    /// The order's position in [`Self::ALL`].
    pub(super) fn position(self) -> usize {
        Self::ALL.iter().position(|sort| *sort == self).unwrap_or(0)
    }
}

/// A filter on the rows, from `/`.
#[derive(Debug)]
pub struct Filter {
    pub query: String,

    /// Whether the query is still being typed.
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

/// The rows, which of them are shown and in what order, and which are expanded.
#[derive(Debug, Default)]
pub struct Table {
    /// Every worktree's row, then every pull request without a worktree.
    pub(super) rows: Vec<Row>,

    pub sort: Sort,

    /// Show every worktree, not only the active ones.
    pub show_all: bool,

    pub filter: Option<Filter>,

    /// Rows whose tabs and conversations are listed below them.
    pub(super) expanded: HashSet<RowId>,

    /// Conversations whose operations are listed below them.
    pub(super) opened: HashSet<Key>,
}

impl Table {
    /// The filter's query, when one is in force.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.filter
            .as_ref()
            .map(|filter| filter.query.as_str())
            .filter(|query| !query.is_empty())
    }

    /// The rows on screen, in the chosen order.
    ///
    /// With a filter, every row whose worktree or branch name contains the
    /// query, idle worktrees included.
    /// Without one, the active worktrees (or every worktree with `show_all`)
    /// and every pull request without a worktree.
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

    /// The selectable lines, in the order they are drawn.
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

    /// The number of worktrees, shown or not.
    #[must_use]
    pub fn total(&self) -> usize {
        self.rows.iter().filter(|row| row.is_worktree()).count()
    }

    /// The number of rows, worktrees and pull requests, shown or not.
    #[must_use]
    pub fn total_rows(&self) -> usize {
        self.rows.len()
    }

    /// Start editing the filter, from the query in force.
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

/// The drawn lines being built, numbering items as they are added.
struct Lines<'a> {
    /// Conversations whose operations are listed below them.
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

    /// A note, belonging to the item added last.
    fn note(&mut self, text: String, indent: Indent) {
        self.lines.push(Display::Note {
            owner: self.items.saturating_sub(1),
            text,
            indent,
        });
    }

    /// The lines below an expanded row: its pull request, its tabs, each
    /// followed by the conversations in it, and the conversations of shells no
    /// tab could be found for.
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

    /// A session's conversations, in `tab` or in no tab.
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

/// Whether the row's worktree or branch name contains `query`, which is lower
/// case.
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
    }
}

/// How urgent and recent the row's JP activity is: a turn waiting on the user
/// beats a running one, which beats every recorded event.
fn activity(row: &Row) -> Option<(bool, bool, Option<DateTime<Utc>>)> {
    let waiting = row.runs.iter().any(|run| run.waiting_since.is_some());
    let running = !row.runs.is_empty();
    let last = row.last_activity();

    (running || last.is_some()).then_some((waiting, running, last))
}

/// Where a row goes when sorting by CI, first to last.
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
