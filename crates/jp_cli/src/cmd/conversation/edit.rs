use std::time::Duration;

use chrono::Utc;
use jp_config::{
    AppConfig, PartialAppConfig, ToPartial as _, model::id::PartialModelIdOrAliasConfig,
};
use jp_conversation::{ConversationId, ConversationStream};
use jp_llm::{provider, structured};
use jp_printer::PrinterWriter;

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

    /// Set the expiration time of the conversation.
    #[arg(long = "tmp", group = "edit")]
    expires_at: Option<Option<humantime::Duration>>,

    /// Remove the expiration time of the conversation.
    #[arg(long = "no-tmp", group = "edit", conflicts_with = "expires_at")]
    no_expires_at: bool,

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

        if let Some(user) = self.local {
            let mut conversation = ctx.workspace.try_get_conversation_mut(&id)?;
            conversation.user = user.unwrap_or(!conversation.user);
        }

        if let Some(title) = self.title {
            let events = ctx.workspace.try_get_events(&id)?.clone();
            let title = match title {
                Some(title) => title,
                None => {
                    generate_titles(&ctx.config(), ctx.printer.out_writer(), events, vec![]).await?
                }
            };

            ctx.workspace.try_get_conversation_mut(&id)?.title = Some(title);
        } else if self.no_title {
            ctx.workspace.try_get_conversation_mut(&id)?.title = None;
        }

        if let Some(ephemeral) = self.expires_at {
            let mut conversation = ctx.workspace.try_get_conversation_mut(&id)?;
            let duration = ephemeral.map_or(Duration::ZERO, Into::into);
            conversation.expires_at = Some(Utc::now() + duration);
        } else if self.no_expires_at {
            ctx.workspace.try_get_conversation_mut(&id)?.expires_at = None;
        }

        Ok(Success::Message("Conversation updated.".into()))
    }
}

async fn generate_titles(
    config: &AppConfig,
    mut writer: PrinterWriter<'_>,
    mut events: ConversationStream,
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

    let mut partial = PartialAppConfig::empty();
    partial.assistant.model.id = PartialModelIdOrAliasConfig::Id(model_id.to_partial());
    events.add_config_delta(partial);

    let provider = provider::get_provider(model_id.provider, &config.providers.llm)?;
    let query = structured::titles::titles(count, events.clone(), &rejected)?;
    let titles: Vec<String> = structured::completion(provider.as_ref(), &model_id, query).await?;

    let mut choices = titles.clone();
    choices.extend(rejected.clone());
    choices.push("More...".to_owned());
    choices.push("Manually enter a title".to_owned());

    let result =
        inquire::Select::new("Conversation Title", choices).prompt_with_writer(&mut writer)?;

    match result.as_str() {
        "More..." => {
            rejected.extend(titles);
            Box::pin(generate_titles(config, writer, events, rejected)).await
        }
        "Manually enter a title" => {
            let title = inquire::Text::new("Title").prompt_with_writer(&mut writer)?;
            Ok(title.trim().to_owned())
        }
        choice => Ok(choice.to_owned()),
    }
}
