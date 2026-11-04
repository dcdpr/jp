//! Drawing the dashboard.
//!
//! [`screen`] turns the app state into one [`Line`] per terminal row: a header,
//! the column headings, the table, and a status line at the bottom.
//! The table has a line per row and, below an expanded row, a line per tab
//! followed by the conversations of the JP sessions in it, laid out in columns
//! of their own the way `jp conversation ls` lists them.
//! Every line is exactly as wide as the terminal, so painting a screen
//! overwrites the previous one without clearing it first.
//!
//! A column whose source has never answered shows `…` while it waits and `?`
//! once it has failed.

use chrono::{DateTime, Utc};
use crossterm::style::Color;
use unicode_width::UnicodeWidthChar as _;

use crate::{
    app::{App, Display, Feed, Indent, Item, Operation, Sort, Target},
    model::{Checks, PullRequest, Row, RowId, SessionConversation, Status, Terminal},
};

const HELP: &str =
    "enter open · →/space expand · n new · / filter · s sort · o PR · a all · r refresh · q quit";

const HEADINGS: [&str; 7] = [
    "WORKTREE",
    "BRANCH",
    "JP",
    "CONVERSATION",
    "PR",
    "CI",
    "REVIEW",
];

/// The worktree cell of a pull request without a worktree: `enter` creates one.
const CREATE: &str = "create worktree";

/// The columns a click on a line can mean something in.
const PR_COLUMN: usize = 4;
const CI_COLUMN: usize = 5;
const REVIEW_COLUMN: usize = 6;

/// Blank columns between two table columns.
const SPACING: usize = 2;

/// Terminal rows that are not table lines: the header, the column headings, and
/// the status line.
const CHROME: usize = 3;

/// The widest a worktree or branch column gets, so one long name cannot push
/// the conversation off screen.
const MAX_NAME_WIDTH: usize = 36;

/// How a run of text is drawn.
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

/// A turn waiting on the user: the one thing on screen that asks for action.
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

/// A run of text in one style.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub style: Style,

    /// A URL the text links to, drawn as an OSC 8 hyperlink.
    pub link: Option<String>,
}

/// One terminal row.
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

    /// Cut the line to `width` terminal columns, then pad it with blanks to
    /// exactly that width.
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

/// The number of table lines a terminal `height` rows tall has room for.
#[must_use]
pub fn body_rows(height: u16) -> usize {
    usize::from(height).saturating_sub(CHROME)
}

/// Lay out the whole screen for a terminal `width` by `height`.
#[must_use]
pub fn screen(app: &App, width: u16, height: u16) -> Vec<Line> {
    let (width, height) = (usize::from(width), usize::from(height));
    let rows = app.visible();
    let widths = columns(&rows, width);
    let selected = app.selected_index();
    let display = app.display();
    let listing = Listing::of(&display, app.now);

    let mut lines = vec![
        header_line(app),
        table_line(HEADINGS.map(|h| (h.to_owned(), DIM)), &widths, None),
    ];

    for line in display
        .iter()
        .skip(app.scroll)
        .take(height.saturating_sub(CHROME))
    {
        let line = match line {
            Display::Item {
                index,
                item,
                indent,
            } => {
                let mut line = match *item {
                    Item::Row(row) => table_line(
                        cells(app, row),
                        &widths,
                        row.pr.as_ref().map(|pr| (PR_COLUMN, pr.url.as_str())),
                    ),
                    Item::Tab { tab, index, .. } => {
                        table_line(tab_cells(tab, index, *indent), &widths, None)
                    }
                    Item::Conversation { conversation, .. } => {
                        listing.line(conversation, *indent, app.now)
                    }
                    Item::Operation {
                        conversation,
                        operation,
                        ..
                    } => operation_line(operation, &conversation.id, *indent),
                }
                .fit(width);
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
        overlay(&mut lines, &sort_menu(app.sort, highlighted), width);
    }

    lines.into_iter().map(|line| line.fit(width)).collect()
}

/// What a mouse press at column `x`, row `y` of a `width` by `height` screen
/// landed on: a line of the table, by its position in [`App::display`], and the
/// part of it that was hit.
/// `None` outside the table, or while the sort menu covers it.
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
    let index = app.scroll + line;
    if index >= app.display().len() {
        return None;
    }

    let widths = columns(&app.visible(), usize::from(width));
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

/// Cut `text` to at most `width` terminal columns, returning what is left and
/// how many columns it takes.
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

/// Pad `text` with blanks to `width` columns, on the right, or on the left when
/// `right_aligned`.
fn pad(text: &str, width: usize, right_aligned: bool) -> String {
    let blanks = " ".repeat(width.saturating_sub(text_width(text)));
    if right_aligned {
        format!("{blanks}{text}")
    } else {
        format!("{text}{blanks}")
    }
}

/// The width of each table column: the worktree and branch columns fit their
/// widest value, the conversation takes whatever the others leave.
fn columns(rows: &[&Row], width: usize) -> [usize; 7] {
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
            let tab = text_width(guide(Some(Indent::Tab { last: true }))) + 4;
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

    // JP fits `waiting 59m` and `running ×2`, CI `✗ 12 failed` and
    // `● 100/100`.
    let mut widths = [name, branch, 11, 0, 6, 12, 6];
    let taken = widths.iter().sum::<usize>() + SPACING * (widths.len() - 1);
    widths[3] = width.saturating_sub(taken);
    widths
}

/// One table line: each cell cut or padded to its column's width.
///
/// `link` makes the text of one column, by index, a hyperlink to a URL.
/// Only the text links, not the padding after it.
fn table_line(
    cells: [(String, Style); 7],
    widths: &[usize; 7],
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

/// The tree lines in front of a line below a row.
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

/// An operation below its conversation: what it does, then the command that
/// does it, dimmed, so what will run in the tab is on screen before it runs.
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
    let rows = app.visible();
    let mut line = Line::default();
    line.push("dash  ", BOLD);

    if let Some(filter) = &app.filter
        && !filter.query.is_empty()
    {
        line.push(
            format!(
                "filter /{}: {} of {}",
                filter.query,
                rows.len(),
                app.total_rows()
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

        let mut summary = if app.worktrees.data.is_none() {
            "reading worktrees…".to_owned()
        } else if app.show_all {
            format!("all {shown} worktrees")
        } else {
            format!("{shown} active of {} worktrees", app.total())
        };
        match elsewhere.len() {
            0 => {}
            1 => summary.push_str(", 1 pull request without one"),
            n => summary.push_str(&format!(", {n} pull requests without one")),
        }
        line.push(summary, PLAIN);
    }

    if app.sort != Sort::default() {
        line.push(format!(" · by {}", app.sort.label().to_lowercase()), PLAIN);
    }

    line.push("   ", PLAIN);
    line.push(HELP, DIM);
    line
}

fn cells(app: &App, row: &Row) -> [(String, Style); 7] {
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

    // `jp` runs belong to worktrees; a pull request without one has none,
    // whatever state that source is in.
    let jp = unknown(&app.jp)
        .filter(|_| row.is_worktree())
        .unwrap_or_else(|| {
            let waiting = row.runs.iter().filter_map(|run| run.waiting_since).min();
            match waiting {
                Some(since) => status_cell(Status::Waiting { since }, app.now),
                None => running(row.runs.len()),
            }
        });

    // What is running, or else what the worktree's shells last worked on.
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

    let [pr, ci, review] = match (unknown(&app.prs), &row.pr) {
        (Some(marker), _) => [marker.clone(), marker.clone(), marker],
        (None, None) => [
            (String::new(), PLAIN),
            (String::new(), PLAIN),
            (String::new(), PLAIN),
        ],
        (None, Some(pr)) => pr_cells(pr),
    };

    [name, branch, jp, conversation, pr, ci, review]
}

fn running(count: usize) -> (String, Style) {
    match count {
        0 => (String::new(), PLAIN),
        1 => ("running".to_owned(), BOLD),
        n => (format!("running ×{n}"), BOLD),
    }
}

/// A status as a cell: `waiting 2m`, `running`, or nothing when idle.
fn status_cell(status: Status, now: DateTime<Utc>) -> (String, Style) {
    match status {
        Status::Idle => (String::new(), PLAIN),
        Status::Running => ("running".to_owned(), fg(Color::Yellow)),
        Status::Waiting { since } => (format!("waiting {}", age(now, since)), WAITING),
    }
}

/// A tab below its worktree: its number, and its title in the conversation
/// column.
/// The JP column stays empty: the conversations listed below the tab say what
/// each is doing, and the worktree's line sums it up.
fn tab_cells(tab: &Terminal, index: usize, indent: Option<Indent>) -> [(String, Style); 7] {
    let empty = || (String::new(), PLAIN);

    [
        (format!("{}tab {}", guide(indent), index + 1), PLAIN),
        empty(),
        empty(),
        (tab.name.clone(), DIM),
        empty(),
        empty(),
        empty(),
    ]
}

fn pr_cells(pr: &PullRequest) -> [(String, Style); 3] {
    let number = (
        format!("#{}", pr.number),
        if pr.draft { DIM } else { PLAIN },
    );

    let review = if pr.threads == 0 {
        ("-".to_owned(), DIM)
    } else {
        (format!("{}/{}", pr.threads, pr.answered), PLAIN)
    };

    [number, checks(pr.checks), review]
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

/// The column widths of the conversation lines, shared by every conversation on
/// screen so they line up across tabs and rows.
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

        // Ages tick every second, so the age columns start at the widest an
        // age below 100 days prints (`59m ago`, `waiting 59m`). Sized to the
        // current values alone, they would widen and narrow as `9s` turns
        // into `10s` and `59s` into `1m`, and push the titles after them back
        // and forth. The status column is there even when nothing runs, so a
        // turn starting does not push them either.
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

    /// A conversation, as `jp conversation ls` would list it: marker, ID, event
    /// count, last activity, status, and title.
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

/// When the conversation last saw an event, as `5m ago`, or nothing for an
/// empty one.
fn activity(conversation: &SessionConversation, now: DateTime<Utc>) -> String {
    conversation
        .last_event_at
        .map(|at| format!("{} ago", age(now, at)))
        .unwrap_or_default()
}

/// How long before `now` `then` was, as `12s`, `5m`, `3h`, or `2d`.
fn age(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let seconds = (now - then).num_seconds().max(0);
    match seconds {
        0..60 => format!("{seconds}s"),
        60..3_600 => format!("{}m", seconds / 60),
        3_600..86_400 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

/// The sort menu's lines, with entry `highlighted` reversed and the order in
/// force marked.
fn sort_menu(current: Sort, highlighted: usize) -> Vec<Line> {
    let entries: Vec<String> = Sort::ALL
        .iter()
        .enumerate()
        .map(|(index, sort)| {
            let mark = if *sort == current { " ✓" } else { "" };
            format!("{} {}{mark}", index + 1, sort.label())
        })
        .collect();
    let hint = "enter sort · esc close";
    let inner = entries
        .iter()
        .map(|entry| text_width(entry))
        .chain([text_width(hint)])
        .max()
        .unwrap_or(0)
        + 2;

    let title = " Sort by ";
    let bar = |left: char, right: char, label: &str| {
        let fill = "─".repeat(inner.saturating_sub(text_width(label)));
        format!("{left}{label}{fill}{right}")
    };
    let padded = |text: &str| {
        format!(
            " {text}{} ",
            " ".repeat(inner.saturating_sub(text_width(text) + 2))
        )
    };

    let mut lines = vec![];
    let mut top = Line::default();
    top.push(bar('╭', '╮', title), BOLD);
    lines.push(top);

    for (index, entry) in entries.iter().enumerate() {
        let mut line = Line::default();
        line.push("│", BOLD);
        let style = if index == highlighted {
            Style {
                reversed: true,
                ..PLAIN
            }
        } else {
            PLAIN
        };
        line.push(padded(entry), style);
        line.push("│", BOLD);
        lines.push(line);
    }

    let mut hint_line = Line::default();
    hint_line.push("│", BOLD);
    hint_line.push(padded(hint), DIM);
    hint_line.push("│", BOLD);
    lines.push(hint_line);

    let mut bottom = Line::default();
    bottom.push(bar('╰', '╯', ""), BOLD);
    lines.push(bottom);
    lines
}

/// Draw `panel` over the middle of the table, centred, each of its rows
/// replacing the screen row under it.
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

/// The marker for a column whose source has no data yet: `?` after a failed
/// read, `…` before the first one.
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
        line.push(format!("new worktree from {}: ", app.base_branch()), PLAIN);
        line.push(name.clone(), BOLD);
        line.push("   enter create · esc cancel", DIM);
        return line;
    }

    if let Some(filter) = &app.filter
        && filter.editing
    {
        line.push(format!("/{}", filter.query), BOLD);
        if !filter.query.is_empty() && app.visible().is_empty() {
            line.push("  no match", fg(Color::Red));
        }
        line.push("   enter keep · esc cancel", DIM);
        return line;
    }

    if let Some(notice) = &app.notice {
        line.push(notice.clone(), fg(Color::Yellow));
        return line;
    }

    let base = format!("updating {}", app.base_branch());
    let errors: Vec<String> = [
        ("git", &app.worktrees.error),
        ("ghostty", &app.terminals.error),
        ("jp", &app.jp.error),
        ("github", &app.prs.error),
        (base.as_str(), &app.base.error),
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
