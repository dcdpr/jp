//! `bash` — run shell commands inside a throwaway container.
//!
//! Each call starts a fresh container from `options.image`, runs the requested
//! commands under `set -euo pipefail`, and removes the container when the
//! script exits.
//! Nothing carries over between calls.
//!
//! Two grants decide what the container can see:
//!
//! - Workspace paths named in `mounts` go through [`Context::check_read`] and
//!   are bind-mounted read-only under [`MOUNT_ROOT`].
//! - Variables named in `envs` are matched against `access.env`.
//!   A granting rule forwards the variable, a denying rule refuses the call,
//!   and a variable no rule mentions is escalated to the user as an inquiry.
//!
//! [`runtime`] holds the container seam: which runtime to use and how to spell
//! its command line.

use camino::{Utf8Path, Utf8PathBuf};
use jp_tool::{Context, Outcome, Question, lexical_workspace_relative};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::{
    Error, Tool,
    bash::runtime::{RunSpec, Runtime, argv, detect},
    to_xml,
    util::{
        OneOrMany, ToolResult,
        runner::{DuctProcessRunner, ProcessOutput, ProcessRunner, RunnerOpts},
        truncate,
    },
};

mod runtime;

/// Image used when the tool config does not set `options.image`.
///
/// Carries `curl`, `jq`, `bash`, `git`, `coreutils`, and `zip`, and runs as an
/// unprivileged user — so a command cannot install anything the image does not
/// already ship.
const DEFAULT_IMAGE: &str = "registry.gitlab.com/gitlab-ci-utils/curl-jq:5.0.2";

/// Directory inside the container that workspace mounts are placed under.
const MOUNT_ROOT: &str = "/workspace";

/// Truncate stdout and stderr beyond this limit so a single runaway command
/// cannot fill the assistant's context window.
const MAX_OUTPUT_BYTES: usize = 100_000;

/// Prefix of the inquiry asking to expose one ungranted variable.
const EXPOSE_ENV_PREFIX: &str = "expose_env_";

/// Host variables forwarded to the container runtime itself.
///
/// The runtime process runs with a cleared environment so nothing can reach the
/// container except what `--env` names, but the runtime client still needs to
/// find its binaries, its config, and its daemon socket.
const RUNTIME_PASSTHROUGH: &[&str] = &[
    "PATH",
    "HOME",
    "XDG_RUNTIME_DIR",
    "DOCKER_HOST",
    "DOCKER_CONTEXT",
    "DOCKER_CONFIG",
    "CONTAINER_HOST",
];

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    let plan = match resolve(&ctx, &t, |name| std::env::var(name).ok())? {
        Resolution::Stop(outcome) => return Ok(outcome),
        Resolution::Run(plan) => plan,
    };

    let Some(runtime) = detect() else {
        return Ok(refuse(
            "No container runtime found on PATH. Install one of: container, docker, podman.",
        ));
    };

    execute(&ctx.root, runtime, &plan, &DuctProcessRunner)
}

/// The container invocation a call resolves to, once policy has been applied.
#[derive(Debug, PartialEq)]
struct Plan {
    image: String,

    /// Host path and container path for each read-only bind mount.
    mounts: Vec<(Utf8PathBuf, String)>,

    /// Variable names and the values read from this process's environment.
    envs: Vec<(String, String)>,

    script: String,
}

/// Either something to run, or an outcome to hand back to the caller as-is.
#[derive(Debug, PartialEq)]
enum Resolution {
    Run(Plan),
    Stop(Outcome),
}

/// Validate the arguments and apply the access policy.
///
/// Returns a [`Plan`] only when every requested mount and variable is
/// permitted; anything else (an argument preview, an inquiry, a refusal)
/// short-circuits into [`Resolution::Stop`].
///
/// `lookup` reads a variable's value from the host environment.
fn resolve(
    ctx: &Context,
    t: &Tool,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Resolution, Error> {
    let commands = t
        .req::<OneOrMany<String>>("commands")?
        .into_vec()
        .into_iter()
        .filter(|command| !command.trim().is_empty())
        .collect::<Vec<_>>();
    let envs = t
        .opt::<OneOrMany<String>>("envs")?
        .unwrap_or_default()
        .into_vec();
    let mounts = t
        .opt::<OneOrMany<String>>("mounts")?
        .unwrap_or_default()
        .into_vec();

    if ctx.action.is_format_arguments() {
        return Ok(Resolution::Stop(
            format_preview(&commands, &envs, &mounts).into(),
        ));
    }

    if commands.is_empty() {
        return Ok(retry("`commands` must contain at least one command."));
    }

    let image = match t.options.get("image") {
        None | Some(Value::Null) => DEFAULT_IMAGE.to_owned(),
        Some(Value::String(image)) => image.clone(),
        // Falling back to the default image would run something other than what
        // the operator configured, and report success for it.
        Some(other) => {
            return Err(format!(
                "Invalid `image` option for tool 'bash': expected a string, got {other}"
            )
            .into());
        }
    };

    if let Some(name) = envs.iter().find(|name| !is_valid_env_name(name)) {
        return Ok(retry(format!(
            "'{name}' is not a valid environment variable name; names must match \
             [A-Za-z_][A-Za-z0-9_]*."
        )));
    }

    let envs = match resolve_envs(ctx, &t.answers, &envs, lookup) {
        Ok(envs) => envs,
        Err(outcome) => return Ok(Resolution::Stop(outcome)),
    };

    let mounts = match resolve_mounts(ctx, &mounts) {
        Ok(mounts) => mounts,
        Err(outcome) => return Ok(Resolution::Stop(outcome)),
    };

    Ok(Resolution::Run(Plan {
        image,
        mounts,
        envs,
        script: build_script(&commands),
    }))
}

/// Run the plan under `runtime` and render the result.
fn execute<R: ProcessRunner>(
    root: &Utf8Path,
    runtime: Runtime,
    plan: &Plan,
    runner: &R,
) -> ToolResult {
    let spec = RunSpec {
        image: plan.image.clone(),
        mounts: plan.mounts.clone(),
        envs: plan.envs.iter().map(|(name, _)| name.clone()).collect(),
        workdir: (!plan.mounts.is_empty()).then(|| MOUNT_ROOT.to_owned()),
        script: plan.script.clone(),
    };

    let (program, args) = argv(runtime, &spec);
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();

    let mut env: Vec<(String, String)> = RUNTIME_PASSTHROUGH
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_owned(), value))
        })
        .collect();
    env.extend(plan.envs.iter().cloned());
    let env_refs: Vec<(&str, &str)> = env
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();

    let ProcessOutput {
        stdout,
        stderr,
        status,
    } = runner.run_with_opts(&program, &arg_refs, root, &RunnerOpts {
        env: &env_refs,
        clean_env: true,
        ..RunnerOpts::default()
    })?;

    Ok(to_xml(CommandOutput {
        stdout: truncate(stdout.trim_end(), MAX_OUTPUT_BYTES),
        stderr: truncate(stderr.trim_end(), MAX_OUTPUT_BYTES),
        status: status.to_string(),
    })?
    .into())
}

/// Resolve each requested variable to its value, or stop the call.
///
/// A variable no `access.env` rule mentions becomes a boolean inquiry rather
/// than a refusal, so the user can grant it for this call without editing
/// config.
/// The question carries only the variable's name — its value is read here,
/// after the answer comes back, so it never reaches the prompt or the
/// conversation stream.
///
/// Unlike `access.fs`, an empty rule list denies rather than permits: silently
/// handing over every variable in the environment on request is not a default
/// worth having.
fn resolve_envs(
    ctx: &Context,
    answers: &Map<String, Value>,
    names: &[String],
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<Vec<(String, String)>, Outcome> {
    let mut resolved = Vec::with_capacity(names.len());

    for name in names {
        let rule = ctx
            .access
            .as_ref()
            .and_then(|policy| policy.matching_env_rule(name));

        match rule.map(|rule| rule.read) {
            Some(true) => {}
            Some(false) => {
                return Err(refuse(format!(
                    "Access to environment variable '{name}' is denied by this tool's access.env \
                     configuration."
                )));
            }
            None => {
                let id = format!("{EXPOSE_ENV_PREFIX}{name}");
                match answers.get(&id).and_then(Value::as_bool) {
                    Some(true) => {}
                    Some(false) => {
                        return Err(refuse(format!(
                            "Not exposing environment variable '{name}': the user declined."
                        )));
                    }
                    None => {
                        let question = Question::boolean(
                            id,
                            format!("Expose the environment variable '{name}' to the container?"),
                        )
                        .map_err(|error| Outcome::fail(&error))?
                        .with_default(Value::Bool(false));
                        return Err(Outcome::NeedsInput { question });
                    }
                }
            }
        }

        let Some(value) = lookup(name) else {
            return Err(refuse(format!(
                "Environment variable '{name}' is not set on the host."
            )));
        };
        resolved.push((name.clone(), value));
    }

    Ok(resolved)
}

/// Resolve each requested mount to a host path and a container path.
///
/// Paths are checked with [`Context::check_read`], which enforces the
/// workspace-relative and `access.fs` boundaries, then placed under
/// [`MOUNT_ROOT`] at the path the caller asked for.
fn resolve_mounts(ctx: &Context, inputs: &[String]) -> Result<Vec<(Utf8PathBuf, String)>, Outcome> {
    let mut resolved = Vec::with_capacity(inputs.len());

    for input in inputs {
        let path = Utf8Path::new(input);
        let host = ctx
            .check_read(path)
            .map_err(|error| refuse(format!("Cannot mount '{input}': {error}")))?;

        if !host.exists() {
            return Err(refuse(format!("Cannot mount '{input}': no such path.")));
        }

        // `check_read` already rejected absolute and escaping inputs, so the
        // lexical form exists; the target mirrors what the caller wrote rather
        // than the canonical path, so an in-workspace symlink appears in the
        // container under the name it was asked for.
        let relative = lexical_workspace_relative(path)
            .ok_or_else(|| refuse(format!("Cannot mount '{input}': not workspace-relative.")))?;

        let target = if relative.as_str().is_empty() {
            MOUNT_ROOT.to_owned()
        } else {
            format!("{MOUNT_ROOT}/{relative}")
        };

        resolved.push((host, target));
    }

    Ok(resolved)
}

/// Join the commands into one script.
///
/// `set -euo pipefail` stops the run at the first failing command instead of
/// letting later commands operate on a state an earlier one failed to produce.
fn build_script(commands: &[String]) -> String {
    let mut script = String::from("set -euo pipefail\n");
    for command in commands {
        script.push_str(command.trim_end());
        script.push('\n');
    }

    script
}

/// Render the approval preview shown before the container runs.
fn format_preview(commands: &[String], envs: &[String], mounts: &[String]) -> String {
    let mut out = String::new();

    if !envs.is_empty() {
        out.push_str("**Environment variables**\n\n");
        for name in envs {
            out.push_str(&format!("- `{name}`\n"));
        }
        out.push('\n');
    }

    if !mounts.is_empty() {
        out.push_str("**Workspace mounts**\n\n");
        for path in mounts {
            out.push_str(&format!("- `{path}`\n"));
        }
        out.push('\n');
    }

    out.push_str("**Commands**\n\n```bash\n");
    for command in commands {
        out.push_str(command.trim_end());
        out.push('\n');
    }
    out.push_str("```\n");

    out
}

/// Whether `name` is a portable environment variable name.
///
/// Anything else is rejected before it can reach the runtime's `--env` flag.
fn is_valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Stop the call with an error the assistant can correct by calling again with
/// different arguments.
fn retry(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Resolution {
    Resolution::Stop(Outcome::error(message.into().as_ref()))
}

/// Stop the call with a refusal that retrying cannot fix.
fn refuse(message: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Outcome {
    Outcome::fail(message.into().as_ref())
}

#[derive(Serialize)]
struct CommandOutput {
    #[serde(skip_serializing_if = "String::is_empty")]
    stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    stderr: String,
    /// Always reported: for a shell script the exit code is the primary signal,
    /// and an omitted field is easy to misread as "no output".
    status: String,
}

#[cfg(test)]
#[path = "bash_tests.rs"]
mod tests;
