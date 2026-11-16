//! The dashboard's state and what each key and click does to it.
//!
//! [`App`] holds the latest snapshot from every source in its [`Feeds`],
//! applies each [`Update`] the sources send, and turns a key press or click
//! into an [`Action`] for the shell to carry out.
//! It does no I/O.
//!
//! The rows and how they are laid out as lines are its [`Table`]; which line is
//! selected, and how far the table is scrolled, its [`Cursor`].

mod cursor;
mod table;

use std::{collections::HashSet, mem};

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use self::table::Key;
pub use self::{
    cursor::Cursor,
    table::{Display, Filter, Indent, Item, Operation, Sort, Table},
};
use crate::{
    create::Created,
    model::{self, BaseUpdate, PullRequest, Row, RowId, Terminal, Worktree},
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

/// The latest state of every source.
#[derive(Debug, Default)]
pub struct Feeds {
    pub worktrees: Feed<Vec<Worktree>>,
    pub terminals: Feed<Vec<Terminal>>,
    pub jp: Feed<JpState>,
    pub prs: Feed<Vec<PullRequest>>,
    pub base: Feed<BaseUpdate>,
}

impl Feeds {
    /// The name of the default branch, as of its last update.
    #[must_use]
    pub fn base_branch(&self) -> &str {
        self.base
            .data
            .as_ref()
            .map_or("main", |base| base.branch.as_str())
    }

    /// The table's rows, joined from the last successful read of each source.
    fn rows(&self) -> Vec<Row> {
        let jp = self.jp.data.as_ref();
        model::rows(
            self.worktrees.data.as_deref().unwrap_or_default(),
            self.terminals.data.as_deref().unwrap_or_default(),
            jp.map(|jp| jp.runs.as_slice()).unwrap_or_default(),
            jp.map(|jp| jp.sessions.as_slice()).unwrap_or_default(),
            jp.map(|jp| jp.recent.as_slice()).unwrap_or_default(),
            self.prs.data.as_deref().unwrap_or_default(),
        )
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

#[derive(Debug, Default)]
pub struct App {
    pub feeds: Feeds,
    pub table: Table,
    pub cursor: Cursor,

    /// The last one-off message, until the next key press.
    pub notice: Option<String>,

    /// Pull requests a worktree is being created for.
    pub creating: HashSet<u64>,

    /// The sort menu, while open, with its highlighted entry in [`Sort::ALL`].
    pub sort_menu: Option<usize>,

    /// The name of a new worktree's branch, while `n` is asking for it.
    pub new_branch: Option<String>,

    /// The current time, which relative times on screen count from.
    /// Set by the shell before every draw.
    pub now: DateTime<Utc>,

    /// The row `enter` last focused a tab of, and which of its tabs the next
    /// `enter` focuses.
    cycle: Option<(RowId, usize)>,

    /// Whether the last key was a `g` that a second one would complete.
    pending_g: bool,
}

impl App {
    /// The position of the selected line in [`Table::items`].
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        self.cursor.index(&self.table.items())
    }

    fn selected_item(&self) -> Option<Item<'_>> {
        self.cursor.item(&self.table.items())
    }

    pub fn apply(&mut self, update: Update) {
        match update {
            Update::Worktrees(read) => self.feeds.worktrees.apply(read),
            Update::Terminals(read) => self.feeds.terminals.apply(read),
            Update::Jp(read) => self.feeds.jp.apply(read),
            Update::PullRequests(read) => self.feeds.prs.apply(read),
            Update::Base(read) => {
                self.feeds.base.apply(read);
                return;
            }
            Update::Created { pr, outcome } => {
                if let Some(number) = pr {
                    self.creating.remove(&number);
                }
                self.notice = Some(match outcome {
                    Ok(created) => {
                        self.cursor.wanted = Some(RowId::Worktree(created.path.clone()));
                        match created.stale {
                            Some(stale) => format!(
                                "Created {}, from the local {}: {stale}.",
                                created.path,
                                self.feeds.base_branch()
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

        self.table.rows = self.feeds.rows();

        // A new worktree shows up in the list once a tab opens in it, which may
        // be a read or two after git reports it.
        self.cursor.follow(&self.table.visible());
        self.cursor.keep_visible(&self.table.items());
    }

    pub fn handle(&mut self, key: KeyEvent) -> Action {
        self.notice = None;
        let cycle = self.cycle.take();

        if self.sort_menu.is_some() {
            self.sort_menu_key(key);
            return Action::None;
        }
        if self
            .table
            .filter
            .as_ref()
            .is_some_and(|filter| filter.editing)
        {
            self.filter_key(key);
            return Action::None;
        }
        if self.new_branch.is_some() {
            return self.new_branch_key(key);
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let pending_g = mem::take(&mut self.pending_g);

        match key.code {
            KeyCode::Char('c') if ctrl => Action::Quit,
            KeyCode::Char('q') => Action::Quit,
            // With a filter in force, `esc` clears it instead.
            KeyCode::Esc if self.table.query().is_none() => Action::Quit,
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Enter => self.enter(cycle),
            KeyCode::Char('o') => self.open(),
            _ => {
                self.browse(key, pending_g);
                Action::None
            }
        }
    }

    /// Handle a key that moves the selection, expands or collapses rows, or
    /// opens a prompt or the sort menu.
    ///
    /// `pending_g` is whether the key before was a `g` that a second one would
    /// complete.
    fn browse(&mut self, key: KeyEvent, pending_g: bool) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Char('a') if ctrl => self.cursor.jump(&self.table.items(), false),
            KeyCode::Char('d') if ctrl => self.cursor.jump(&self.table.items(), true),
            KeyCode::Esc if self.table.query().is_some() => self.table.filter = None,
            KeyCode::Down | KeyCode::Char('j') => self.cursor.step(&self.table.items(), true),
            KeyCode::Up | KeyCode::Char('k') => self.cursor.step(&self.table.items(), false),
            KeyCode::Char('g') if pending_g => self.cursor.select(&self.table.items(), 0),
            KeyCode::Char('g') => self.pending_g = true,
            KeyCode::Home => self.cursor.select(&self.table.items(), 0),
            KeyCode::Char('G') | KeyCode::End => self.cursor.select_last(&self.table.items()),
            KeyCode::Right | KeyCode::Char('l') => self.descend(),
            KeyCode::Left | KeyCode::Char('h') => self.ascend(),
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('+' | '=') => self.expand_all(true),
            KeyCode::Char('-') => self.expand_all(false),
            KeyCode::Char('/') => self.table.edit_filter(),
            KeyCode::Char('s') => self.sort_menu = Some(self.table.sort.position()),
            KeyCode::Char('n') => self.new_branch = Some(String::new()),
            KeyCode::Char('a') => {
                self.table.show_all = !self.table.show_all;
                self.cursor.keep_visible(&self.table.items());
            }
            _ => {}
        }
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
        self.table.sort = Sort::ALL[index];
        self.sort_menu = None;
    }

    /// Handle a key event while editing the filter query.
    fn filter_key(&mut self, key: KeyEvent) {
        let Some(filter) = &mut self.table.filter else {
            return;
        };
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Esc => {
                let previous = filter.previous.take();
                self.table.filter = previous.map(|query| Filter {
                    query,
                    editing: false,
                    previous: None,
                });
            }
            KeyCode::Char('c') if ctrl => {
                let previous = filter.previous.take();
                self.table.filter = previous.map(|query| Filter {
                    query,
                    editing: false,
                    previous: None,
                });
            }
            KeyCode::Enter if filter.query.is_empty() => self.table.filter = None,
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

        self.cursor.keep_visible(&self.table.items());
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
                    self.feeds.base_branch()
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
                let opened = self.table.expanded.insert(id.clone());
                let first = self
                    .table
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
                let opened = self.table.opened.insert(key.clone());
                let first = self
                    .table
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
            self.cursor.selected = first_child;
        }
    }

    /// `←`: collapse or step out of the current selection one level at a time.
    fn ascend(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let id = item.row().id.clone();
        let conversation = item.conversation_key();

        match item {
            Item::Row(_) => {
                self.table.collapse(&id);
                self.cursor.last_selected.remove(&id);
            }
            Item::Operation { .. } => {
                if let Some(key) = conversation {
                    self.table.opened.remove(&key);
                    self.cursor.selected = Some(key);
                }
            }
            Item::Conversation { .. }
                if conversation
                    .as_ref()
                    .is_some_and(|key| self.table.opened.contains(key)) =>
            {
                if let Some(key) = conversation {
                    self.table.opened.remove(&key);
                }
            }
            Item::Tab { .. } | Item::Conversation { .. } => {
                self.cursor.selected = Some(Key {
                    row: id,
                    child: None,
                });
            }
        }
    }

    /// `Space`: toggle expansion of the selected item's row, preserving and
    /// restoring the inner selection.
    fn toggle(&mut self) {
        let Some(item) = self.selected_item() else {
            return;
        };
        let id = item.row().id.clone();
        let row = Key {
            row: id.clone(),
            child: None,
        };

        if self.table.expanded.remove(&id) {
            if let Some(selected) = self.cursor.selected.take() {
                self.cursor.last_selected.insert(id, selected);
            }
            self.cursor.selected = Some(row);
            return;
        }

        self.table.expanded.insert(id.clone());
        let back = self
            .cursor
            .last_selected
            .remove(&id)
            .filter(|key| self.table.items().iter().any(|item| item.key() == *key));
        self.cursor.selected = Some(back.unwrap_or(row));
    }

    /// Run the selected operation in the session's tab if unambiguously
    /// matched, or in a new tab otherwise.
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
                    format!("Running `{command}` in its tab, after clearing its prompt."),
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
                .table
                .visible()
                .into_iter()
                .map(|row| row.id.clone())
                .collect();
            self.table.expanded.extend(ids);
        } else {
            self.table.expanded.clear();
            self.table.opened.clear();
            self.cursor.last_selected.clear();
        }
        self.cursor.keep_visible(&self.table.items());
    }

    /// Execute the primary action for the selected item: focus or open a tab,
    /// run an operation, or create a worktree.
    ///
    /// Pressing `Enter` repeatedly on a worktree with multiple tabs cycles
    /// through its tabs.
    fn enter(&mut self, cycle: Option<(RowId, usize)>) -> Action {
        let outcome = match self.selected_item() {
            None => return Action::None,
            Some(Item::Operation { .. }) => return self.run(),
            Some(Item::Tab { tab, .. }) => Enter::Focus(tab.id.clone(), None, None),
            Some(Item::Conversation { tab: Some(tab), .. }) => {
                Enter::Focus(tab.id.clone(), None, None)
            }
            Some(Item::Conversation { tab: None, .. }) => {
                Enter::Notice("No tab could be found for this conversation's shell.".to_owned())
            }
            Some(Item::Row(row)) => enter_row(row, &self.creating, cycle),
        };

        match outcome {
            Enter::Focus(id, cycle, notice) => {
                self.cycle = cycle;
                self.notice = notice;
                Action::Focus(id)
            }
            Enter::OpenTab(path, notice) => {
                self.notice = Some(notice);
                Action::OpenTab(path)
            }
            Enter::Create(number, branch) => {
                self.creating.insert(number);
                self.notice = Some(format!("Creating a worktree for #{number}…"));
                Action::CreateWorktree { number, branch }
            }
            Enter::Notice(notice) => {
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
        self.cursor.fit(&self.table.display(), rows);
    }

    /// Move the selection one line down (or up) for a turn of the scroll wheel,
    /// stopping at either end rather than wrapping.
    pub fn wheel(&mut self, down: bool) {
        self.cursor.wheel(&self.table.items(), down);
    }

    /// Act on a click on drawn line `line` (a position in [`Table::display`]),
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

        let (index, on_item) = match self.table.display().get(line) {
            Some(Display::Item { index, .. }) => (*index, true),
            Some(Display::Note { owner, .. }) => (*owner, false),
            None => return Action::None,
        };
        self.cursor.select(&self.table.items(), index);
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
}

/// What `enter` does on the selected item.
enum Enter {
    /// Focus the terminal with this ID, then remember this tab cycle and show
    /// this notice.
    Focus(String, Option<(RowId, usize)>, Option<String>),

    /// Open a tab in this directory, showing this notice.
    OpenTab(Utf8PathBuf, String),

    /// Create a worktree for this pull request, on this branch.
    Create(u64, String),

    Notice(String),
}

/// What `enter` does on `row`.
///
/// A pull request gets a worktree, unless one is in `creating` already.
/// A worktree without a tab gets one, and one with a tab focuses it.
/// One with several focuses each in turn, continuing the tab cycle `cycle` when
/// it is the row's.
fn enter_row(row: &Row, creating: &HashSet<u64>, cycle: Option<(RowId, usize)>) -> Enter {
    match (&row.id, row.terminals.as_slice()) {
        (RowId::PullRequest(number), _) if creating.contains(number) => {
            Enter::Notice(format!("Already creating a worktree for #{number}."))
        }
        (RowId::PullRequest(number), _) => Enter::Create(
            *number,
            row.branch.clone().unwrap_or_else(|| format!("pr-{number}")),
        ),
        (RowId::Worktree(path), []) => {
            Enter::OpenTab(path.clone(), format!("Opening a tab in {}…", row.name))
        }
        (_, [only]) => Enter::Focus(only.id.clone(), None, None),
        (_, tabs) => {
            let next = cycle
                .filter(|(id, _)| *id == row.id)
                .map_or(0, |(_, next)| next % tabs.len());
            Enter::Focus(
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
    }
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
