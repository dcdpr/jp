use std::{env, fs, path::PathBuf};

use anyhow::{anyhow, bail, Context as _, Result};
use clap::Args;

use super::ConfigArgs;
use crate::{
    cmd::canonical_path,
    config::{get_global_config_path, get_local_config_path, Config, WORKSPACE_CONFIG_FILENAME},
    context::Context,
};

#[derive(Args)]
pub struct InitArgs {
    /// Custom path for the config file
    #[arg(value_parser = canonical_path)]
    pub path: Option<PathBuf>,

    /// Force overwrite if the config already exists
    #[arg(short, long)]
    pub force: bool,

    /// Don't inherit from global configuration
    #[arg(long)]
    pub no_inherit: bool,
}

pub async fn run(_ctx: Context, config_args: &ConfigArgs, args: &InitArgs) -> Result<()> {
    let global = config_args.global;
    let config_path = determine_config_path(global, args.path.clone())?;

    if config_path.exists() && !args.force {
        println!("Config file already exists at {:?}", config_path);
        println!("Use --force to overwrite");
        return Ok(());
    }

    // Create directory structure if needed
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)
            .context(format!("Failed to create directory at {:?}", parent))?;
    }

    // Generate the template
    let template = Config::generate_template(true);

    // Apply the no-inherit flag if needed
    let final_template = if args.no_inherit && !global {
        template.replace("#inherit = true", "inherit = false")
    } else {
        template
    };

    // Write to file
    fs::write(&config_path, final_template)?;

    let location = if global { "Global" } else { "Local" };
    println!(
        "{} config generated successfully at {:?}",
        location, config_path
    );

    // If global config is initialized in a non-standard location, suggest environment variable
    if global && args.path.is_some() {
        println!("\nTo use this global config, set the environment variable:");
        println!("export JP_GLOBAL_CONFIG_FILE=\"{}\"", config_path.display());
    }

    // Initialize project state if it's a local config
    if !global {
        if let Some(project_dir) = config_path.parent() {
            crate::initialize_workspace_state(project_dir)?;
            println!("Project state initialized successfully");
        }
    }

    Ok(())
}

fn determine_config_path(global: bool, custom_path: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(mut path) = custom_path {
        if global {
            // For global config: if path ends with .toml, use it; otherwise treat as directory
            if path.extension().is_none_or(|ext| ext != "toml") {
                path = path.join("config.toml");
            }
        } else {
            // For local config: always treat as directory and append .jp.toml
            if path.extension().is_some() {
                bail!(
                    "For local config initialization, path must be a directory. Received: {:?}",
                    path
                );
            }
            path = path.join(WORKSPACE_CONFIG_FILENAME);
        }

        return Ok(path);
    }

    // Handle default paths when no custom path is provided
    if global {
        get_global_config_path(false)
            .ok_or_else(|| anyhow!("Could not determine global config path. Set `JP_GLOBAL_CONFIG_FILE` environment variable."))
    } else {
        let current_dir = env::current_dir()?;
        Ok(get_local_config_path().unwrap_or_else(|| current_dir.join(WORKSPACE_CONFIG_FILENAME)))
    }
}
