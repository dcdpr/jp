//! Dashboard application state and input handling.
//!
//! [`App`] stores the latest snapshot from each source in [`Feeds`], applies
//! incoming [`Update`]s, and translates key presses and mouse clicks into
//! [`Action`]s for the event loop to execute.
//! It performs no I/O.
//!
//! [`Table`] manages row filtering, sorting, and tree expansion; [`Cursor`]
//! tracks the selected item and scroll offset.

mod cursor;
mod queue_view;
mod table;

use std::{collections::HashSet, mem};

use camino::Utf8PathBuf;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use self::table::Key;
pub use self::{
    cursor::Cursor,
    queue_view::QueueView,
    table::{Display, Filter, Indent, Item, Operation, Sort, Table},
};
use crate::{
    create::Created,
    model::{self, BaseUpdate, PullRequest, Row, RowId, Terminal, Worktree},
    queue::QueueRole,
    source::jp::JpState,
};

/// Event delivered to [`App::apply`] from a background source or task.
#[derive(Debug)]
pub enum Update {
    Worktrees(Result<Vec<Worktree>, String>),
    Terminals(Result<Vec<Terminal>, String>),
    Jp(Result<JpState, String>),
    PullRequests(Result<Vec<PullRequest>, String>),

    /// Result of fast-forwarding the local default branch.
    Base(Result<BaseUpdate, String>),

    /// Role of this process in the merge queue.
    Queue(QueueRole),

    /// Result of adding (`add: true`) or removing (`add: false`) the queue
    /// label on pull request `number`.
    QueueEdited {
        number: u64,
        add: bool,
        outcome: Result<(), String>,
    },

    /// Result of creating a worktree for pull request `pr` (or a new branch
    /// when `None`).
    Created {
        pr: Option<u64>,
        outcome: Result<Created, String>,
    },

    /// Transient message to display on the status line.
    Notice(String),
}

/// Side effect for the event loop to execute after handling an input event.
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

    /// Create a worktree for the pull request, check out its branch, and open a
    /// tab in it.
    CreateWorktree {
        number: u64,
        branch: String,
    },

    /// Create a worktree on a new branch from the default branch and open a tab
    /// in it.
    NewWorktree {
        branch: String,
    },

    /// Focus `terminal` and run `command` at its prompt if `shell` is the
    /// foreground process.
    RunInTab {
        terminal: String,
        shell: u32,
        command: String,
    },

    /// Open a new tab in `dir` and run `command` in it.
    RunInNewTab {
        dir: Utf8PathBuf,
        command: String,
    },

    /// Add (`add: true`) or remove (`add: false`) the queue label on pull
    /// request `number`.
    Queue {
        number: u64,
        add: bool,
    },

    /// Pause or resume the merge queue in this process.
    PauseQueue(bool),
}

/// Latest value and error state for a single background source.
#[derive(Debug)]
pub struct Feed<T> {
    /// Most recent successful snapshot, retained across failed polls so a
    /// transient error does not blank the table.
    pub data: Option<T>,

    /// Error from the most recent poll, cleared on the next success.
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

/// Latest snapshots from all background sources.
#[derive(Debug, Default)]
pub struct Feeds {
    pub worktrees: Feed<Vec<Worktree>>,
    pub terminals: Feed<Vec<Terminal>>,
    pub jp: Feed<JpState>,
    pub prs: Feed<Vec<PullRequest>>,
    pub base: Feed<BaseUpdate>,

    /// Role of this process in the merge queue, populated after the first
    /// GitHub poll.
    pub queue: Option<QueueRole>,
}

impl Feeds {
    /// Name of the repository's default branch, defaulting to `"main"` before
    /// the first update.
    #[must_use]
    pub fn base_branch(&self) -> &str {
        self.base
            .data
            .as_ref()
            .map_or("main", |base| base.branch.as_str())
    }

    /// Join the latest source snapshots into table rows.
    fn rows(&self) -> Vec<Row> {
        let jp = self.jp.data.as_ref();
        model::rows(
            self.worktrees.data.as_deref().unwrap_or_default(),
            self.terminals.data.as_deref().unwrap_or_default(),
            jp.map_or_default(|jp| jp.runs.as_slice()),
            jp.map_or_default(|jp| jp.sessions.as_slice()),
            jp.map_or_default(|jp| jp.recent.as_slice()),
            self.prs.data.as_deref().unwrap_or_default(),
        )
    }
}

/// Table region hit by a mouse click.
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

    /// Transient message shown on the status line until the next input event.
    pub notice: Option<String>,

    /// PR numbers for which worktree creation is currently in flight.
    pub creating: HashSet<u64>,

    /// Highlighted index in [`Sort::ALL`] when the sort menu is open.
    pub sort_menu: Option<usize>,

    /// Branch name buffer while the new-worktree prompt (`n`) is active.
    pub new_branch: Option<String>,

    /// Reference timestamp used to format relative ages on the current frame.
    pub now: DateTime<Utc>,

    /// Whether the merge queue is paused here, and label edits in flight.
    pub queue: QueueView,

    /// Row and next tab index for cycling through tabs on repeated `Enter`
    /// presses.
    cycle: Option<(RowId, usize)>,

    /// Whether the previous key press was `g`, awaiting a second `g` for `gg`.
    pending_g: bool,
}

impl App {
    /// Return the index of the selected item in [`Table::items`].
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
            Update::PullRequests(read) => {
                self.feeds.prs.apply(read);
                if let Some(prs) = &self.feeds.prs.data {
                    self.queue.settle(prs);
                }
            }
            Update::Base(read) => {
                self.feeds.base.apply(read);
                return;
            }
            Update::Queue(role) => {
                self.feeds.queue = Some(role);
                return;
            }
            Update::QueueEdited {
                number,
                add,
                outcome,
            } => {
                self.notice = Some(self.queue.edited(number, add, outcome));
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

        // Select a newly created worktree once it becomes visible in the table.
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
            // When a filter is active, `Esc` clears the filter instead of quitting.
            KeyCode::Esc if self.table.query().is_none() => Action::Quit,
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Enter => self.enter(cycle),
            KeyCode::Char('o') => self.open(),
            KeyCode::Char('m') => self.toggle_queue(),
            KeyCode::Char('p') => {
                self.notice = Some(self.queue.toggle_pause());
                Action::PauseQueue(self.queue.paused)
            }
            _ => {
                self.browse(key, pending_g);
                Action::None
            }
        }
    }

    /// Handle navigation, expansion, filter, and modal-prompt keys.
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

    /// Handle a key event while the new-worktree branch prompt is active.
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
            // Git branch names cannot contain spaces; replace spaces with dashes.
            KeyCode::Char(' ') if !ctrl => name.push('-'),
            KeyCode::Char(c) if !ctrl => name.push(c),
            _ => {}
        }

        Action::None
    }

    /// `→`: expand the selected row or conversation, or move selection to its
    /// first child if already expanded.
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

    /// `m`: toggle the merge-queue label on the selected row's pull request.
    fn toggle_queue(&mut self) -> Action {
        let Some(item) = self.selected_item() else {
            return Action::None;
        };
        let row = item.row();

        let Some(pr) = &row.pr else {
            self.notice = Some(format!("{} has no open pull request.", row.name));
            return Action::None;
        };

        let number = pr.number;
        let add = self.queue.toggle(number, pr.labeled);

        self.notice = Some(if add {
            format!("Adding #{number} to the merge queue…")
        } else {
            format!("Taking #{number} out of the merge queue…")
        });
        Action::Queue { number, add }
    }

    /// Adjust the scroll offset so the selected item fits within `rows` screen
    /// lines.
    pub fn fit_scroll(&mut self, rows: usize) {
        self.cursor.fit(&self.table.display(), rows);
    }

    /// Move the selection by one item on mouse scroll without wrapping at the
    /// bounds.
    pub fn wheel(&mut self, down: bool) {
        self.cursor.wheel(&self.table.items(), down);
    }

    /// Handle a left-click on display line `line` at `target`.
    ///
    /// Clicking any line selects its item (clicking a note selects the item it
    /// belongs to).
    /// Clicking a worktree cell triggers `Enter`, clicking a tab focuses it,
    /// and clicking the PR, CI, or review cell opens the corresponding GitHub
    /// URL.
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

/// Outcome of pressing `Enter` on the selected item.
enum Enter {
    /// Focus the terminal with this ID, updating the tab-cycle state and status
    /// notice.
    Focus(String, Option<(RowId, usize)>, Option<String>),

    /// Open a new tab in this directory and display the given notice.
    OpenTab(Utf8PathBuf, String),

    /// Create a worktree for this PR number and branch.
    Create(u64, String),

    Notice(String),
}

/// Resolve the `Enter` action for a top-level `row`.
///
/// For an unchecked-out pull request, starts worktree creation unless one is
/// already in `creating`.
/// For a worktree, opens a tab if none exist, focuses its sole tab, or cycles
/// through its tabs using `cycle`.
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
