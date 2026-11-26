//! Terminal rendering and mouse hit-testing for the dashboard.
//!
//! [`screen`] renders [`App`] into one [`Line`] per terminal row: a summary
//! header, column headings, the scrollable table body, and a bottom status
//! line.
//! Under an expanded row, tabs and session conversations are rendered as
//! indented tree lines, with conversations formatted in aligned columns like
//! `jp conversation ls`.
//! Every [`Line`] is padded to the full terminal width so each frame overwrites
//! the previous one without clearing the screen.
//!
//! Columns whose background source has not yet produced a snapshot render `…`
//! while waiting and `?` if the first poll failed.

use chrono::{DateTime, Utc};
use crossterm::style::Color;
use unicode_width::UnicodeWidthChar as _;

use crate::{
    app::{App, Display, Feed, Indent, Item, Operation, Sort, Target},
    model::{Checks, PullRequest, Row, RowId, SessionConversation, Status, Terminal},
    queue::{self, QueueRole, QueueState},
};

const HELP: &str = "enter open · →/space expand · n new · m queue · p pause · / filter · s sort · \
                    o PR · a all · r refresh · q quit";

const HEADINGS: [&str; COLUMNS] = [
    "WORKTREE",
    "BRANCH",
    "JP",
    "CONVERSATION",
    "PR",
    "CI",
    "REVIEW",
    "QUEUE",
];

/// Number of table columns.
const COLUMNS: usize = 8;

/// Label shown in the worktree column for an unchecked-out pull request.
const CREATE: &str = "create worktree";

/// Column indices with click actions.
const PR_COLUMN: usize = 4;
const CI_COLUMN: usize = 5;
const REVIEW_COLUMN: usize = 6;

/// Blank columns between two table columns.
const SPACING: usize = 2;

/// Number of non-body terminal rows (header, column headings, and status line).
const CHROME: usize = 3;

/// Maximum width of the worktree and branch columns.
const MAX_NAME_WIDTH: usize = 36;

/// Width of the JP column, which fits `waiting 59m` and `running ×2`.
const JP_WIDTH: usize = 11;

/// Width of the PR column.
const PR_WIDTH: usize = 6;

/// Width of the CI column, which fits `✗ 12 failed` and `● 100/100`.
const CI_WIDTH: usize = 12;

/// Width of the review column.
const REVIEW_WIDTH: usize = 6;

/// Width of the merge-queue column, which fits `▶ waiting for CI · 2/3`.
const QUEUE_WIDTH: usize = 22;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<Color>,
    pub bold: bool,
    pub dim: bool,
    pub reversed: bool,
}

const PLAIN: Style = Style {
    fg: None,
    bold: false,
    dim: false,
    reversed: false,
};
const DIM: Style = Style { dim: true, ..PLAIN };
const BOLD: Style = Style {
    bold: true,
    ..PLAIN
};

/// Style for a turn waiting on user input.
const WAITING: Style = Style {
    fg: Some(Color::Magenta),
    bold: true,
    ..PLAIN
};

const fn fg(color: Color) -> Style {
    Style {
        fg: Some(color),
        ..PLAIN
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,

    /// Optional URL rendered as an OSC 8 terminal hyperlink.
    pub link: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
}

impl Line {
    fn push(&mut self, text: impl Into<String>, style: Style) {
        self.spans.push(Span {
            text: text.into(),
            style,
            link: None,
        });
    }

    /// The row's text without its styling.
    #[cfg(test)]
    fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }

    /// Truncate the line to `width` terminal columns and right-pad it with
    /// spaces.
    fn fit(self, width: usize) -> Self {
        let mut used = 0;
        let mut line = Self::default();

        for span in self.spans {
            let (text, taken) = clip(&span.text, width - used);
            used += taken;
            if !text.is_empty() {
                line.spans.push(Span { text, ..span });
            }
        }

        if used < width {
            line.push(" ".repeat(width - used), PLAIN);
        }
        line
    }

    fn reverse(&mut self) {
        for span in &mut self.spans {
            span.style.reversed = true;
        }
    }
}

/// Return the number of table body rows that fit in a terminal of `height`
/// rows.
#[must_use]
pub fn body_rows(height: u16) -> usize {
    usize::from(height).saturating_sub(CHROME)
}

/// Render the full screen for a terminal of `width` columns by `height` rows.
#[must_use]
pub fn screen(app: &App, width: u16, height: u16) -> Vec<Line> {
    let (width, height) = (usize::from(width), usize::from(height));
    let rows = app.table.visible();
    let widths = columns(&rows, width);
    let selected = app.selected_index();
    let display = app.table.display();
    let listing = Listing::of(&display, app.now);

    let mut lines = vec![
        header_line(app),
        table_line(HEADINGS.map(|h| (h.to_owned(), DIM)), &widths, None),
    ];

    for line in display
        .iter()
        .skip(app.cursor.scroll)
        .take(height.saturating_sub(CHROME))
    {
        let line = match line {
            Display::Item {
                index,
                item,
                indent,
            } => {
                let mut line = item_line(app, *item, *indent, &widths, &listing).fit(width);
                if Some(*index) == selected {
                    line.reverse();
                }
                line
            }
            Display::Note { text, indent, .. } => {
                let mut line = Line::default();
                line.push(guide(Some(*indent)), DIM);
                line.push(text.clone(), DIM);
                line
            }
        };
        lines.push(line);
    }

    lines.resize(height.saturating_sub(1), Line::default());
    lines.push(status_line(app));
    lines.truncate(height);

    if let Some(highlighted) = app.sort_menu {
        overlay(&mut lines, &sort_menu(app.table.sort, highlighted), width);
    }

    lines.into_iter().map(|line| line.fit(width)).collect()
}

/// Render a selectable table item before width fitting and selection
/// highlighting.
fn item_line(
    app: &App,
    item: Item<'_>,
    indent: Option<Indent>,
    widths: &[usize; COLUMNS],
    listing: &Listing,
) -> Line {
    match item {
        Item::Row(row) => table_line(
            cells(app, row),
            widths,
            row.pr.as_ref().map(|pr| (PR_COLUMN, pr.url.as_str())),
        ),
        Item::Tab { tab, index, .. } => table_line(tab_cells(tab, index, indent), widths, None),
        Item::Conversation { conversation, .. } => listing.line(conversation, indent, app.now),
        Item::Operation {
            conversation,
            operation,
            ..
        } => operation_line(operation, &conversation.id, indent),
    }
}

/// Map a mouse click at column `x`, row `y` to a [`Table::display`] index and
/// [`Target`].
///
/// Returns `None` if the click is outside the table body or while the sort menu
/// is open.
///
/// [`Table::display`]: crate::app::Table::display
#[must_use]
pub fn hit(app: &App, width: u16, height: u16, x: u16, y: u16) -> Option<(usize, Target)> {
    if app.sort_menu.is_some() {
        return None;
    }

    // The header and the column headings come before the first table line.
    let line = usize::from(y).checked_sub(2)?;
    if line >= usize::from(height).saturating_sub(CHROME) {
        return None;
    }
    let index = app.cursor.scroll + line;
    if index >= app.table.display().len() {
        return None;
    }

    let widths = columns(&app.table.visible(), usize::from(width));
    let start = |column: usize| widths[..column].iter().sum::<usize>() + SPACING * column;
    let within =
        |column: usize, x: usize| (start(column)..start(column) + widths[column]).contains(&x);
    let x = usize::from(x);

    let target = if x < widths[0] {
        Target::Worktree
    } else if within(PR_COLUMN, x) {
        Target::PullRequest
    } else if within(CI_COLUMN, x) {
        Target::Checks
    } else if within(REVIEW_COLUMN, x) {
        Target::Reviews
    } else {
        Target::Other
    };
    Some((index, target))
}

/// Truncate `text` to at most `width` terminal columns, returning the truncated
/// string and its display width.
fn clip(text: &str, width: usize) -> (String, usize) {
    let mut used = 0;
    let mut clipped = String::new();

    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > width {
            break;
        }
        used += w;
        clipped.push(c);
    }

    (clipped, used)
}

fn text_width(text: &str) -> usize {
    text.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Pad `text` with spaces to `width` display columns, left-aligned or
/// `right_aligned`.
fn pad(text: &str, width: usize, right_aligned: bool) -> String {
    let blanks = " ".repeat(width.saturating_sub(text_width(text)));
    if right_aligned {
        format!("{blanks}{text}")
    } else {
        format!("{text}{blanks}")
    }
}

/// Compute the width of each table column, sizing worktree and branch to their
/// widest visible value and giving the remaining width to the conversation
/// column.
fn columns(rows: &[&Row], width: usize) -> [usize; COLUMNS] {
    let fit = |widest: Option<usize>, heading: &str| {
        widest
            .unwrap_or(0)
            .max(text_width(heading))
            .min(MAX_NAME_WIDTH)
    };

    // A pull request's `create worktree`, and a tab's `├ tab N`, sit in the
    // worktree column too.
    let labels = rows.iter().map(|row| {
        if !row.is_worktree() {
            text_width(CREATE)
        } else if row.terminals.is_empty() {
            text_width(&row.name)
        } else {
            let tab = text_width(guide(Some(Indent::Tab { last: true }))) + text_width("tab ");
            text_width(&row.name).max(tab + row.terminals.len().to_string().len())
        }
    });
    let name = fit(labels.max(), HEADINGS[0]);
    let branch = fit(
        rows.iter()
            .map(|row| row.branch.as_deref().map_or(0, text_width))
            .max(),
        HEADINGS[1],
    );

    let mut widths = [
        name,
        branch,
        JP_WIDTH,
        0,
        PR_WIDTH,
        CI_WIDTH,
        REVIEW_WIDTH,
        QUEUE_WIDTH,
    ];
    let taken = widths.iter().sum::<usize>() + SPACING * (widths.len() - 1);
    widths[3] = width.saturating_sub(taken);
    widths
}

/// Format a table row, truncating or padding each cell to its column width.
///
/// If `link` is `Some((col, url))`, wraps the text of column `col` (excluding
/// trailing padding) in an OSC 8 hyperlink.
fn table_line(
    cells: [(String, Style); COLUMNS],
    widths: &[usize; COLUMNS],
    link: Option<(usize, &str)>,
) -> Line {
    let mut line = Line::default();

    for (column, ((text, style), width)) in cells.into_iter().zip(widths).enumerate() {
        let (text, used) = clip(&text, *width);
        line.spans.push(Span {
            text,
            style,
            link: link
                .filter(|(linked, _)| *linked == column)
                .map(|(_, url)| url.to_owned()),
        });
        line.push(" ".repeat(width - used + SPACING), PLAIN);
    }

    line
}

/// Return the tree-drawing prefix for a given [`Indent`].
fn guide(indent: Option<Indent>) -> &'static str {
    match indent {
        None => "",
        Some(Indent::Tab { last: false }) => "├ ",
        Some(Indent::Tab { last: true }) => "└ ",
        Some(Indent::InTab { last_tab: false }) => "│   ",
        Some(Indent::InTab { last_tab: true }) => "    ",
        Some(Indent::InRow) => "  ",
        // Under the conversation's ID, past its guide and its marker.
        Some(Indent::Operation {
            last_tab: Some(false),
        }) => "│       ",
        Some(Indent::Operation {
            last_tab: Some(true),
        }) => "        ",
        Some(Indent::Operation { last_tab: None }) => "      ",
    }
}

/// Format an operation line showing its label and the dimmed CLI command.
fn operation_line(operation: Operation, id: &str, indent: Option<Indent>) -> Line {
    let widest = Operation::ALL
        .iter()
        .map(|operation| text_width(operation.label()))
        .max()
        .unwrap_or(0);

    let mut line = Line::default();
    line.push(guide(indent), DIM);
    line.push(pad(operation.label(), widest, false), PLAIN);
    line.push("  ", PLAIN);
    line.push(operation.command(id), DIM);
    line
}

fn header_line(app: &App) -> Line {
    let rows = app.table.visible();
    let mut line = Line::default();
    line.push("dash  ", BOLD);

    if let Some(filter) = &app.table.filter
        && !filter.query.is_empty()
    {
        line.push(
            format!(
                "filter /{}: {} of {}",
                filter.query,
                rows.len(),
                app.table.total_rows()
            ),
            fg(Color::Yellow),
        );
        if !filter.editing {
            line.push(" · esc clears", DIM);
        }
    } else {
        let (worktrees, elsewhere): (Vec<&Row>, Vec<&Row>) =
            rows.iter().partition(|row| row.is_worktree());
        let shown = worktrees.len();

        let mut summary = if app.feeds.worktrees.data.is_none() {
            "reading worktrees…".to_owned()
        } else if app.table.show_all {
            format!("all {shown} worktrees")
        } else {
            format!("{shown} active of {} worktrees", app.table.total())
        };
        match elsewhere.len() {
            0 => {}
            1 => summary.push_str(", 1 pull request without one"),
            n => summary.push_str(&format!(", {n} pull requests without one")),
        }
        line.push(summary, PLAIN);
    }

    queue_summary(app, &mut line);

    if app.table.sort != Sort::default() {
        line.push(
            format!(" · by {}", app.table.sort.label().to_lowercase()),
            PLAIN,
        );
    }

    line.push("   ", PLAIN);
    line.push(HELP, DIM);
    line
}

/// Append the merge-queue summary to the header line: paused state, waiting
/// entry count, and the active step for the front entry.
fn queue_summary(app: &App, line: &mut Line) {
    if app.queue.paused {
        line.push(" · queue paused", fg(Color::Yellow));
        return;
    }

    let prs = app.feeds.prs.data.as_deref().unwrap_or_default();
    let waiting = queue::waiting(prs);
    if waiting == 0 {
        return;
    }
    line.push(format!(" · queue: {waiting}"), PLAIN);

    if app.feeds.queue == Some(QueueRole::Watching) {
        line.push(" · watching", DIM);
    } else if let Some(front) = queue::front(prs)
        && let Some(QueueState::Front(step)) = &front.members[0].queue
    {
        line.push(
            format!(" · ▶ {} {}", front.name(), step.label()),
            fg(Color::Yellow),
        );
    }
}

fn cells(app: &App, row: &Row) -> [(String, Style); COLUMNS] {
    let name = match &row.id {
        RowId::PullRequest(number) if app.creating.contains(number) => {
            ("creating…".to_owned(), fg(Color::Yellow))
        }
        RowId::PullRequest(_) => (CREATE.to_owned(), DIM),
        RowId::Worktree(_) if row.runs.is_empty() => (row.name.clone(), PLAIN),
        RowId::Worktree(_) => (row.name.clone(), BOLD),
    };

    let branch = match &row.branch {
        Some(branch) => (branch.clone(), PLAIN),
        None => ("(detached)".to_owned(), DIM),
    };

    // `jp` runs only exist in local worktrees, never on unchecked-out PR rows.
    let jp = unknown(&app.feeds.jp)
        .filter(|_| row.is_worktree())
        .unwrap_or_else(|| {
            let waiting = row.runs.iter().filter_map(|run| run.waiting_since).min();
            match (waiting, row.runs.len()) {
                (Some(since), _) => status_cell(Status::Waiting { since }, app.now),
                (None, 0) => idle(row, app.now),
                (None, count) => running(count),
            }
        });

    // Show the running conversation's title, or fall back to the active
    // session conversation.
    let conversation = match row.runs.first() {
        Some(run) => (run.conversation.clone(), PLAIN),
        None => row
            .sessions
            .iter()
            .find_map(|session| session.active())
            .map_or((String::new(), PLAIN), |active| {
                (active.title.clone().unwrap_or_default(), DIM)
            }),
    };

    let [pr, ci, review, queue] = pull_request_cells(app, row);
    [name, branch, jp, conversation, pr, ci, review, queue]
}

/// The PR, CI, review, and queue cells of `row`.
fn pull_request_cells(app: &App, row: &Row) -> [(String, Style); 4] {
    let [pr, ci, review, mut queue] = match (unknown(&app.feeds.prs), &row.pr) {
        (Some(marker), _) => [marker.clone(), marker.clone(), marker.clone(), marker],
        (None, None) => [
            (String::new(), PLAIN),
            (String::new(), PLAIN),
            (String::new(), PLAIN),
            (String::new(), PLAIN),
        ],
        (None, Some(pr)) => pr_cells(pr),
    };

    // Render optimistic label edits before the next GitHub poll confirms them.
    if let Some(add) = row
        .pr
        .as_ref()
        .and_then(|pr| app.queue.edits.get(&pr.number))
    {
        let text = if *add { "adding…" } else { "removing…" };
        queue = (text.to_owned(), fg(Color::Yellow));
    }

    [pr, ci, review, queue]
}

fn running(count: usize) -> (String, Style) {
    match count {
        1 => ("running".to_owned(), BOLD),
        n => (format!("running ×{n}"), BOLD),
    }
}

/// Format the time since JP's last activity in an idle worktree (for example,
/// `5m ago`), or an empty cell if none.
fn idle(row: &Row, now: DateTime<Utc>) -> (String, Style) {
    row.last_activity().map_or((String::new(), PLAIN), |at| {
        (format!("{} ago", age(now, at)), DIM)
    })
}

/// Format a [`Status`] cell (`waiting 2m`, `running`, or empty when idle).
fn status_cell(status: Status, now: DateTime<Utc>) -> (String, Style) {
    match status {
        Status::Idle => (String::new(), PLAIN),
        Status::Running => ("running".to_owned(), fg(Color::Yellow)),
        Status::Waiting { since } => (format!("waiting {}", age(now, since)), WAITING),
    }
}

/// Build the table cells for a terminal tab row under an expanded worktree.
///
/// The JP status column is left blank because per-conversation status is shown
/// on the child conversation lines and summarized on the parent worktree row.
fn tab_cells(tab: &Terminal, index: usize, indent: Option<Indent>) -> [(String, Style); COLUMNS] {
    let empty = || (String::new(), PLAIN);

    [
        (format!("{}tab {}", guide(indent), index + 1), PLAIN),
        empty(),
        empty(),
        (tab.name.clone(), DIM),
        empty(),
        empty(),
        empty(),
        empty(),
    ]
}

fn pr_cells(pr: &PullRequest) -> [(String, Style); 4] {
    let number = (
        format!("#{}", pr.number),
        if pr.draft { DIM } else { PLAIN },
    );

    let review = if pr.threads == 0 {
        ("-".to_owned(), DIM)
    } else {
        (format!("{}/{}", pr.threads, pr.answered), PLAIN)
    };

    [number, checks(pr.checks), review, queue_cell(pr)]
}

/// Format the `QUEUE` cell for `pr`, appending `<position>/<size>` when `pr`
/// belongs to a stack.
fn queue_cell(pr: &PullRequest) -> (String, Style) {
    let (state, style) = queue_state(pr.queue.as_ref());
    match &pr.stack {
        Some(place) if !state.is_empty() => (
            format!("{state} · {}/{}", place.position, place.size),
            style,
        ),
        _ => (state, style),
    }
}

fn queue_state(state: Option<&QueueState>) -> (String, Style) {
    match state {
        None => (String::new(), PLAIN),
        Some(QueueState::Front(step)) => (format!("▶ {}", step.label()), fg(Color::Yellow)),
        Some(QueueState::Waiting { position }) => (format!("queued {position}"), PLAIN),
        Some(QueueState::Draft) => ("draft".to_owned(), DIM),
        Some(QueueState::Failing) => ("failed".to_owned(), fg(Color::Red)),
        Some(QueueState::Conflicts) => ("conflicts".to_owned(), fg(Color::Red)),
        Some(QueueState::Blocked(_)) => ("blocked".to_owned(), fg(Color::Red)),
        Some(QueueState::OtherBase(base)) => (format!("base {base}"), DIM),
    }
}

fn checks(checks: Checks) -> (String, Style) {
    let Checks {
        passed,
        pending,
        failed,
    } = checks;
    let total = checks.total();

    if total == 0 {
        ("· none".to_owned(), DIM)
    } else if failed > 0 {
        (format!("✗ {failed} failed"), fg(Color::Red))
    } else if pending > 0 {
        (format!("● {passed}/{total}"), fg(Color::Yellow))
    } else {
        (format!("✓ {passed}/{total}"), fg(Color::Green))
    }
}

/// Shared column widths for expanded conversation lines across all visible
/// rows.
#[derive(Debug, Default)]
struct Listing {
    id: usize,
    events: usize,
    activity: usize,
    status: usize,
}

impl Listing {
    fn of(display: &[Display<'_>], now: DateTime<Utc>) -> Self {
        let conversations = display.iter().filter_map(|line| match line {
            Display::Item {
                item: Item::Conversation { conversation, .. },
                indent,
                ..
            } => Some((conversation, *indent)),
            _ => None,
        });

        // Reserve enough width for the widest sub-100-day age strings (`59m ago`
        // and `waiting 59m`) so conversation titles do not shift horizontally as
        // second and minute counters tick or when a turn starts running.
        let mut listing = Self {
            activity: text_width("59m ago"),
            status: text_width("waiting 59m"),
            ..Self::default()
        };
        for (conversation, indent) in conversations {
            // The ID column starts after the deepest guide, so IDs align.
            listing.id = listing
                .id
                .max(text_width(guide(indent)) + text_width(&conversation.id));
            listing.events = listing.events.max(conversation.events.to_string().len());
            listing.activity = listing
                .activity
                .max(text_width(&activity(conversation, now)));
            listing.status = listing
                .status
                .max(text_width(&status_cell(conversation.status, now).0));
        }
        listing
    }

    /// Format a session conversation line with marker, ID, event count, age,
    /// status, and title.
    fn line(
        &self,
        conversation: &SessionConversation,
        indent: Option<Indent>,
        now: DateTime<Utc>,
    ) -> Line {
        let mut line = Line::default();
        let guide = guide(indent);
        line.push(guide, DIM);

        let (marker, id_style) = if conversation.active {
            ("▶ ", fg(Color::Cyan))
        } else if conversation.pinned {
            ("★ ", fg(Color::Blue))
        } else {
            ("  ", PLAIN)
        };
        line.push(marker, id_style);
        line.push(
            pad(
                &conversation.id,
                self.id.saturating_sub(text_width(guide)),
                false,
            ),
            id_style,
        );
        line.push("  ", PLAIN);
        line.push(
            pad(&conversation.events.to_string(), self.events, true),
            DIM,
        );
        line.push("  ", PLAIN);
        line.push(pad(&activity(conversation, now), self.activity, true), DIM);

        let (status, style) = status_cell(conversation.status, now);
        line.push("  ", PLAIN);
        line.push(pad(&status, self.status, false), style);

        line.push("  ", PLAIN);
        let title_style = if conversation.active { BOLD } else { PLAIN };
        line.push(conversation.title.clone().unwrap_or_default(), title_style);
        line
    }
}

/// Format the relative age of `conversation`'s last event (e.g. `5m ago`), or
/// an empty string if none.
fn activity(conversation: &SessionConversation, now: DateTime<Utc>) -> String {
    conversation
        .last_event_at
        .map_or_default(|at| format!("{} ago", age(now, at)))
}

/// Format the elapsed time from `then` to `now` as `12s`, `5m`, `3h`, or `2d`.
fn age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..MINUTE => format!("{seconds}s"),
        MINUTE..HOUR => format!("{}m", seconds / MINUTE),
        HOUR..DAY => format!("{}h", seconds / HOUR),
        _ => format!("{}d", seconds / DAY),
    }
}

/// Render the sort menu box with `highlighted` reversed and `current` marked
/// with `✓`.
fn sort_menu(current: Sort, highlighted: usize) -> Vec<Line> {
    let mut body: Vec<(String, Style)> = Sort::ALL
        .iter()
        .enumerate()
        .map(|(index, sort)| {
            let mark = if *sort == current { " ✓" } else { "" };
            let style = if index == highlighted {
                Style {
                    reversed: true,
                    ..PLAIN
                }
            } else {
                PLAIN
            };
            (format!("{} {}{mark}", index + 1, sort.label()), style)
        })
        .collect();
    body.push(("enter sort · esc close".to_owned(), DIM));

    panel(" Sort by ", &body)
}

/// Render a bordered box around `body` with `title` in the top border.
fn panel(title: &str, body: &[(String, Style)]) -> Vec<Line> {
    let inner = body
        .iter()
        .map(|(text, _)| text_width(text))
        .max()
        .unwrap_or(0)
        + 2;

    let bar = |left: char, right: char, label: &str| {
        let fill = "─".repeat(inner.saturating_sub(text_width(label)));
        format!("{left}{label}{fill}{right}")
    };

    let mut top = Line::default();
    top.push(bar('╭', '╮', title), BOLD);
    let mut lines = vec![top];

    for (text, style) in body {
        let blanks = " ".repeat(inner.saturating_sub(text_width(text) + 2));
        let mut line = Line::default();
        line.push("│", BOLD);
        line.push(format!(" {text}{blanks} "), *style);
        line.push("│", BOLD);
        lines.push(line);
    }

    let mut bottom = Line::default();
    bottom.push(bar('╰', '╯', ""), BOLD);
    lines.push(bottom);
    lines
}

/// Overlay `panel` centered over the table body in `lines`.
fn overlay(lines: &mut [Line], panel: &[Line], width: usize) {
    let panel_width = panel
        .iter()
        .map(|line| line.spans.iter().map(|span| text_width(&span.text)).sum())
        .max()
        .unwrap_or(0);
    let left = width.saturating_sub(panel_width) / 2;
    let table = lines.len().saturating_sub(CHROME);
    let top = 2 + table.saturating_sub(panel.len()) / 2;

    for (offset, row) in panel.iter().enumerate() {
        let Some(target) = lines.get_mut(top + offset) else {
            break;
        };
        let mut line = Line::default();
        line.push(" ".repeat(left), PLAIN);
        line.spans.extend(row.spans.iter().cloned());
        *target = line;
    }
}

/// Return the placeholder cell for a feed with no data yet (`?` on error, `…`
/// while loading).
fn unknown<T>(feed: &Feed<T>) -> Option<(String, Style)> {
    if feed.data.is_some() {
        return None;
    }

    Some(if feed.error.is_some() {
        ("?".to_owned(), fg(Color::Red))
    } else {
        ("…".to_owned(), DIM)
    })
}

fn status_line(app: &App) -> Line {
    let mut line = Line::default();

    if let Some(name) = &app.new_branch {
        line.push(
            format!("new worktree from {}: ", app.feeds.base_branch()),
            PLAIN,
        );
        line.push(name.clone(), BOLD);
        line.push("   enter create · esc cancel", DIM);
        return line;
    }

    if let Some(filter) = &app.table.filter
        && filter.editing
    {
        line.push(format!("/{}", filter.query), BOLD);
        if !filter.query.is_empty() && app.table.visible().is_empty() {
            line.push("  no match", fg(Color::Red));
        }
        line.push("   enter keep · esc cancel", DIM);
        return line;
    }

    if let Some(notice) = &app.notice {
        line.push(notice.clone(), fg(Color::Yellow));
        return line;
    }

    let base = format!("updating {}", app.feeds.base_branch());
    let errors: Vec<String> = [
        ("git", &app.feeds.worktrees.error),
        ("ghostty", &app.feeds.terminals.error),
        ("jp", &app.feeds.jp.error),
        ("github", &app.feeds.prs.error),
        (base.as_str(), &app.feeds.base.error),
    ]
    .into_iter()
    .filter_map(|(source, error)| Some(format!("{source}: {}", error.as_ref()?)))
    .collect();

    if !errors.is_empty() {
        line.push(errors.join(" · "), fg(Color::Red));
    }
    line
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
