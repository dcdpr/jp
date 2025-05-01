use jp_conversation::PersonaId;

use super::Output;
use crate::{error::Error, Ctx};

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Persona name to switch to.
    pub name: String,
}

impl Args {
    pub fn run(self, ctx: &mut Ctx) -> Output {
        // Validate the persona exists
        let persona_id = PersonaId::try_from(&self.name)?;
        ctx.workspace
            .get_persona(&persona_id)
            .ok_or(Error::NotFound("Persona", self.name.clone()))?;

        // Update context with new persona
        ctx.workspace
            .get_active_conversation_mut()
            .context
            .persona_id = persona_id;

        Ok(format!("Switched to persona: {}", self.name).into())
    }
}
