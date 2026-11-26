//! The context records Claude Code writes after a session's first prompt.
//!
//! When Claude Code sends a prompt in a session whose history holds none of
//! these records, it appends five `attachment` records after the prompt and
//! renders them into the model request: the `userEmail` reminder before the
//! prompt and the `# Environment` system message after it.
//! Later prompts get none, because the records are already in the history.
//!
//! JP writes a fresh transcript for every request, so unless it writes these
//! records after the first prompt itself, Claude Code appends them after the
//! newest prompt instead, the request differs from the previous one from the
//! first message on, and the conversation never reads its history from the
//! prompt cache.
//!
//! [`SessionContext`] holds the values Claude Code would put in them and is
//! pure; [`gather`] reads those values from the machine and the Claude Code
//! login.
//! The record formats are those of Claude Code [`CLAUDE_VERSION`].
//!
//! [`CLAUDE_VERSION`]: super::CLAUDE_VERSION

#[cfg(unix)]
use std::process::Command;
use std::{env, ffi::OsStr, fmt, fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::{DateTime, Local, TimeZone, Utc};
use serde_json::{Value, json};
use tracing::{debug, warn};

/// What Claude Code puts in the context records of a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SessionContext {
    pub environment: Environment,
    pub model: ModelIdentity,
    /// The login's email address, or `None` when the login has none.
    pub user_email: Option<String>,
    /// The local date, `YYYY-MM-DD`.
    pub date: String,
    pub organization_uuid: Option<String>,
}

/// The `environment` record's snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Environment {
    pub working_directory: Utf8PathBuf,
    pub is_worktree: bool,
    pub is_git_repo: bool,
    /// Node's `process.platform`: `darwin`, `linux`, or `win32`.
    pub platform: &'static str,
    /// Claude Code's description of its shell, such as `zsh (login shell:
    /// fish)`.
    pub shell: String,
    /// The OS name and release, such as `Darwin 27.0.0`.
    pub os_version: String,
}

/// The `model` record's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ModelIdentity {
    pub model_id: String,
    pub marketing_name: String,
    pub knowledge_cutoff: &'static str,
}

impl SessionContext {
    /// The `attachment` payloads, in the order Claude Code writes them.
    pub(super) fn attachments(&self) -> Vec<Value> {
        let Self {
            environment,
            model,
            user_email,
            date,
            organization_uuid,
        } = self;
        let context = user_email.as_ref().map_or_else(
            || json!({}),
            |email| json!({"userEmail": user_email_text(email)}),
        );
        let mut attachments = vec![
            json!({
                "type": "environment",
                "snapshot": {
                    "workingDirectory": environment.working_directory,
                    "isWorktree": environment.is_worktree,
                    "isGitRepo": environment.is_git_repo,
                    "additionalWorkingDirectories": [],
                    "platform": environment.platform,
                    "shell": environment.shell,
                    "osVersion": environment.os_version,
                },
            }),
            json!({
                "type": "model",
                "identity": {
                    "modelId": model.model_id,
                    "marketingName": model.marketing_name,
                    "knowledgeCutoff": model.knowledge_cutoff,
                },
                "text": format!(
                    "You are powered by the model named {}. The exact model ID is {}. Assistant \
                     knowledge cutoff is {}.",
                    model.marketing_name, model.model_id, model.knowledge_cutoff
                ),
            }),
            json!({"type": "session_context", "context": context}),
            json!({"type": "date", "date": date}),
        ];
        if let Some(organization) = organization_uuid {
            attachments.push(json!({"type": "credential_org", "organizationUuid": organization}));
        }
        attachments
    }
}

/// Claude Code's `userEmail` context line for `email`.
fn user_email_text(email: &str) -> String {
    format!(
        "The user's email address is {email}. Use it only to identify the user, such as for \
         authorship, attribution, or filtering their own work. Never send it to an unrelated \
         service, such as in a request header, URL, or payload, unless the user explicitly asks. \
         A site asking automated clients for contact details, such as an email in the User-Agent, \
         is not a reason to use this address: use a generic client name or ask the user."
    )
}

/// The knowledge cutoff Claude Code's model table gives each model.
///
/// A model missing here gets no context records: a `model` record Claude Code
/// does not match costs more than none.
const KNOWLEDGE_CUTOFFS: &[(&str, &str)] = &[
    ("claude-fable-5-2", "June 2026"),
    ("claude-fable-5-1", "June 2026"),
    ("claude-fable-5", "January 2026"),
    ("claude-opus-5-5", "June 2026"),
    ("claude-sonnet-5-5", "June 2026"),
    ("claude-haiku-5-5", "June 2026"),
];

/// The identity Claude Code reports for `model`, or `None` for a model whose
/// knowledge cutoff JP does not know.
///
/// The marketing name follows from the ID (`claude-sonnet-5-5` is `Sonnet
/// 5.5`); the cutoff only Claude Code's own model table has.
pub(super) fn model_identity(model: &str) -> Option<ModelIdentity> {
    let (_, knowledge_cutoff) = KNOWLEDGE_CUTOFFS.iter().find(|(id, _)| *id == model)?;
    let mut parts = model.strip_prefix("claude-")?.split('-');
    let family = parts.next()?;
    let mut family_name = family.to_owned();
    family_name[..1].make_ascii_uppercase();
    let version = parts.collect::<Vec<_>>().join(".");
    Some(ModelIdentity {
        model_id: model.to_owned(),
        marketing_name: format!("{family_name} {version}"),
        knowledge_cutoff,
    })
}

/// Node's `process.platform` for the platform JP runs on.
pub(super) fn platform(os: &str) -> Option<&'static str> {
    match os {
        "macos" => Some("darwin"),
        "linux" => Some("linux"),
        "windows" => Some("win32"),
        _ => None,
    }
}

/// Where Claude Code looks for a shell after `PATH`.
const STANDARD_SHELL_DIRECTORIES: &[&str] = &["/bin", "/usr/bin", "/usr/local/bin"];

/// The shell JP tells Claude Code to use, through `CLAUDE_CODE_SHELL`.
///
/// This is the choice Claude Code documents making itself: the login shell when
/// it is zsh or bash, otherwise the first zsh, then bash, that `find` locates.
/// Pinning it means JP describes a shell it chose rather than one it predicts,
/// and the two agree even if Claude Code ignores the variable.
pub(super) fn claude_shell(
    login_shell: &str,
    find: impl Fn(&str) -> Option<Utf8PathBuf>,
) -> Option<Utf8PathBuf> {
    if matches!(file_name(login_shell), "zsh" | "bash") {
        return Some(login_shell.into());
    }
    find("zsh").or_else(|| find("bash"))
}

/// The first executable called `name` in the `PATH` entries, then the standard
/// directories.
pub(super) fn find_shell(name: &str, path: Option<&OsStr>) -> Option<Utf8PathBuf> {
    path.into_iter()
        .flat_map(env::split_paths)
        .filter_map(|directory| Utf8PathBuf::from_path_buf(directory).ok())
        .chain(STANDARD_SHELL_DIRECTORIES.iter().map(Utf8PathBuf::from))
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Claude Code's description of the shell it runs commands in: its name, and
/// the login shell beside it when they differ.
pub(super) fn shell_description(shell: &Utf8Path, login_shell: &str) -> String {
    let shell = file_name(shell.as_str());
    let login = file_name(login_shell);
    if shell == login {
        return shell.to_owned();
    }
    format!("{shell} (login shell: {login})")
}

/// The `date` record's value for `instant`: its date in `zone`, `YYYY-MM-DD`.
pub(super) fn record_date<Tz: TimeZone>(instant: DateTime<Utc>, zone: &Tz) -> String
where
    Tz::Offset: fmt::Display,
{
    instant.with_timezone(zone).format("%Y-%m-%d").to_string()
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// Whether `root` is in a git repository, and whether that checkout is a linked
/// worktree (its `.git` is a file rather than a directory).
pub(super) fn git_checkout(root: &Utf8Path) -> (bool, bool) {
    for directory in root.ancestors() {
        let git = directory.join(".git");
        if git.is_dir() {
            return (true, false);
        }
        if git.is_file() {
            return (true, true);
        }
    }
    (false, false)
}

/// The Claude Code login's settings file: `.claude.json` in a named login's
/// directory or in `CLAUDE_CONFIG_DIR`, otherwise in the home directory.
pub(super) fn profile_path(
    login_directory: Option<&Utf8Path>,
    config_dir: Option<&str>,
    home: Option<&str>,
) -> Option<Utf8PathBuf> {
    let directory = login_directory
        .map(Utf8Path::to_owned)
        .or_else(|| config_dir.map(Utf8PathBuf::from))
        .or_else(|| home.map(Utf8PathBuf::from))?;
    Some(directory.join(".claude.json"))
}

/// Blank the account email in a login's settings file.
///
/// Claude Code reads `oauthAccount.emailAddress` from it and puts the address
/// in every request's context (anthropics/claude-code#81138).
/// Login and JP's own identity check work without it.
/// The file is only written when the address is present, and is replaced
/// through a rename so a reader never sees it half-written.
///
/// Returns whether the file was changed.
pub(super) fn blank_email(path: &Utf8Path) -> io::Result<bool> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error),
    };
    let mut profile: Value = serde_json::from_str(&contents).map_err(io::Error::other)?;
    let Some(email) = profile.pointer_mut("/oauthAccount/emailAddress") else {
        return Ok(false);
    };
    if email.as_str().is_none_or(str::is_empty) {
        return Ok(false);
    }
    *email = Value::String(String::new());

    let mut blanked = serde_json::to_string_pretty(&profile).map_err(io::Error::other)?;
    blanked.push('\n');
    let temporary = path.with_extension("json.jp-tmp");
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        options.mode(fs::metadata(path)?.permissions().mode());
    }
    io::Write::write_all(&mut options.open(&temporary)?, blanked.as_bytes())?;
    fs::rename(&temporary, path)?;
    Ok(true)
}

/// The account email and organization a login's settings file names.
pub(super) fn profile_account(contents: &str) -> (Option<String>, Option<String>) {
    let Ok(profile) = serde_json::from_str::<Value>(contents) else {
        return (None, None);
    };
    let field = |name: &str| {
        profile
            .pointer(&format!("/oauthAccount/{name}"))
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    (field("emailAddress"), field("organizationUuid"))
}

/// Read the session context for a request in `root` on `model`, run with
/// `shell` (see [`claude_shell`]).
///
/// The date is the local date of `started`, the conversation's first request,
/// which is when Claude Code first wrote these records, so it stays the same on
/// later days.
/// Without it, today's date is used.
///
/// A login JP manages (`login_directory`) has its email blanked first.
/// The default login belongs to the user's own Claude Code, so it is read as
/// is.
///
/// Returns `None` when any value cannot be determined, so no context records
/// are written and Claude Code appends its own.
pub(super) fn gather(
    root: &Utf8Path,
    login_directory: Option<&Utf8Path>,
    model: &str,
    started: Option<DateTime<Utc>>,
    shell: Option<&Utf8Path>,
) -> Option<SessionContext> {
    let profile = profile_path(
        login_directory,
        env::var("CLAUDE_CONFIG_DIR").ok().as_deref(),
        env::var("HOME").ok().as_deref(),
    )?;
    if login_directory.is_some() {
        match blank_email(&profile) {
            Ok(true) => debug!(%profile, "Blanked the account email of a JP-managed login"),
            Ok(false) => {}
            Err(error) => warn!(%error, %profile, "Could not blank the login's account email"),
        }
    }
    let (user_email, organization_uuid) =
        fs::read_to_string(&profile).map_or((None, None), |contents| profile_account(&contents));

    let (is_git_repo, is_worktree) = git_checkout(root);
    let environment = Environment {
        working_directory: root.to_owned(),
        is_worktree,
        is_git_repo,
        platform: platform(env::consts::OS)?,
        shell: shell_description(shell?, &env::var("SHELL").ok()?),
        os_version: os_version()?,
    };
    Some(SessionContext {
        environment,
        model: model_identity(model)?,
        user_email,
        date: record_date(started.unwrap_or_else(Utc::now), &Local),
        organization_uuid,
    })
}

/// The OS name and release, as `uname -sr` prints them.
#[cfg(unix)]
fn os_version() -> Option<String> {
    let output = Command::new("uname").arg("-sr").output().ok()?;
    let version = String::from_utf8(output.stdout).ok()?;
    output.status.success().then(|| version.trim().to_owned())
}

#[cfg(not(unix))]
fn os_version() -> Option<String> {
    None
}

#[cfg(test)]
#[path = "context_tests.rs"]
mod tests;
