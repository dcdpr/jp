use comfy_table::{Cell, CellAlignment, Row};
use crossterm::style::{Color, Stylize as _};
use jp_conversation::ConversationId;
use jp_term::osc::hyperlink;
use time::{macros::format_description, UtcDateTime, UtcOffset};

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
pub struct Args {
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
    Messages,
    Local,
}

struct Details {
    id: ConversationId,
    title: Option<String>,
    messages: usize,
    last_message_at: Option<UtcDateTime>,
    local: bool,
}

impl Args {
    #[expect(clippy::unnecessary_wraps)]
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let active_conversation_id = ctx.workspace.active_conversation_id();
        let limit = self.limit.unwrap_or(usize::MAX);
        let mut conversations = ctx
            .workspace
            .conversations()
            .filter(|(_, c)| !self.local || c.local)
            .map(|(id, c)| (id, c, ctx.workspace.get_messages(id)))
            .map(|(id, c, messages)| Details {
                id: *id,
                title: c.title.clone(),
                messages: messages.len(),
                last_message_at: messages.last().map(|m| m.timestamp),
                local: c.local,
            })
            .collect::<Vec<_>>();

        let count = conversations.len();
        let skip = count.saturating_sub(limit);

        conversations.sort_by(|a, b| match self.sort {
            Some(Sort::Created) => a.id.timestamp().cmp(&b.id.timestamp()),
            Some(Sort::Messages) => a.messages.cmp(&b.messages),
            Some(Sort::Activity) => a.last_message_at.cmp(&b.last_message_at),
            Some(Sort::Local) => a.local.cmp(&b.local),
            _ => a.id.cmp(&b.id),
        });

        if self.descending {
            conversations.reverse();
        }

        let conversations: Vec<_> = conversations.into_iter().skip(skip).collect();
        let (local_column, title_column, header) = build_header_row(&conversations);

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

        for details in conversations.into_iter().skip(skip) {
            rows.push(self.build_conversation_row(
                ctx,
                active_conversation_id,
                local_column,
                title_column,
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
        details: Details,
    ) -> Row {
        let Details {
            id,
            title,
            messages,
            last_message_at,
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
            hyperlink(format!("jp://show-messages/{id}"), messages.to_string())
        } else {
            messages.to_string()
        };

        let last_message_at_fmt = if self.full {
            last_message_at
                .and_then(|t| {
                    let format =
                        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]");
                    let local_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

                    t.to_offset(local_offset).format(&format).ok()
                })
                .unwrap_or_default()
        } else {
            last_message_at.map_or_else(String::new, |t| {
                let ago = (UtcDateTime::now() - t).try_into().expect("valid duration");
                timeago::Formatter::new().convert(ago)
            })
        };

        let mut row = Row::new();
        row.add_cell(Cell::new(id_fmt));
        row.add_cell(Cell::new(messages_fmt).set_alignment(CellAlignment::Right));
        row.add_cell(Cell::new(last_message_at_fmt).set_alignment(CellAlignment::Right));
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

fn build_header_row(conversations: &[Details]) -> (bool, bool, Row) {
    let mut header = Row::new();
    header.add_cell(Cell::new("ID"));
    header.add_cell(Cell::new("#").set_alignment(CellAlignment::Right));
    header.add_cell(Cell::new("Activity").set_alignment(CellAlignment::Right));

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

    (local_column, title_column, header)
}
