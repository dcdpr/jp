//! The dashboard's state and what each key and click does to it.
//!
//! [`App`] holds the latest snapshot from every source, applies each [`Update`]
//! the sources send, and turns a key press or click into an [`Action`] for the
//! shell to carry out.
//! It does no I/O.
//!
//! The table is a tree: a row per worktree (or pull request without one) and,
//! below an expanded row, its tabs, each followed by the conversations of the
//! JP sessions running in it.
//! Every row, tab, and conversation is a selectable [`Item`]; the lines between
//! them that only explain something are notes.

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
};

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::{
    create::Created,
    model::{
        self, BaseUpdate, JpSession, PullRequest, Row, RowId, SessionConversation, Terminal,
        Worktree,
    },
    source::jp::JpState,
};

/// A fresh read from one source, or the error it hit.
#[derive(Debug)]
pub enum Update {
    Worktrees(Result<Vec<Worktree>, String>),
    Terminals(Result<Vec<Terminal>, String>),
    Jp(Result<JpState, String>),
    PullRequests(Result<Vec<PullRequest>, String>),

    /// The default branch was brought up to date, or failed to be.
    Base(Result<BaseUpdate, String>),

    /// Creating a worktree finished: for pull request `pr`, or for a new branch
    /// when `None`.
    Created {
        pr: Option<u64>,
        outcome: Result<Created, String>,
    },

    /// A one-off message for the status line, such as a failed focus.
    Notice(String),
}

/// What the shell should do after a key press or click.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,

    /// Focus the terminal with this ID.
    Focus(String),

    /// Open a new terminal tab in this directory.
    OpenTab(Utf8PathBuf),

    /// Open this URL in the browser.
    Open(String),

    /// Create a worktree for the pull request, check its branch out there, and
    /// open a tab in it.
    CreateWorktree {
        number: u64,
        branch: String,
    },

    /// Create a worktree on a new branch from the default branch, and open a
    /// tab in it.
    NewWorktree {
        branch: String,
    },

    /// Focus the terminal `terminal` and run `command` at its prompt, if its
    /// shell `shell` sits idle there.
    RunInTab {
        terminal: String,
        shell: u32,
        command: String,
    },

    /// Open a tab in `dir` that runs `command`.
    RunInNewTab {
        dir: Utf8PathBuf,
        command: String,
    },
}

/// Something to do with a conversation, done by the `jp` command that does it,
/// run in the conversation's tab so its prompts and confirmations show there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    Query,
    Archive,
    Delete,
}

impl Operation {
    /// Every operation, as a conversation lists them.
    pub const ALL: [Self; 3] = [Self::Query, Self::Archive, Self::Delete];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Query => "start query",
            Self::Archive => "archive conversation",
            Self::Delete => "delete conversation",
        }
    }

    /// The command that does it to the conversation `id`.
    #[must_use]
    pub fn command(self, id: &str) -> String {
        match self {
            Self::Query => format!("jp query --id={id}"),
            Self::Archive => format!("jp conversation archive {id}"),
            Self::Delete => format!("jp conversation rm {id}"),
        }
    }
}

/// The latest state of one source.
#[derive(Debug)]
pub struct Feed<T> {
    /// The last successful read.
    /// Kept when a later read fails, so one failure does not blank the column.
    pub data: Option<T>,

    /// The error of the last read, if it failed.
    pub error: Option<String>,
}

impl<T> Default for Feed<T> {
    fn default() -> Self {
        Self {
            data: None,
            error: None,
        }
    }
}

impl<T> Feed<T> {
    fn apply(&mut self, read: Result<T, String>) {
        match read {
            Ok(data) => {
                self.data = Some(data);
                self.error = None;
            }
            Err(error) => self.error = Some(error),
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

    /// A conversation of a JP session in an expanded worktree: in one of its
    /// tabs, or in a shell no tab could be found for.
    Conversation {
        row: &'a Row,
        tab: Option<&'a Terminal>,
        conversation: &'a SessionConversation,
    },

    /// Something to do with a conversation, listed below it once opened.
    Operation {
        row: &'a Row,
        tab: Option<&'a Terminal>,
        session: &'a JpSession,
        conversation: &'a SessionConversation,
        operation: Operation,
    },
}

impl<'a> Item<'a> {
    /// The row the line belongs to.
    #[must_use]
    pub const fn row(&self) -> &'a Row {
        match *self {
            Self::Row(row)
            | Self::Tab { row, .. }
            | Self::Conversation { row, .. }
            | Self::Operation { row, .. } => row,
        }
    }

    fn key(&self) -> Key {
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
    fn conversation_key(&self) -> Option<Key> {
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
    /// The indent of an operation below a conversation indented `self`.
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
        /// The item's position in [`App::items`].
        index: usize,
        item: Item<'a>,
        indent: Option<Indent>,
    },

    /// A line that explains something about the item before it, and cannot be
    /// selected.
    Note {
        /// The position in [`App::items`] of the item the note belongs to.
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
}

/// What part of a line a click landed on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Worktree,
    PullRequest,
    Checks,
    Reviews,
    Other,
}

/// A filter on the rows, from `/`.
#[derive(Debug)]
pub struct Filter {
    pub query: String,

    /// Whether the query is still being typed.
    pub editing: bool,

    /// The query in force before this one was started, restored when typing is
    /// cancelled.
    previous: Option<String>,
}

/// Identifies a table line across refreshes, as its position does not.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    row: RowId,
    child: Option<Child>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Child {
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

#[derive(Debug, Default)]
pub struct App {
    pub worktrees: Feed<Vec<Worktree>>,
    pub terminals: Feed<Vec<Terminal>>,
    pub jp: Feed<JpState>,
    pub prs: Feed<Vec<PullRequest>>,
    pub base: Feed<BaseUpdate>,

    /// Show every worktree, not only the active ones.
    pub show_all: bool,

    /// The last one-off message, until the next key press.
    pub notice: Option<String>,

    /// Pull requests a worktree is being created for.
    pub creating: HashSet<u64>,

    pub sort: Sort,

    /// The sort menu, while open, with its highlighted entry in [`Sort::ALL`].
    pub sort_menu: Option<usize>,

    pub filter: Option<Filter>,

    /// The name of a new worktree's branch, while `n` is asking for it.
    pub new_branch: Option<String>,

    /// The current time, which relative times on screen count from.
    /// Set by the shell before every draw.
    pub now: DateTime<Utc>,

    /// Every worktree's row, then every pull request without a worktree.
    rows: Vec<Row>,

    /// Rows whose tabs and conversations are listed below them.
    expanded: HashSet<RowId>,

    /// Conversations whose operations are listed below them.
    opened: HashSet<Key>,

    /// For a row `space` closed, the line that was selected when it did, so
    /// `space` opening it again goes back there.
    last_selected: HashMap<RowId, Key>,

    /// The row `enter` last focused a tab of, and which of its tabs the next
    /// `enter` focuses.
    cycle: Option<(RowId, usize)>,

    /// The selected line, so the selection stays on its row or child as lines
    /// come and go.
    selected: Option<Key>,

    /// A row to select as soon as it shows up: a worktree being created.
    wanted: Option<RowId>,

    /// The first drawn line on screen.
    /// See [`Self::fit_scroll`].
    pub scroll: usize,

    /// The number of table lines on screen, as of the last
    /// [`Self::fit_scroll`].
    page: usize,

    /// Whether the last key was a `g` that a second one would complete.
    pending_g: bool,
}

impl App {
    /// The filter's query, when one is in force.
    #[must_use]
    pub fn query(&self) -> Option<&str> {
        self.filter
            .as_ref()
            .map(|filter| filter.query.as_str())
            .filter(|query| !query.is_empty())
    }

    /// The name of the default branch, as of its last update.
    #[must_use]
    pub fn base_branch(&self) -> &str {
        self.base
            .data
            .as_ref()
            .map_or("main", |base| base.branch.as_str())
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

    /// The lines as drawn: every visible row, followed by what it lists when it
    /// is expanded.
    #[must_use]
    pub fn display(&self) -> Vec<Display<'_>> {
        let mut lines = Lines {
            opened: &self.opened,
            lines: vec![],
            items: 0,
        };

        for row in self.visible() {
            lines.item(Item::Row(row), None);
            if !self.expanded.contains(&row.id) {
                continue;
            }

            if let Some(pr) = &row.pr {
                lines.note(format!("#{} {}", pr.number, pr.title), Indent::InRow);
            }

            let placed = model::place(row);
            let last = row.terminals.len().saturating_sub(1);
            for (index, (tab, sessions)) in row.terminals.iter().zip(&placed.tabs).enumerate() {
                let last_tab = index == last;
                lines.item(
                    Item::Tab { row, tab, index },
                    Some(Indent::Tab { last: last_tab }),
                );
                for session in sessions {
                    lines.session(row, Some(tab), session, Indent::InTab { last_tab });
                }
            }

            if !placed.other.is_empty() {
                lines.note(
                    "In shells no tab could be found for:".to_owned(),
                    Indent::InRow,
                );
                for session in &placed.other {
                    lines.session(row, None, session, Indent::InRow);
                }
            }

            if row.is_worktree() && row.terminals.is_empty() && placed.other.is_empty() {
                lines.note(
                    "No tab is open here: enter opens one.".to_owned(),
                    Indent::InRow,
                );
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

    /// The position of the selected line in [`Self::items`].
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let selected = self.selected.as_ref()?;
        self.items().iter().position(|item| item.key() == *selected)
    }

    fn selected_item(&self) -> Option<Item<'_>> {
        let selected = self.selected.as_ref()?;
        self.items()
            .into_iter()
            .find(|item| item.key() == *selected)
    }

    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Worktrees(read) => self.worktrees.apply(read),
            Update::Terminals(read) => self.terminals.apply(read),
            Update::Jp(read) => self.jp.apply(read),
            Update::PullRequests(read) => self.prs.apply(read),
            Update::Base(read) => {
                self.base.apply(read);
                return;
            }
            Update::Created { pr, outcome } => {
                if let Some(number) = pr {
                    self.creating.remove(&number);
                }
                self.notice = Some(match outcome {
                    Ok(created) => {
                        self.wanted = Some(RowId::Worktree(created.path.clone()));
                        match created.stale {
                            Some(stale) => format!(
                                "Created {}, from the local {}: {stale}.",
                                created.path,
                                self.base_branch()
                            ),
                            None => format!("Created {}.", created.path),
                        }
                    }
                    Err(error) => error,
                });
                return;
            }
            Update::Notice(notice) => {
                self.notice = Some(notice);
                return;
            }
        }

        let jp = self.jp.data.as_ref();
        self.rows = model::rows(
            self.worktrees.data.as_deref().unwrap_or_default(),
            self.terminals.data.as_deref().unwrap_or_default(),
            jp.map(|jp| jp.runs.as_slice()).unwrap_or_default(),
            jp.map(|jp| jp.sessions.as_slice()).unwrap_or_default(),
            self.prs.data.as_deref().unwrap_or_default(),
        );

        // A new worktree shows up in the list once a tab opens in it, which may
        // be a read or two after git reports it.
        if let Some(wanted) = self.wanted.take() {
            if self.visible().iter().any(|row| row.id == wanted) {
                self.selected = Some(Key {
                    row: wanted,
                    child: None,
                });
            } else {
                self.wanted = Some(wanted);
            }
        }
        self.keep_selection_visible();
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        self.notice = None;
        let cycle = self.cycle.take();

        if self.sort_menu.is_some() {
            self.sort_menu_key(key);
            return Action::None;
        }
        if self.filter.as_ref().is_some_and(|filter| filter.editing) {
            self.filter_key(key);
            return Action::None;
        }
        if self.new_branch.is_some() {
            return self.new_branch_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let pending_g = std::mem::take(&mut self.pending_g);

        match key.code {
            KeyCode::Char('c') if ctrl => return Action::Quit,
            KeyCode::Char('a') if ctrl => self.jump(false),
            KeyCode::Char('d') if ctrl => self.jump(true),
            KeyCode::Esc if self.query().is_some() => self.filter = None,
            KeyCode::Char('q') | KeyCode::Esc => return Action::Quit,
            KeyCode::Char('r') => return Action::Refresh,
            KeyCode::Down | KeyCode::Char('j') => self.step(true),
            KeyCode::Up | KeyCode::Char('k') => self.step(false),
            KeyCode::Char('g') if pending_g => self.select(0),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Home => self.select(0),
            KeyCode::Char('G') | KeyCode::End => {
                let last = self.items().len().saturating_sub(1);
                self.select(last);
            }
            KeyCode::Right | KeyCode::Char('l') => self.descend(),
            KeyCode::Left | KeyCode::Char('h') => self.ascend(),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('+' | '=') => self.expand_all(true),
            KeyCode::Char('-') => self.expand_all(false),
            KeyCode::Char('/') => {
                let previous = self.filter.take().map(|filter| filter.query);
                self.filter = Some(Filter {
                    query: previous.clone().unwrap_or_default(),
                    editing: true,
                    previous,
                });
            }
            KeyCode::Char('s') => {
                let current = Sort::ALL.iter().position(|sort| *sort == self.sort);
                self.sort_menu = Some(current.unwrap_or(0));
            }
            KeyCode::Char('n') => self.new_branch = Some(String::new()),
            KeyCode::Char('a') => {
                self.show_all = !self.show_all;
                self.keep_selection_visible();
            }
            KeyCode::Enter => return self.enter(cycle),
            KeyCode::Char('o') => return self.open(),
            _ => {}
        }

        Action::None
    }

    /// Handle a key while the sort menu is open.
    fn sort_menu_key(&mut self, key: KeyEvent) {
        let Some(highlighted) = self.sort_menu else {
            return;
        };
        let last = Sort::ALL.len() - 1;

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.sort_menu = Some(if highlighted == last {
                    0
                } else {
                    highlighted + 1
                });
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.sort_menu = Some(highlighted.checked_sub(1).unwrap_or(last));
            }
            KeyCode::Enter => self.choose_sort(highlighted),
            KeyCode::Char(c @ '1'..='9') => {
                let index = usize::from(c as u8 - b'1');
                if index <= last {
                    self.choose_sort(index);
                }
            }
            KeyCode::Esc | KeyCode::Char('s' | 'q') => self.sort_menu = None,
            _ => {}
        }
    }

    fn choose_sort(&mut self, index: usize) {
        self.sort = Sort::ALL[index];
        self.sort_menu = None;
    }

    /// Handle a key while a filter is being typed: characters narrow the rows
    /// as they are typed, `enter` keeps the filter, and `esc` puts back the one
    /// before it.
    fn filter_key(&mut self, key: KeyEvent) {
        let Some(filter) = &mut self.filter else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => {
                let previous = filter.previous.take();
                self.filter = previous.map(|query| Filter {
                    query,
                    editing: false,
                    previous: None,
                });
            }
            KeyCode::Char('c') if ctrl => {
                let previous = filter.previous.take();
                self.filter = previous.map(|query| Filter {
                    query,
                    editing: false,
                    previous: None,
                });
            }
            KeyCode::Enter if filter.query.is_empty() => self.filter = None,
            KeyCode::Enter => {
                filter.editing = false;
                filter.previous = None;
            }
            KeyCode::Backspace => {
                filter.query.pop();
            }
            KeyCode::Char(c) if !ctrl => filter.query.push(c),
            _ => {}
        }

        self.keep_selection_visible();
    }

    /// Handle a key while `n` asks for a new worktree's branch name.
    fn new_branch_key(&mut self, key: KeyEvent) -> Action {
        let Some(name) = &mut self.new_branch else {
            return Action::None;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => self.new_branch = None,
            KeyCode::Char('c') if ctrl => self.new_branch = None,
            KeyCode::Enter => {
                let branch = name.trim().to_owned();
                self.new_branch = None;
                if branch.is_empty() {
                    return Action::None;
                }
                self.notice = Some(format!(
                    "Creating a worktree for {branch} from {}…",
                    self.base_branch()
                ));
                return Action::NewWorktree { branch };
            }
            KeyCode::Backspace => {
                name.pop();
            }
            // Branch names hold no spaces; typing one is almost certainly
            // meant as a separator.
            KeyCode::Char(' ') if !ctrl => name.push('-'),
            KeyCode::Char(c) if !ctrl => name.push(c),
            _ => {}
        }

        Action::None
    }

    /// `→`: open the selected row or conversation, or step into one that is
    /// open already.
    fn descend(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };

        let (opened, first_child) = match item {
            Item::Row(row) => {
                let id = row.id.clone();
                let opened = self.expanded.insert(id.clone());
                let first = self
                    .items()
                    .iter()
                    .find(|item| !matches!(item, Item::Row(_)) && item.row().id == id)
                    .map(Item::key);
                (opened, first)
            }
            Item::Conversation { .. } => {
                let Some(key) = item.conversation_key() else {
                    return;
                };
                let opened = self.opened.insert(key.clone());
                let first = self
                    .items()
                    .iter()
                    .find(|item| {
                        matches!(item, Item::Operation { .. })
                            && item.conversation_key().as_ref() == Some(&key)
                    })
                    .map(Item::key);
                (opened, first)
            }
            Item::Tab { .. } | Item::Operation { .. } => return,
        };

        if !opened && first_child.is_some() {
            self.selected = first_child;
        }
    }

    /// `←`: close what the selection sits in, one level at a time: an
    /// operation's list, then the row.
    fn ascend(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let id = item.row().id.clone();
        let conversation = item.conversation_key();

        match item {
            Item::Row(_) => self.collapse(&id),
            Item::Operation { .. } => {
                if let Some(key) = conversation {
                    self.opened.remove(&key);
                    self.selected = Some(key);
                }
            }
            Item::Conversation { .. }
                if conversation
                    .as_ref()
                    .is_some_and(|key| self.opened.contains(key)) =>
            {
                if let Some(key) = conversation {
                    self.opened.remove(&key);
                }
            }
            Item::Tab { .. } | Item::Conversation { .. } => {
                self.selected = Some(Key {
                    row: id,
                    child: None,
                });
            }
        }
    }

    /// `space`: close the selected line's row, selecting the row, or open it
    /// again, selecting the line that was selected when it closed.
    ///
    /// Closing this way keeps the row's open conversations open, so that line
    /// is there to go back to.
    fn toggle(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let id = item.row().id.clone();
        let row = Key {
            row: id.clone(),
            child: None,
        };

        if self.expanded.remove(&id) {
            if let Some(selected) = self.selected.take() {
                self.last_selected.insert(id, selected);
            }
            self.selected = Some(row);
            return;
        }

        self.expanded.insert(id.clone());
        let back = self
            .last_selected
            .remove(&id)
            .filter(|key| self.items().iter().any(|item| item.key() == *key));
        self.selected = Some(back.unwrap_or(row));
    }

    /// Stop listing the row's tabs and conversations, close whatever
    /// conversations of it were open, and forget where `space` left off.
    fn collapse(&mut self, id: &RowId) {
        self.expanded.remove(id);
        self.opened.retain(|key| key.row != *id);
        self.last_selected.remove(id);
    }

    /// Run the selected operation in the conversation's tab: the tab it
    /// certainly runs in, or a new tab in its shell's directory when that is
    /// not certain.
    fn run(&mut self) -> Action {
        let (action, notice) = {
            let Some(Item::Operation {
                row,
                tab,
                session,
                conversation,
                operation,
            }) = self.selected_item()
            else {
                return Action::None;
            };

            let command = operation.command(&conversation.id);
            match tab.filter(|tab| row.owns(tab, session)) {
                Some(tab) => (
                    Action::RunInTab {
                        terminal: tab.id.clone(),
                        shell: session.shell,
                        command: command.clone(),
                    },
                    format!("Running `{command}` in its tab."),
                ),
                None => (
                    Action::RunInNewTab {
                        dir: session.cwd.clone(),
                        command: command.clone(),
                    },
                    format!("Running `{command}` in a new tab."),
                ),
            }
        };

        self.notice = Some(notice);
        action
    }

    /// Expand every visible row (or collapse every row).
    fn expand_all(&mut self, open: bool) {
        if open {
            let ids: Vec<RowId> = self
                .visible()
                .into_iter()
                .map(|row| row.id.clone())
                .collect();
            self.expanded.extend(ids);
        } else {
            self.expanded.clear();
            self.opened.clear();
            self.last_selected.clear();
        }
        self.keep_selection_visible();
    }

    /// Act on the selected line: focus a tab, open one, or create a worktree.
    ///
    /// On a worktree with several tabs, the first `enter` focuses its first
    /// tab, and each `enter` after it, with the selection left where it is, the
    /// next.
    fn enter(&mut self, cycle: Option<(RowId, usize)>) -> Action {
        enum Outcome {
            Focus(String, Option<(RowId, usize)>, Option<String>),
            OpenTab(Utf8PathBuf, String),
            Create(u64, String),
            Notice(String),
        }

        let outcome = match self.selected_item() {
            None => return Action::None,
            Some(Item::Operation { .. }) => return self.run(),
            Some(Item::Tab { tab, .. }) => Outcome::Focus(tab.id.clone(), None, None),
            Some(Item::Conversation { tab: Some(tab), .. }) => {
                Outcome::Focus(tab.id.clone(), None, None)
            }
            Some(Item::Conversation { tab: None, .. }) => {
                Outcome::Notice("No tab could be found for this conversation's shell.".to_owned())
            }
            Some(Item::Row(row)) => match (&row.id, row.terminals.as_slice()) {
                (RowId::PullRequest(number), _) if self.creating.contains(number) => {
                    Outcome::Notice(format!("Already creating a worktree for #{number}."))
                }
                (RowId::PullRequest(number), _) => Outcome::Create(
                    *number,
                    row.branch.clone().unwrap_or_else(|| format!("pr-{number}")),
                ),
                (RowId::Worktree(path), []) => {
                    Outcome::OpenTab(path.clone(), format!("Opening a tab in {}…", row.name))
                }
                (_, [only]) => Outcome::Focus(only.id.clone(), None, None),
                (_, tabs) => {
                    let next = cycle
                        .filter(|(id, _)| *id == row.id)
                        .map_or(0, |(_, next)| next % tabs.len());
                    Outcome::Focus(
                        tabs[next].id.clone(),
                        Some((row.id.clone(), next + 1)),
                        Some(format!(
                            "Tab {} of {} in {}: enter again for the next.",
                            next + 1,
                            tabs.len(),
                            row.name
                        )),
                    )
                }
            },
        };

        match outcome {
            Outcome::Focus(id, cycle, notice) => {
                self.cycle = cycle;
                self.notice = notice;
                Action::Focus(id)
            }
            Outcome::OpenTab(path, notice) => {
                self.notice = Some(notice);
                Action::OpenTab(path)
            }
            Outcome::Create(number, branch) => {
                self.creating.insert(number);
                self.notice = Some(format!("Creating a worktree for #{number}…"));
                Action::CreateWorktree { number, branch }
            }
            Outcome::Notice(notice) => {
                self.notice = Some(notice);
                Action::None
            }
        }
    }

    fn open(&mut self) -> Action {
        let Some(item) = self.selected_item() else {
            return Action::None;
        };
        let row = item.row();

        if let Some(pr) = &row.pr {
            return Action::Open(pr.url.clone());
        }

        let notice = format!("{} has no open pull request.", row.name);
        self.notice = Some(notice);
        Action::None
    }

    /// Scroll so the selected line is among the `rows` lines on screen, without
    /// leaving lines empty past the end of the table.
    pub fn fit_scroll(&mut self, rows: usize) {
        self.page = rows;
        let (count, line) = {
            let display = self.display();
            let selected = self.selected_index();
            let line = selected.and_then(|selected| {
                display.iter().position(
                    |line| matches!(line, Display::Item { index, .. } if *index == selected),
                )
            });
            (display.len(), line)
        };

        if let Some(line) = line {
            if rows > 0 && line >= self.scroll + rows {
                self.scroll = line + 1 - rows;
            }
            if line < self.scroll {
                self.scroll = line;
            }
        }

        self.scroll = self.scroll.min(count.saturating_sub(rows));
    }

    /// Move the selection one line down (or up) for a turn of the scroll wheel,
    /// stopping at either end rather than wrapping.
    pub fn wheel(&mut self, down: bool) {
        let Some(last) = self.items().len().checked_sub(1) else {
            return;
        };

        let index = self.selected_index().map_or(0, |index| {
            if down {
                (index + 1).min(last)
            } else {
                index.saturating_sub(1)
            }
        });
        self.select(index);
    }

    /// Act on a click on drawn line `line` (a position in [`Self::display`]),
    /// at `target`.
    ///
    /// Every click selects its line (a note selects the item it belongs to).
    /// A click on a row's worktree acts as `enter`, on a tab focuses it, and on
    /// the pull request, CI, or review cell opens that page of the pull
    /// request.
    pub fn click(&mut self, line: usize, target: Target) -> Action {
        self.notice = None;
        self.pending_g = false;
        let cycle = self.cycle.take();

        let (index, on_item) = match self.display().get(line) {
            Some(Display::Item { index, .. }) => (*index, true),
            Some(Display::Note { owner, .. }) => (*owner, false),
            None => return Action::None,
        };
        self.select(index);
        if !on_item {
            return Action::None;
        }

        let Some(item) = self.selected_item() else {
            return Action::None;
        };
        let url = item.row().pr.as_ref().map(|pr| pr.url.clone());

        match (item, target) {
            (Item::Tab { tab, .. }, _) => Action::Focus(tab.id.clone()),
            (Item::Operation { .. }, _) => self.run(),
            (Item::Conversation { .. }, _) | (Item::Row(_), Target::Other) => Action::None,
            (Item::Row(_), Target::Worktree) => self.enter(cycle),
            (Item::Row(_), Target::PullRequest) => url.map_or(Action::None, Action::Open),
            (Item::Row(_), Target::Checks) => {
                url.map_or(Action::None, |url| Action::Open(format!("{url}/checks")))
            }
            (Item::Row(_), Target::Reviews) => {
                url.map_or(Action::None, |url| Action::Open(format!("{url}/changes")))
            }
        }
    }

    /// Select the line at `index` in [`Self::items`], when there is one.
    fn select(&mut self, index: usize) {
        let key = self.items().get(index).map(Item::key);
        if key.is_some() {
            self.selected = key;
        }
    }

    /// Move the selection one line down (or up), wrapping from the last line to
    /// the first and back.
    fn step(&mut self, down: bool) {
        let Some(last) = self.items().len().checked_sub(1) else {
            return;
        };

        let index = match self.selected_index() {
            None => 0,
            Some(index) if down => {
                if index == last {
                    0
                } else {
                    index + 1
                }
            }
            Some(index) => index.checked_sub(1).unwrap_or(last),
        };
        self.select(index);
    }

    /// Move the selection half a screen down (or up), stopping at either end.
    fn jump(&mut self, down: bool) {
        let Some(last) = self.items().len().checked_sub(1) else {
            return;
        };

        let by = (self.page / 2).max(1);
        let index = self.selected_index().map_or(0, |index| {
            if down {
                (index + by).min(last)
            } else {
                index.saturating_sub(by)
            }
        });
        self.select(index);
    }

    /// Repair the selection when its line is gone or hidden.
    ///
    /// A child that is gone falls back to its row, and a pull request that got
    /// a worktree follows it to the worktree's row.
    /// Anything else falls back to the first line.
    fn keep_selection_visible(&mut self) {
        if self.selected_index().is_some() {
            return;
        }

        let items = self.items();
        let fallback = self.selected.as_ref().and_then(|key| {
            items
                .iter()
                .find(|item| match (item, &key.row) {
                    (Item::Row(row), RowId::PullRequest(number)) => {
                        row.pr.as_ref().is_some_and(|pr| pr.number == *number)
                    }
                    (Item::Row(row), id) => row.id == *id,
                    _ => false,
                })
                .map(Item::key)
        });
        let fallback = fallback.or_else(|| items.first().map(Item::key));
        drop(items);

        self.selected = fallback;
    }
}

/// The drawn lines being built, numbering items as they are added.
struct Lines<'a> {
    /// Conversations whose operations are listed below them.
    opened: &'a HashSet<Key>,

    lines: Vec<Display<'a>>,

    /// The number of items added so far.
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

/// Order `rows` by `by`.
/// Rows the order says nothing about (no branch, no pull request, no activity)
/// go last, and ties are broken by worktree name, with pull requests without a
/// worktree after the worktrees, by number.
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
    let last = row
        .sessions
        .iter()
        .flat_map(|session| &session.conversations)
        .filter_map(|conversation| conversation.last_event_at)
        .max();

    (running || last.is_some()).then_some((waiting, running, last))
}

/// Failing checks first, then running, passing, none, and no pull request.
fn checks_rank(row: &Row) -> u8 {
    let Some(pr) = &row.pr else {
        return 4;
    };
    let checks = pr.checks;

    if checks.failed > 0 {
        0
    } else if checks.pending > 0 {
        1
    } else if checks.total() > 0 {
        2
    } else {
        3
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
