use std::fs;

use jp_config::PartialAppConfig;

use super::Target;
use crate::{Error, cmd::Output, ctx::Ctx};

#[derive(Debug, clap::Args)]
pub(crate) struct Fmt {
    #[command(flatten)]
    target: Target,

    /// Run fmt in check mode.
    #[arg(long)]
    check: bool,

    /// Format all configuration files.
    #[arg(long)]
    all: bool,
}

impl Fmt {
    pub(crate) fn run(self, ctx: &mut Ctx) -> Output {
        let targets = if self.target == Target::default() {
            vec![
                Target::default(),
                Target {
                    user_workspace: true,
                    ..Default::default()
                },
                Target {
                    user_global: true,
                    ..Default::default()
                },
                Target {
                    cwd: true,
                    ..Default::default()
                },
            ]
        } else {
            vec![self.target]
        };

        let mut ok = true;
        let mut msg = String::new();
        let results = targets
            .into_iter()
            .map(|target| self.fmt_target(target, ctx));

        for result in results {
            match result {
                Ok(v) if !v.trim().is_empty() => {
                    msg.push_str(v.trim());
                    msg.push('\n');
                }
                Ok(_) => {}
                Err(err) => {
                    ok = false;
                    msg.push_str(&err.to_string());
                    msg.push('\n');
                    break;
                }
            }
        }

        if ok {
            ctx.printer.println(msg.trim());
            Ok(())
        } else {
            Err(Error::CliConfig(msg).into())
        }
    }

    fn fmt_target(
        &self,
        target: Target,
        ctx: &mut Ctx,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut config = match target.config_file(ctx) {
            Ok(Some(config)) => config,
            Ok(None) => return Ok(String::new()),
            // Missing files is not an error.
            Err(jp_config::fs::ConfigLoaderError::NotFound { .. }) => {
                return Ok(String::new());
            }
            Err(error) => return Err(Box::new(error)),
        };

        config.format_content::<PartialAppConfig>()?;

        let curr = fs::read_to_string(&config.path)?;
        if self.check {
            if curr != config.content {
                return Err(Error::CliConfig(format!(
                    "Configuration file {} is not formatted correctly.",
                    config.path,
                ))
                .into());
            }

            Ok(format!("Checked configuration file: {}", config.path))
        } else if curr != config.content {
            fs::write(&config.path, config.content)?;
            Ok(format!("Formatted configuration file: {}", config.path))
        } else {
            Ok(format!(
                "Skipped formatted configuration file: {}",
                config.path,
            ))
        }
    }
}
