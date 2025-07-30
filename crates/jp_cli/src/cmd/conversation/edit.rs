use crossterm::style::Stylize as _;
use jp_config::Config;
use jp_conversation::{event::ConversationEvent, ConversationId};
use jp_llm::{provider, structured_completion};
use jp_query::structured::conversation_titles;

use crate::{cmd::Success, ctx::Ctx, Output};

#[derive(Debug, clap::Args)]
#[group(required = true, id = "edit")]
#[command(arg_required_else_help = true)]
pub(crate) struct Edit {
    /// Conversation ID to edit. Defaults to active conversation.
    id: Option<ConversationId>,

    /// Toggle the conversation between user and workspace-scoped.
    ///
    /// A user-scoped conversation is stored on your local machine and is not
    /// part of the workspace storage. This means, when using a VCS, user
    /// conversations are not stored in the VCS, but are otherwise identical to
    /// workspace conversations.
    #[arg(long, group = "edit")]
    local: Option<Option<bool>>,

    /// Edit the title of the conversation.
    #[arg(long, group = "edit", conflicts_with = "no_title")]
    title: Option<Option<String>>,

    /// Remove the title of the conversation.
    #[arg(long, group = "edit", conflicts_with = "title")]
    no_title: bool,
}

impl Edit {
    pub(crate) async fn run(self, ctx: &mut Ctx) -> Output {
        let active_id = ctx.workspace.active_conversation_id();
        let id = self.id.unwrap_or(active_id);
        let events = ctx.workspace.get_events(&id).to_vec();
        let Some(conversation) = ctx.workspace.get_conversation_mut(&id) else {
            return Err(
                format!("Conversation {} not found", id.to_string().bold().yellow()).into(),
            );
        };

        if let Some(user) = self.local {
            conversation.user = user.unwrap_or(!conversation.user);
        }

        if let Some(title) = self.title {
            let title = match title {
                Some(title) => title,
                None => generate_titles(&ctx.config, events, vec![]).await?,
            };

            conversation.title = Some(title);
        } else if self.no_title {
            conversation.title = None;
        }

        Ok(Success::Message("Conversation updated.".into()))
    }
}

async fn generate_titles(
    config: &Config,
    events: Vec<ConversationEvent>,
    mut rejected: Vec<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let count = 3;
    let parameters = &config.conversation.title.generate.model.parameters;
    let model_id = &config
        .conversation
        .title
        .generate
        .model
        .id
        .clone()
        .ok_or(jp_model::Error::MissingId)?;

    let provider = provider::get_provider(model_id.provider(), &config.assistant.provider)?;
    let query = conversation_titles(count, events.clone(), &rejected)?;
    let titles: Vec<String> =
        structured_completion(provider.as_ref(), model_id, parameters, query).await?;

    let mut choices = titles.clone();
    choices.extend(rejected.clone());
    choices.push("More...".to_owned());
    choices.push("Manually enter a title".to_owned());

    let result = inquire::Select::new("Conversation Title", choices).prompt()?;
    match result.as_str() {
        "More..." => {
            rejected.extend(titles);
            Box::pin(generate_titles(config, events, rejected)).await
        }
        "Manually enter a title" => {
            let title = inquire::Text::new("Title").prompt()?;
            Ok(title.trim().to_owned())
        }
        choice => Ok(choice.to_owned()),
    }
}
