use chrono::{DateTime, FixedOffset, Local, Utc};
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::{Conversation, ConversationId};
use jp_storage::backend::StoragePresence;
use jp_term::{osc::hyperlink, table::list};
use jp_workspace::ConversationHandle;
use strip_ansi_escapes::strip_str;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    cmd::{ConversationLoadRequest, Output, conversation_id::PositionalIds},
    ctx::Ctx,
    output::print_table,
};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// Sort conversations by a specific field.
    ///
    /// Defaults to last activity (archived conversations default to archive
    /// time).
    #[arg(long)]
    sort: Option<Sort>,

    /// Sort conversations in descending order.
    #[arg(long)]
    descending: bool,

    /// Limit the number of conversations to display.
    #[arg(long)]
    limit: Option<usize>,

    /// Display full conversation details.
    #[arg(long)]
    full: bool,

    /// Show only local conversations.
    #[arg(long)]
    local: bool,

    /// Show archived conversations instead of active ones.
    #[arg(long)]
    archived: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Sort {
    Id,
    Created,
    Activity,
    Expires,
    Messages,
    Local,
}

/// The table column a sort field maps to, used to draw the sort marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortColumn {
    Id,
    Messages,
    Activity,
    Expires,
    Local,
}

/// The active sort, rendered as an up/down marker on its column header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SortMarker {
    column: SortColumn,
    descending: bool,
}

impl SortMarker {
    fn arrow(self) -> char {
        if self.descending { '↓' } else { '↑' }
    }
}

struct Details {
    id: ConversationId,
    active: bool,
    pinned_at: Option<DateTime<Utc>>,
    archived_at: Option<DateTime<Utc>>,
    title: Option<String>,
    messages: usize,
    last_event_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    local: bool,
    external: bool,
}

/// The stable machine-readable payload: one object per listed conversation.
///
/// Keys are a fixed contract, deliberately decoupled from the table's display
/// columns: column headers, markers, and layout can change freely, these keys
/// cannot change without breaking consumers.
/// Absent fields serialize as `null`; titles are never truncated here (only the
/// pretty table shaves them to fit the terminal); timestamps are RFC 3339 in
/// UTC.
fn payload(conversations: &[Details]) -> serde_json::Value {
    let items: Vec<_> = conversations
        .iter()
        .map(|d| {
            serde_json::json!({
                "id": d.id.to_string(),
                "title": d.title,
                "active": d.active,
                "pinned_at": d.pinned_at,
                "archived_at": d.archived_at,
                "local": d.local,
                "external": d.external,
                "events": d.messages,
                "created_at": d.id.timestamp(),
                "last_event_at": d.last_event_at,
                "expires_at": d.expires_at,
            })
        })
        .collect();

    serde_json::Value::Array(items)
}

impl Ls {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.target)
    }

    #[expect(clippy::unnecessary_wraps, clippy::too_many_lines)]
    pub(crate) fn run(&self, ctx: &mut Ctx, handles: &[ConversationHandle]) -> Output {
        let active_conversation_id = ctx
            .session
            .as_ref()
            .and_then(|s| ctx.workspace.session_active_conversation(s));
        let limit = self.limit.unwrap_or(usize::MAX);

        // If specific handles were given, filter to those IDs.
        let filter_ids: Option<Vec<_>> = if handles.is_empty() {
            None
        } else {
            Some(handles.iter().map(ConversationHandle::id).collect())
        };

        let to_details =
            |id: ConversationId, c: &Conversation, local: bool, external: bool| Details {
                active: active_conversation_id == Some(id),
                pinned_at: c.pinned_at,
                archived_at: c.archived_at,
                title: c.title.clone(),
                messages: c.events_count,
                last_event_at: c.last_event_at.or(Some(id.timestamp())),
                expires_at: c.expires_at,
                local,
                external,
                id,
            };

        let matches_filters = |id: &ConversationId, local: bool| -> bool {
            filter_ids.as_ref().is_none_or(|f| f.contains(id)) && (!self.local || local)
        };

        // `local` is derived from storage presence: a conversation is shown as
        // local only when it has no workspace projection.
        let workspace = &ctx.workspace;
        let mut conversations: Vec<_> = if self.archived {
            workspace
                .archived_conversations()
                .filter_map(|(id, c, presence)| {
                    let local = presence == StoragePresence::UserLocalOnly;
                    let external = presence == StoragePresence::WorkspaceOnly;
                    matches_filters(&id, local).then(|| to_details(id, &c, local, external))
                })
                .collect()
        } else {
            workspace
                .conversations()
                .filter_map(|(id, c)| {
                    let presence = workspace.conversation_presence(id);
                    let local = presence == Some(StoragePresence::UserLocalOnly);
                    let external = presence == Some(StoragePresence::WorkspaceOnly);
                    matches_filters(id, local).then(|| to_details(*id, &c, local, external))
                })
                .collect()
        };

        let count = conversations.len();
        let skip = count.saturating_sub(limit);

        let sort_cmp = |a: &Details, b: &Details| match self.sort {
            Some(Sort::Id) => a.id.cmp(&b.id),
            Some(Sort::Created) => a.id.timestamp().cmp(&b.id.timestamp()),
            Some(Sort::Messages) => a.messages.cmp(&b.messages),
            Some(Sort::Expires) => a.expires_at.cmp(&b.expires_at),
            Some(Sort::Local) => (a.external, a.local).cmp(&(b.external, b.local)),
            None if self.archived => b.archived_at.cmp(&a.archived_at),
            Some(Sort::Activity) | None => a.last_event_at.cmp(&b.last_event_at),
        };

        conversations.sort_by(|a, b| {
            // Active is always last, pinned conversations come right before it.
            match (a.active, b.active) {
                (true, false) => return std::cmp::Ordering::Greater,
                (false, true) => return std::cmp::Ordering::Less,
                _ => {}
            }
            match (a.pinned_at, b.pinned_at) {
                (Some(_), None) => return std::cmp::Ordering::Greater,
                (None, Some(_)) => return std::cmp::Ordering::Less,
                (Some(a_pin), Some(b_pin)) => return a_pin.cmp(&b_pin),
                (None, None) => {}
            }
            let ord = sort_cmp(a, b);
            if self.descending { ord.reverse() } else { ord }
        });

        let conversations: Vec<_> = conversations.into_iter().skip(skip).collect();
        let hidden = if count > limit { skip } else { 0 };

        let mut columns = Columns {
            expires_at: conversations.iter().any(|d| d.expires_at.is_some()),
            local: conversations.iter().any(|d| d.local || d.external),
            title: conversations.iter().any(|d| d.title.is_some()),
        };

        let marker = sort_marker(self.sort, self.archived, self.descending);

        // Shrink or drop the free-text title column so the pretty box table
        // fits the terminal. Only the box table mangles its borders by
        // overflowing; piped and JSON output keep full titles for machines.
        let mut title_budget = None;
        if columns.title
            && ctx.printer.pretty_printing_enabled()
            && let Some(max_width) = ctx.term.width.map(usize::from)
        {
            let probe = list(
                build_header_row(columns, marker),
                self.build_body(ctx, &conversations, columns, None, hidden),
                false,
            );
            match fit_title(
                max_line_width(&probe),
                max_width,
                title_column_width(&conversations),
            ) {
                TitleFit::Full => {}
                TitleFit::Truncate(width) => title_budget = Some(width),
                TitleFit::Drop => columns.title = false,
            }
        }

        let header = build_header_row(columns, marker);
        let rows = self.build_body(ctx, &conversations, columns, title_budget, hidden);
        let footer = rows.len() > 20;
        print_table(&ctx.printer, header, rows, footer, &payload(&conversations));
        Ok(())
    }

    fn build_conversation_row(
        &self,
        ctx: &Ctx,
        columns: Columns,
        title_budget: Option<usize>,
        details: &Details,
    ) -> Row {
        let id = details.id;
        let mut id_fmt = if details.active {
            id.to_string().bold().yellow().to_string()
        } else if details.pinned_at.is_some() {
            id.to_string().blue().to_string()
        } else {
            id.to_string()
        };

        if ctx.printer.pretty_printing_enabled() {
            id_fmt = hyperlink(format!("jp://show-metadata/{id}"), id_fmt);
        }

        let messages_fmt = if ctx.printer.pretty_printing_enabled() {
            hyperlink(
                format!("jp://show-events/{id}"),
                details.messages.to_string(),
            )
        } else {
            details.messages.to_string()
        };

        let last_message_at_fmt = if self.full {
            details
                .last_event_at
                .map(|t| {
                    let format = "%Y-%m-%d %H:%M:%S";
                    let local_offset: FixedOffset = *Local::now().offset();

                    t.with_timezone(&local_offset).format(format).to_string()
                })
                .unwrap_or_default()
        } else {
            details.last_event_at.map_or_else(String::new, |t| {
                let ago = (Utc::now() - t).to_std().expect("valid duration");
                timeago::Formatter::new().convert(ago)
            })
        };

        let mut row = Row::new();
        row.add_cell(Cell::new(id_fmt));
        row.add_cell(Cell::new(messages_fmt).set_alignment(CellAlignment::Right));
        row.add_cell(Cell::new(last_message_at_fmt).set_alignment(CellAlignment::Right));

        if columns.expires_at {
            let expires_at_fmt = details.expires_at.map_or_else(String::new, |t| {
                if t < Utc::now() {
                    "Now".to_string()
                } else {
                    let dur = (Utc::now() - t).abs().to_std().unwrap_or_default();
                    timeago::Formatter::new().ago("").convert(dur)
                }
            });

            row.add_cell(Cell::new(expires_at_fmt).set_alignment(CellAlignment::Right));
        }

        if columns.local {
            let cell = local_cell(details.local, details.external);
            row.add_cell(Cell::new(cell).set_alignment(CellAlignment::Center));
        }

        if columns.title {
            let title = details.title.clone().unwrap_or_default();
            let title = match title_budget {
                Some(max) => truncate_to_width(&title, max),
                None => title,
            };
            row.add_cell(Cell::new(title));
        }

        row
    }

    /// Build the table body: the optional "(N hidden)" row followed by one row
    /// per conversation.
    fn build_body(
        &self,
        ctx: &Ctx,
        conversations: &[Details],
        columns: Columns,
        title_budget: Option<usize>,
        hidden: usize,
    ) -> Vec<Row> {
        let mut rows = vec![];

        if hidden > 0 {
            let mut row = Row::new();
            row.add_cell(Cell::new(
                format!("({hidden} hidden)")
                    .italic()
                    .with(Color::AnsiValue(245)),
            ));
            rows.push(row);
        }

        for details in conversations {
            rows.push(self.build_conversation_row(ctx, columns, title_budget, details));
        }

        rows
    }
}

fn build_header_row(columns: Columns, marker: Option<SortMarker>) -> Row {
    let label = |base: &str, column: SortColumn| match marker {
        Some(m) if m.column == column => format!("{base} {}", m.arrow()),
        _ => base.to_string(),
    };

    let mut header = Row::new();
    header.add_cell(Cell::new(label("ID", SortColumn::Id)));
    header
        .add_cell(Cell::new(label("#", SortColumn::Messages)).set_alignment(CellAlignment::Right));
    header.add_cell(
        Cell::new(label("Activity", SortColumn::Activity)).set_alignment(CellAlignment::Right),
    );

    if columns.expires_at {
        header.add_cell(
            Cell::new(label("Expires In", SortColumn::Expires)).set_alignment(CellAlignment::Right),
        );
    }

    if columns.local {
        header.add_cell(
            Cell::new(label("Local", SortColumn::Local)).set_alignment(CellAlignment::Right),
        );
    }

    if columns.title {
        header.add_cell(Cell::new(TITLE_HEADER).set_alignment(CellAlignment::Left));
    }

    header
}

/// The column header that should carry the sort marker, if any.
///
/// Returns `None` when the active sort has no visible column: an archived
/// listing with no explicit `--sort` orders by archive time, which has no
/// column of its own.
fn sort_marker(sort: Option<Sort>, archived: bool, descending: bool) -> Option<SortMarker> {
    if sort.is_none() && archived {
        return None;
    }

    let column = match sort.unwrap_or(Sort::Activity) {
        Sort::Id | Sort::Created => SortColumn::Id,
        Sort::Messages => SortColumn::Messages,
        Sort::Activity => SortColumn::Activity,
        Sort::Expires => SortColumn::Expires,
        Sort::Local => SortColumn::Local,
    };

    Some(SortMarker { column, descending })
}

/// Render the storage-locality cell for a conversation row.
///
/// - `Y` (blue): user-local only, no workspace projection.
/// - `N`: projected into the workspace.
/// - `ext` (magenta): external — present only in the workspace, not yet
///   imported into user-local storage.
fn local_cell(local: bool, external: bool) -> String {
    if external {
        "ext".magenta().to_string()
    } else if local {
        "Y".blue().to_string()
    } else {
        "N".to_string()
    }
}

/// Header label for the free-text title column.
const TITLE_HEADER: &str = "Title";

/// Which optional columns the conversation table renders.
///
/// `ID`, `#`, and `Activity` are always present; these three appear only when
/// at least one listed conversation carries the corresponding value.
#[derive(Clone, Copy)]
struct Columns {
    expires_at: bool,
    local: bool,
    title: bool,
}

/// How the title column must shrink to keep the box table within the terminal.
#[derive(Debug, PartialEq, Eq)]
enum TitleFit {
    /// Render titles in full.
    Full,
    /// Truncate every title to the given display width.
    Truncate(usize),
    /// Drop the title column; even a header-width column would still overflow.
    Drop,
}

/// Decide how to fit the title column given the rendered and available widths.
///
/// The title is the only free-text column, so it is the one shaved when the
/// table overflows; every other column holds a short, fixed-shape value.
/// The column cannot shrink below its header width, so a deeper overflow drops
/// it.
fn fit_title(rendered_width: usize, max_width: usize, title_width: usize) -> TitleFit {
    if rendered_width <= max_width {
        return TitleFit::Full;
    }

    let budget = title_width.saturating_sub(rendered_width - max_width);
    if budget < TITLE_HEADER.len() {
        TitleFit::Drop
    } else {
        TitleFit::Truncate(budget)
    }
}

/// Current display width of the title column: the widest title, floored at the
/// header width.
fn title_column_width(conversations: &[Details]) -> usize {
    conversations
        .iter()
        .filter_map(|d| d.title.as_deref())
        .map(display_width)
        .max()
        .unwrap_or(0)
        .max(TITLE_HEADER.len())
}

/// Truncate `s` to at most `max_width` display columns, appending '…' when
/// cut.
fn truncate_to_width(s: &str, max_width: usize) -> String {
    if display_width(s) <= max_width {
        return s.to_string();
    }
    if max_width == 0 {
        return String::new();
    }

    // Reserve one column for the ellipsis.
    let budget = max_width - 1;
    let mut width = 0;
    let mut out = String::new();
    for ch in s.chars() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > budget {
            break;
        }
        width += w;
        out.push(ch);
    }
    out.push('…');
    out
}

/// Widest line in `rendered`, by display width (ANSI styling and OSC hyperlinks
/// stripped first).
fn max_line_width(rendered: &str) -> usize {
    rendered.lines().map(display_width).max().unwrap_or(0)
}

/// Display width of `s` with ANSI styling and OSC hyperlinks removed.
fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(strip_str(s).as_str())
}

#[cfg(test)]
#[path = "ls_tests.rs"]
mod tests;
