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

    /// Show only private conversations.
    #[arg(long)]
    private: bool,
}

#[derive(Debug, Default, Clone, Copy, clap::ValueEnum)]
enum Sort {
    #[default]
    Id,
    Created,
    Activity,
    Messages,
    Private,
}

struct Details {
    id: ConversationId,
    messages: usize,
    last_message_at: Option<UtcDateTime>,
    private: bool,
}

impl Args {
    #[expect(clippy::unnecessary_wraps)]
    pub fn run(self, ctx: &mut Ctx) -> Output {
        let active_conversation_id = ctx.workspace.active_conversation_id();
        let limit = self.limit.unwrap_or(usize::MAX);
        let mut conversations = ctx
            .workspace
            .conversations()
            .filter(|(_, c)| !self.private || c.private)
            .map(|(id, c)| (id, c, ctx.workspace.get_messages(id)))
            .map(|(id, c, messages)| Details {
                id: *id,
                messages: messages.len(),
                last_message_at: messages.last().map(|m| m.timestamp),
                private: c.private,
            })
            .collect::<Vec<_>>();

        let count = conversations.len();
        let skip = count.saturating_sub(limit);

        conversations.sort_by(|a, b| match self.sort {
            Some(Sort::Created) => a.id.timestamp().cmp(&b.id.timestamp()),
            Some(Sort::Messages) => a.messages.cmp(&b.messages),
            Some(Sort::Activity) => a.last_message_at.cmp(&b.last_message_at),
            Some(Sort::Private) => a.private.cmp(&b.private),
            _ => a.id.cmp(&b.id),
        });

        if self.descending {
            conversations.reverse();
        }

        let mut header = Row::new();
        header.add_cell(Cell::new("ID"));
        header.add_cell(Cell::new("#").set_alignment(CellAlignment::Right));
        header.add_cell(Cell::new("Activity").set_alignment(CellAlignment::Right));

        // Show "private" column if any conversations are private.
        let mut private_column = false;
        if conversations.iter().skip(skip).any(|d| d.private) {
            private_column = true;
            header.add_cell(Cell::new("Private").set_alignment(CellAlignment::Right));
        }

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

        for Details {
            id,
            messages,
            last_message_at,
            private,
        } in conversations.into_iter().skip(skip)
        {
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
                        let local_offset =
                            UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

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
            row.add_cell(Cell::new(messages_fmt));
            row.add_cell(Cell::new(last_message_at_fmt));
            if private_column {
                let private = if private {
                    "Yes".bold().yellow().to_string()
                } else {
                    "No".to_string()
                };

                row.add_cell(Cell::new(private));
            }

            rows.push(row);
        }

        Ok(Success::Table { header, rows })
    }
}
