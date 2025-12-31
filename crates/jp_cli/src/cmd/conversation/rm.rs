use crossterm::style::Stylize as _;
use inquire::Confirm;
use jp_conversation::{Conversation, ConversationId, ConversationStream};
use jp_format::conversation::DetailsFmt;

use crate::{Output, cmd::Success, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Rm {
    /// Conversation IDs to remove.
    ///
    /// Defaults to the active conversation if not specified.
    #[arg(conflicts_with = "from")]
    id: Vec<ConversationId>,

    /// Remove all conversations *starting from* the specified conversation,
    /// based on creation date.
    ///
    /// Can be used in combination with `--until` to remove a range of
    /// conversations.
    #[arg(long, conflicts_with = "id")]
    from: Option<ConversationId>,

    /// Remove all conversations *until and excluding* the specified
    /// conversation, based on creation date.
    ///
    /// Can be used in combination with `--from` to remove a range of
    /// conversations.
    #[arg(long, conflicts_with = "id")]
    until: Option<ConversationId>,

    /// Do not prompt for confirmation.
    #[arg(long)]
    yes: bool,
}

impl Rm {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let ids = if !self.id.is_empty() {
            self.id
        } else if self.from.is_none() && self.until.is_none() {
            vec![active_id]
        } else {
            ctx.workspace
                .conversations()
                .map(|(id, _)| *id)
                .filter(|id| self.from.is_none_or(|from| *id >= from))
                .filter(|id| self.until.is_none_or(|until| *id < until))
                .collect::<Vec<_>>()
        };

        for id in ids {
            remove(ctx, id, self.yes)?;
        }

        Ok(Success::Message("Conversation(s) removed.".into()))
    }
}

fn remove(ctx: &mut Ctx, id: ConversationId, force: bool) -> Output {
    let active_id = ctx.workspace.active_conversation_id();

    let conversation = ctx.workspace.get_conversation(&id);
    let events = ctx.workspace.get_events(&id);
    let mut details = DetailsFmt::new(id)
        .with_last_message_at(events.and_then(|v| v.last().map(|v| v.event.timestamp)))
        .with_event_count(events.map(ConversationStream::len).unwrap_or_default())
        .with_title(conversation.and_then(|v| v.title.as_ref()))
        .with_last_activated_at(conversation.map(|v| v.last_activated_at))
        .with_local_flag(conversation.is_some_and(|v| v.user))
        .with_active_conversation(active_id)
        .with_hyperlinks(ctx.term.args.hyperlinks)
        .with_color(ctx.term.args.colors);

    if !force {
        details.title = Some(format!(
            "Removing conversation {}",
            id.to_string().bold().yellow()
        ));
        println!("{details}\n");

        let confirm = Confirm::new("Are you sure?")
            .with_default(false)
            .with_confirm_on_input(true)
            .with_help_message("this action cannot be undone");

        match confirm.prompt() {
            Ok(true) => {}
            Ok(false) | Err(_) => return Err(1.into()),
        }
    }

    // We can't remove the active conversation, so we need to first switch
    // the active conversation to another one, or create a new one if
    // needed.
    if id == active_id {
        #[expect(clippy::map_unwrap_or, reason = "`map_or_else` fails borrow check")]
        let new_active_id = ctx
            .workspace
            .conversations()
            .filter(|(id, _)| *id != &active_id)
            .max_by_key(|(_, conversation)| conversation.last_activated_at)
            .map(|(id, _)| *id)
            .unwrap_or_else(|| {
                ctx.workspace
                    .create_conversation(Conversation::default(), ctx.config())
            });

        ctx.workspace.set_active_conversation_id(new_active_id)?;
    }

    if let Err(err) = ctx.workspace.remove_conversation(&id) {
        return Err(format!(
            "Failed to remove conversation {}: {}",
            id.to_string().bold().yellow(),
            err.to_string().red()
        )
        .into());
    }

    Ok(().into())
}
