use chrono::{DateTime, FixedOffset, Local, Utc};
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::ConversationId;
use jp_term::osc::hyperlink;
use jp_workspace::ConversationHandle;

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

#[derive(Debug, Default, Clone, Copy, clap::ValueEnum)]
enum Sort {
    #[default]
    Id,
    Created,
    Activity,
    Expires,
    Messages,
    Local,
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
}

impl Ls {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.target)
    }

    #[expect(clippy::unnecessary_wraps)]
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

        let to_details = |id: ConversationId, c: &Conversation| Details {
            active: active_conversation_id == Some(id),
            pinned_at: c.pinned_at,
            archived_at: c.archived_at,
            title: c.title.clone(),
            messages: c.events_count,
            last_event_at: c.last_event_at.or(Some(id.timestamp())),
            expires_at: c.expires_at,
            local: c.user,
            id,
        };

        let matches_filters = |id: &ConversationId, c: &Conversation| -> bool {
            filter_ids.as_ref().is_none_or(|f| f.contains(id)) && (!self.local || c.user)
        };

        let mut conversations: Vec<_> = if self.archived {
            ctx.workspace
                .archived_conversations()
                .filter(|(id, c)| matches_filters(id, c))
                .map(|(id, c)| to_details(id, &c))
                .collect()
        } else {
            ctx.workspace
                .conversations()
                .filter(|(id, c)| matches_filters(id, c))
                .map(|(id, c)| to_details(*id, &c))
                .collect()
        };

        let count = conversations.len();
        let skip = count.saturating_sub(limit);

        let sort_cmp = |a: &Details, b: &Details| match self.sort {
            Some(Sort::Created) => a.id.timestamp().cmp(&b.id.timestamp()),
            Some(Sort::Messages) => a.messages.cmp(&b.messages),
            Some(Sort::Activity) => a.last_event_at.cmp(&b.last_event_at),
            Some(Sort::Expires) => a.expires_at.cmp(&b.expires_at),
            Some(Sort::Local) => a.local.cmp(&b.local),
            None if self.archived => b.archived_at.cmp(&a.archived_at),
            _ => a.id.cmp(&b.id),
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
        let (expires_at_column, local_column, title_column, header) =
            build_header_row(&conversations);

        let mut rows = vec![];
        if count > limit {
            let mut row = Row::new();
            row.add_cell(Cell::new(
                format!("({skip} hidden)")
                    .italic()
                    .with(Color::AnsiValue(245)),
            ));
            rows.push(row);
        }

        for details in conversations {
            rows.push(self.build_conversation_row(
                ctx,
                local_column,
                title_column,
                expires_at_column,
                details,
            ));
        }

        let footer = rows.len() > 20;
        print_table(&ctx.printer, header, rows, footer);
        Ok(())
    }

    fn build_conversation_row(
        &self,
        ctx: &Ctx,
        local_column: bool,
        title_column: bool,
        expires_at_column: bool,
        details: Details,
    ) -> Row {
        let Details {
            id,
            active,
            pinned_at,
            title,
            messages,
            last_event_at: last_message_at,
            expires_at,
            local,
            archived_at: _,
        } = details;

        let mut id_fmt = if active {
            id.to_string().bold().yellow().to_string()
        } else if pinned_at.is_some() {
            id.to_string().blue().to_string()
        } else {
            id.to_string()
        };

        if ctx.printer.pretty_printing_enabled() {
            id_fmt = hyperlink(format!("jp://show-metadata/{id}"), id_fmt);
        }

        let messages_fmt = if ctx.printer.pretty_printing_enabled() {
            hyperlink(format!("jp://show-events/{id}"), messages.to_string())
        } else {
            messages.to_string()
        };

        let last_message_at_fmt = if self.full {
            last_message_at
                .map(|t| {
                    let format = "%Y-%m-%d %H:%M:%S";
                    let local_offset: FixedOffset = *Local::now().offset();

                    t.with_timezone(&local_offset).format(format).to_string()
                })
                .unwrap_or_default()
        } else {
            last_message_at.map_or_else(String::new, |t| {
                let ago = (Utc::now() - t).to_std().expect("valid duration");
                timeago::Formatter::new().convert(ago)
            })
        };

        let mut row = Row::new();
        row.add_cell(Cell::new(id_fmt));
        row.add_cell(Cell::new(messages_fmt).set_alignment(CellAlignment::Right));
        row.add_cell(Cell::new(last_message_at_fmt).set_alignment(CellAlignment::Right));

        if expires_at_column {
            let expires_at_fmt = expires_at.map_or_else(String::new, |t| {
                if t < Utc::now() {
                    "Now".to_string()
                } else {
                    let dur = (Utc::now() - t).abs().to_std().unwrap_or_default();
                    timeago::Formatter::new().ago("").convert(dur)
                }
            });

            row.add_cell(Cell::new(expires_at_fmt).set_alignment(CellAlignment::Right));
        }

        if local_column {
            let local = if local {
                "Y".blue().to_string()
            } else {
                "N".to_string()
            };

            row.add_cell(Cell::new(local).set_alignment(CellAlignment::Center));
        }
        if title_column {
            let title = title.unwrap_or_default();
            row.add_cell(Cell::new(title));
        }

        row
    }
}

fn build_header_row(conversations: &[Details]) -> (bool, bool, bool, Row) {
    let mut header = Row::new();
    header.add_cell(Cell::new("ID"));
    header.add_cell(Cell::new("#").set_alignment(CellAlignment::Right));
    header.add_cell(Cell::new("Activity").set_alignment(CellAlignment::Right));

    let mut expires_at_column = false;
    if conversations.iter().any(|d| d.expires_at.is_some()) {
        expires_at_column = true;
        header.add_cell(Cell::new("Expires In").set_alignment(CellAlignment::Right));
    }

    // Show "local" column if any conversations are stored locally.
    let mut local_column = false;
    if conversations.iter().any(|d| d.local) {
        local_column = true;
        header.add_cell(Cell::new("Local").set_alignment(CellAlignment::Right));
    }

    let mut title_column = false;
    if conversations.iter().any(|d| d.title.is_some()) {
        title_column = true;
        header.add_cell(Cell::new("Title").set_alignment(CellAlignment::Left));
    }

    (expires_at_column, local_column, title_column, header)
}
