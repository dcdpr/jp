use crossterm::style::Stylize as _;
use jp_config::AppConfig;
use jp_conversation::{ConversationId, event::ConversationEvent};
use jp_llm::{provider, structured};

use crate::{Output, cmd::Success, ctx::Ctx};

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

        if let Some(user) = self.local {
            match ctx.workspace.get_conversation_mut(&id) {
                Some(conversation) => conversation.user = user.unwrap_or(!conversation.user),
                None => return missing_conversation(&id),
            }
        }

        if let Some(title) = self.title {
            let title = match title {
                Some(title) => title,
                None => generate_titles(ctx.config(), events, vec![]).await?,
            };

            match ctx.workspace.get_conversation_mut(&id) {
                Some(conversation) => conversation.title = Some(title),
                None => return missing_conversation(&id),
            }
        } else if self.no_title {
            match ctx.workspace.get_conversation_mut(&id) {
                Some(conversation) => conversation.title = None,
                None => return missing_conversation(&id),
            }
        }

        Ok(Success::Message("Conversation updated.".into()))
    }
}

fn missing_conversation(id: &ConversationId) -> Output {
    Err(format!("Conversation {} not found", id.to_string().bold().yellow()).into())
}

async fn generate_titles(
    config: &AppConfig,
    events: Vec<ConversationEvent>,
    mut rejected: Vec<String>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let count = 3;
    let model = config
        .conversation
        .title
        .generate
        .model
        .clone()
        .unwrap_or_else(|| config.assistant.model.clone());

    let model_id = model.id.finalize(&config.providers.llm.aliases)?;

    let provider = provider::get_provider(model_id.provider, &config.providers.llm)?;
    let query = structured::titles::titles(count, events.clone(), &rejected)?;
    let titles: Vec<String> =
        structured::completion(provider.as_ref(), &model_id, &model.parameters, query).await?;

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
