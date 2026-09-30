//! Deciding whether a plugin binary may run.
//!
//! Admission precedes every spawn of a plugin binary, including one that only
//! answers `describe`.
//! [`decide`] is the policy, in the order RFD 077 gives it; [`admit`] gathers
//! what it needs and asks the user when the policy says to.
//!
//! See: `docs/rfd/077-plugin-configuration-and-trust-policy.md`, "Dispatch
//! Integration" and "Approval Store".

use camino::Utf8Path;
use chrono::Utc;
use crossterm::style::Stylize as _;
use jp_config::plugins::{
    PluginsConfig,
    command::{CommandPluginConfig, RunPolicy},
};
use jp_inquire::{InlineOption, InlineSelect};
use jp_plugin::{PROTOCOL_VERSION, registry::ApprovedPlugin};
use jp_printer::Printer;
use tracing::debug;

use super::{
    approvals::{ApprovalMatch, ApprovalStore},
    discovery::LocalPlugin,
    registry,
};
use crate::cmd;

/// What admission knows about the binary it is deciding on.
#[derive(Debug)]
pub(crate) struct Candidate<'a> {
    /// The binary.
    pub plugin: &'a LocalPlugin,

    /// The SHA-256 of its contents.
    pub sha256: &'a str,

    /// Whether its name is an official plugin's.
    pub official: bool,

    /// The SHA-256 the registry publishes for this platform, for an official
    /// plugin.
    pub official_sha256: Option<&'a str>,

    /// The official command a third-party binary replaces.
    pub replaces: Option<&'a str>,

    /// How the binary relates to the approval stored for its name.
    pub approval: ApprovalMatch,
}

/// What admission decided.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Run it.
    Run,

    /// Do not run it, for this reason.
    Refuse(String),

    /// Ask the user, for this reason.
    Ask(Reason),
}

/// Why the user is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Reason {
    /// Nothing answers for this binary yet.
    New,

    /// The approved file changed since it was approved.
    Changed,

    /// Another file with this name is the approved one.
    Elsewhere(camino::Utf8PathBuf),

    /// An official plugin's binary that is not the release the registry
    /// publishes.
    NotTheRelease,
}

/// Decide whether a binary may run, in the order RFD 077 gives:
///
/// 1. `run = "deny"` refuses.
/// 2. A pinned checksum the binary does not match refuses.
/// 3. `run = "allow"` runs.
/// 4. `run = "ask"`, the default, runs an official binary the registry
///    publishes, or one the approval store holds, and asks about anything else.
pub(crate) fn decide(candidate: &Candidate<'_>, config: Option<&CommandPluginConfig>) -> Verdict {
    let name = &candidate.plugin.name;
    let path = &candidate.plugin.path;

    let policy = config.and_then(|c| c.run).unwrap_or_default();
    if policy == RunPolicy::Deny {
        return Verdict::Refuse(format!(
            "plugin `{name}` is denied by configuration (plugins.command.{name}.run = \"deny\")"
        ));
    }

    if let Some(pinned) = config.and_then(|c| c.checksum.as_ref())
        && let Some(refusal) = pin_mismatch(name, path, &pinned.value, candidate.sha256)
    {
        return Verdict::Refuse(refusal);
    }

    if policy == RunPolicy::Allow {
        return Verdict::Run;
    }

    if candidate.official && candidate.official_sha256 == Some(candidate.sha256) {
        return Verdict::Run;
    }

    Verdict::Ask(match &candidate.approval {
        ApprovalMatch::Matches => return Verdict::Run,
        ApprovalMatch::Changed => Reason::Changed,
        ApprovalMatch::Elsewhere(approved) => Reason::Elsewhere(approved.clone()),
        ApprovalMatch::None if candidate.official => Reason::NotTheRelease,
        ApprovalMatch::None => Reason::New,
    })
}

/// Why a binary is refused, when its contents do not match the checksum pinned
/// for it; `None` when they do.
pub(crate) fn pin_mismatch(
    name: &str,
    path: &Utf8Path,
    pinned: &str,
    sha256: &str,
) -> Option<String> {
    (pinned != sha256).then(|| {
        format!(
            "plugin `{name}` binary checksum mismatch.\nexpected: {pinned}\nactual:   \
             {sha256}\nThe binary at {path} has changed since it was pinned. Update \
             plugins.command.{name}.checksum.value in your config to accept the new binary.",
        )
    })
}

/// The lines the prompt shows above its question.
///
/// `emphasis` styles the word that says the binary is third-party.
pub(crate) fn question_lines(
    candidate: &Candidate<'_>,
    reason: &Reason,
    emphasis: impl Fn(&str) -> String,
) -> Vec<String> {
    let plugin = candidate.plugin;
    let name = &plugin.name;
    let command = plugin
        .manifest
        .valid()
        .map(|manifest| manifest.command.join(" "))
        .unwrap_or_default();
    let third_party = emphasis("third-party");

    let mut lines = vec![match (candidate.replaces, candidate.official) {
        (Some(key), _) => format!(
            "`jp {key}` is claimed by the {third_party} plugin `{name}`, which replaces the \
             official one."
        ),
        (None, true) => format!(
            "`jp {command}` is provided by the official plugin `{name}`, but this binary is not \
             the release the registry publishes."
        ),
        (None, false) => {
            let description = plugin
                .manifest
                .valid()
                .map(|manifest| format!(": {}", manifest.description))
                .unwrap_or_default();
            format!("`jp {command}` is provided by the {third_party} plugin `{name}`{description}")
        }
    }];

    lines.push(plugin.path.to_string());

    match reason {
        Reason::Changed => lines.push("It changed since you approved it.".to_owned()),
        Reason::Elsewhere(approved) => {
            lines.push(format!(
                "You approved `{name}` at {approved}, not this file."
            ));
        }
        Reason::New | Reason::NotTheRelease => {}
    }

    lines
}

/// Why a binary nobody can be asked about is refused.
pub(crate) fn not_approved(candidate: &Candidate<'_>, reason: &Reason) -> String {
    let name = &candidate.plugin.name;
    let path = &candidate.plugin.path;

    let why = match reason {
        Reason::New => String::new(),
        Reason::Changed => " (it changed since it was approved)".to_owned(),
        Reason::Elsewhere(approved) => format!(" (`{name}` is approved at {approved})"),
        Reason::NotTheRelease => " (it is not the official release)".to_owned(),
    };

    format!(
        "plugin `{name}` at {path} is not approved{why}. Approve it with `jp plugin approve \
         {path}`, or set plugins.command.{name}.run = \"allow\" in config."
    )
}

/// Refuse a plugin whose manifest asks for a newer protocol than this host
/// speaks, before spawning it.
fn check_protocol(plugin: &LocalPlugin) -> cmd::Output {
    let Some(manifest) = plugin.manifest.valid() else {
        return Ok(());
    };

    if manifest.protocol > PROTOCOL_VERSION {
        return Err(format!(
            "plugin `{}` at {} needs protocol {}, and this `jp` speaks {PROTOCOL_VERSION}. Update \
             `jp`, or install a version of the plugin built for it.",
            plugin.name, plugin.path, manifest.protocol,
        )
        .into());
    }

    Ok(())
}

/// What the registry says about a binary, for [`admit`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Official<'a> {
    /// Whether the binary's name is an official plugin's.
    pub official: bool,

    /// The SHA-256 the registry publishes for this platform.
    pub sha256: Option<&'a str>,

    /// The official command a third-party binary replaces.
    pub replaces: Option<&'a str>,
}

/// Decide whether `plugin` may run, asking the user where the policy says to.
///
/// Records an approval when the user answers `Y`.
///
/// # Errors
///
/// Fails when the plugin is refused, or declined.
pub(crate) fn admit(
    plugin: &LocalPlugin,
    official: Official<'_>,
    plugins_config: &PluginsConfig,
    approvals: &mut ApprovalStore,
    interactive: bool,
    printer: &Printer,
) -> cmd::Output {
    check_protocol(plugin)?;

    let sha256 = registry::sha256_file(&plugin.path)?;
    let candidate = Candidate {
        plugin,
        sha256: &sha256,
        official: official.official,
        official_sha256: official.sha256,
        replaces: official.replaces,
        approval: approvals.check(&plugin.name, &plugin.path, &sha256),
    };

    let reason = match decide(&candidate, plugins_config.command.get(&plugin.name)) {
        Verdict::Run => {
            debug!(name = plugin.name, path = %plugin.path, "Plugin admitted.");
            return Ok(());
        }
        Verdict::Refuse(reason) => return Err(reason.into()),
        Verdict::Ask(reason) => reason,
    };

    if !interactive {
        return Err(not_approved(&candidate, &reason).into());
    }

    let lines = question_lines(&candidate, &reason, |word| word.red().bold().to_string());
    for (index, line) in lines.iter().enumerate() {
        let lead = if index == 0 { "  \u{2192} " } else { "    " };
        printer.prompt_println(format!("{lead}{line}"));
    }

    let answer = InlineSelect::new("Run it?", vec![
        InlineOption::new('y', "run this time"),
        InlineOption::new('Y', "run, and remember this binary"),
        InlineOption::new('n', "don't run it"),
    ])
    .with_default('n')
    .prompt(&mut printer.prompt_writer())
    .map_err(|e| cmd::Error::from(format!("prompt failed: {e}")))?;

    match answer {
        'y' => Ok(()),
        'Y' => approvals.record(&plugin.name, ApprovedPlugin {
            path: plugin.path.clone(),
            sha256,
            approved_at: Utc::now(),
            installed: false,
            manifest: None,
        }),
        _ => Err("plugin execution denied".into()),
    }
}

#[cfg(test)]
#[path = "admission_tests.rs"]
mod tests;
