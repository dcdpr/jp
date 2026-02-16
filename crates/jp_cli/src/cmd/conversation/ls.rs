use chrono::{DateTime, FixedOffset, Local, Utc};
use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::{Conversation, ConversationId};
use jp_term::osc::hyperlink;

use crate::{Output, cmd::Success, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Ls {
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
    title: Option<String>,
    messages: usize,
    last_event_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
    local: bool,
}

impl Ls {
    #[expect(clippy::unnecessary_wraps)]
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let active_conversation_id = ctx.workspace.active_conversation_id();
        let limit = self.limit.unwrap_or(usize::MAX);

        let mut conversations = ctx
            .workspace
            .conversations()
            .filter(|(_, c)| !self.local || c.user)
            .map(|(id, conversation)| {
                let Conversation {
                    title,
                    user,
                    last_event_at,
                    expires_at,
                    events_count,
                    ..
                } = conversation;
                Details {
                    id: *id,
                    title: title.clone(),
                    messages: *events_count,
                    last_event_at: *last_event_at,
                    expires_at: *expires_at,
                    local: *user,
                }
            })
            .collect::<Vec<_>>();

        let count = conversations.len();
        let skip = count.saturating_sub(limit);

        conversations.sort_by(|a, b| match self.sort {
            Some(Sort::Created) => a.id.timestamp().cmp(&b.id.timestamp()),
            Some(Sort::Messages) => a.messages.cmp(&b.messages),
            Some(Sort::Activity) => a.last_event_at.cmp(&b.last_event_at),
            Some(Sort::Expires) => a.expires_at.cmp(&b.expires_at),
            Some(Sort::Local) => a.local.cmp(&b.local),
            _ => a.id.cmp(&b.id),
        });

        if self.descending {
            conversations.reverse();
        }

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
                active_conversation_id,
                local_column,
                title_column,
                expires_at_column,
                details,
            ));
        }

        Ok(Success::Table { header, rows })
    }

    fn build_conversation_row(
        &self,
        ctx: &Ctx,
        active_conversation_id: ConversationId,
        local_column: bool,
        title_column: bool,
        expires_at_column: bool,
        details: Details,
    ) -> Row {
        let Details {
            id,
            title,
            messages,
            last_event_at: last_message_at,
            expires_at,
            local,
        } = details;

        let mut id_fmt = if id == active_conversation_id {
            id.to_string().bold().yellow().to_string()
        } else {
            id.to_string()
        };

        if ctx.term.args.hyperlinks {
            id_fmt = hyperlink(format!("jp://show-metadata/{id}"), id_fmt);
        }

        let messages_fmt = if ctx.term.args.hyperlinks {
            hyperlink(format!("jp://show-events/{id}"), messages.to_string())
        } else {
            messages.to_string()
        };

        let last_message_at_fmt = if self.full {
            last_message_at
                .and_then(|t| {
                    let format = "%Y-%m-%d %H:%M:%S";
                    let local_offset: FixedOffset = *Local::now().offset();

                    Some(t.with_timezone(&local_offset).format(format).to_string())
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
