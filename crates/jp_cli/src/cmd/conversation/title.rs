//! LLM-driven conversation title generation.

use jp_config::{AppConfig, PartialAppConfig};
use jp_conversation::ConversationStream;
use jp_llm::{
    provider,
    title::{self, TitleRequest},
};
use jp_workspace::{ConversationHandle, Workspace};

use crate::{
    cmd::{
        ConversationLoadRequest, Output,
        conversation_id::PositionalIds,
        lock::{LockOutcome, LockRequest, acquire_lock},
        query::apply_model,
    },
    ctx::{Ctx, IntoPartialAppConfig},
    error::{Error, Result},
};

/// How many candidates are generated when the count isn't given.
pub(super) const DEFAULT_COUNT: usize = 3;

/// Picker entry that discards the current candidates and generates new ones.
const MORE: &str = "More...";

/// Picker entry that takes a hand-written title instead of a generated one.
const MANUAL: &str = "Manually enter a title";

#[derive(Debug, clap::Args)]
pub(crate) struct Title {
    #[command(flatten)]
    target: PositionalIds<true, true>,

    /// How many candidate titles to generate.
    ///
    /// The candidates are offered as a picker, which can also generate a fresh
    /// batch or take a hand-written title.
    /// Without a terminal to pick with, the first candidate is applied.
    #[arg(long, short = 'n', default_value_t = DEFAULT_COUNT, value_parser = parse_count)]
    count: usize,

    /// The model to generate the title with.
    ///
    /// Accepts a model alias or a full `provider/name` ID, the same values as
    /// `jp query --model`.
    /// Overrides `conversation.title.generate.model` for this invocation.
    #[arg(long, short = 'm')]
    model: Option<String>,

    /// Print the candidate titles without applying one.
    #[arg(long)]
    dry_run: bool,
}

fn parse_count(s: &str) -> std::result::Result<usize, String> {
    match s.parse::<usize>() {
        Ok(0) => Err("at least one title must be generated".to_owned()),
        Ok(count) => Ok(count),
        Err(error) => Err(error.to_string()),
    }
}

impl Title {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_session(&self.target)
    }

    pub(crate) async fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        for handle in handles {
            self.title_one(ctx, handle).await?;
        }

        Ok(())
    }

    async fn title_one(&self, ctx: &mut Ctx, handle: ConversationHandle) -> Output {
        let lock = match acquire_lock(LockRequest::from_ctx(handle, ctx)).await? {
            LockOutcome::Acquired(lock) => lock,
            LockOutcome::NewConversation | LockOutcome::ForkConversation(_) => {
                unreachable!("title does not allow new/fork on contention")
            }
        };

        let cfg = ctx.config();
        let conv = lock.into_mut();
        let events = conv.events().clone();

        if self.dry_run {
            let candidates =
                generate(&cfg, events, self.count, self.model.is_some(), vec![]).await?;
            for candidate in candidates {
                ctx.printer.println(candidate);
            }
            return Ok(());
        }

        let title = select(ctx, &cfg, events, self.count, self.model.is_some()).await?;
        ctx.printer.println(title.clone());
        conv.update_metadata(|m| m.title = Some(title));

        Ok(())
    }
}

impl IntoPartialAppConfig for Title {
    fn apply_cli_config(
        &self,
        _: Option<&Workspace>,
        mut partial: PartialAppConfig,
        merged_config: Option<&PartialAppConfig>,
    ) -> std::result::Result<PartialAppConfig, Box<dyn std::error::Error + Send + Sync>> {
        apply_model(&mut partial, self.model.as_deref(), merged_config);

        Ok(partial)
    }
}

/// Generate candidate titles and let the user pick one.
///
/// The picker offers a fresh batch (carrying the discarded candidates forward
/// so the model avoids repeating them) and a hand-written title.
/// Without a terminal there is nothing to pick with, so the first candidate is
/// returned as-is.
///
/// `override_model` reads the model from `assistant.model` instead of
/// `conversation.title.generate.model`; see [`generate`].
pub(super) async fn select(
    ctx: &Ctx,
    cfg: &AppConfig,
    events: ConversationStream,
    count: usize,
    override_model: bool,
) -> Result<String> {
    let mut rejected: Vec<String> = vec![];

    loop {
        let candidates =
            generate(cfg, events.clone(), count, override_model, rejected.clone()).await?;

        if !ctx.term.is_tty {
            return Ok(candidates
                .into_iter()
                .next()
                .expect("`generate` rejects an empty batch"));
        }

        // Discarded candidates stay selectable: a user who asked for more may
        // still prefer one of the earlier suggestions.
        let mut choices = candidates.clone();
        choices.extend(rejected.iter().cloned());
        choices.push(MORE.to_owned());
        choices.push(MANUAL.to_owned());

        let mut writer = ctx.printer.prompt_writer();
        let choice =
            inquire::Select::new("Conversation Title", choices).prompt_with_writer(&mut writer)?;

        match choice.as_str() {
            MORE => rejected.extend(candidates),
            MANUAL => {
                let title = inquire::Text::new("Title").prompt_with_writer(&mut writer)?;
                return Ok(title.trim().to_owned());
            }
            _ => return Ok(choice),
        }
    }
}

/// Generate `count` candidate titles for a conversation.
///
/// The model comes from `conversation.title.generate.model`, falling back to
/// the assistant model.
/// With `override_model` it comes from `assistant.model` unconditionally, which
/// is where the config pipeline puts a `--model` flag.
///
/// `rejected` names titles the model must avoid.
///
/// # Errors
///
/// Returns [`Error::TitleGeneration`] if the model produces no titles, and the
/// underlying LLM error if the request itself fails.
async fn generate(
    cfg: &AppConfig,
    events: ConversationStream,
    count: usize,
    override_model: bool,
    rejected: Vec<String>,
) -> Result<Vec<String>> {
    let override_id = override_model.then(|| cfg.assistant.model.id.clone());
    let model = title::resolve_model(cfg, override_id.as_ref());
    let model_id = model.id.resolved().clone();

    let provider = provider::get_provider(model_id.provider, &cfg.providers.llm)?;
    let details = provider.model_details(&model_id.name).await?;

    let titles = title::generate(provider.as_ref(), &details, TitleRequest {
        events,
        model,
        count,
        rejected,
        max_response_bytes: cfg.assistant.request.max_response_bytes,
    })
    .await?;

    if titles.is_empty() {
        return Err(Error::TitleGeneration {
            model: model_id.to_string(),
            reason: "the model returned no titles".to_owned(),
        });
    }

    Ok(titles)
}

#[cfg(test)]
#[path = "title_tests.rs"]
mod tests;
