use std::{fs, sync::Arc};

use camino::Utf8PathBuf;
use camino_tempfile::{Utf8TempDir, tempdir};
use chrono::Utc;
use jp_config::{
    AppConfig, PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
};
use jp_plugin::{message::PathsInfo, registry::ApprovedPlugin};
use jp_printer::OutputFormat;
use pretty_assertions::assert_eq;
use serde_json::{Value, json};

use super::*;
use crate::cmd::plugin::discovery::{Location, ManifestState};

/// The configuration a turn resolved, with `assignments` applied as `--cfg`
/// would apply them.
fn turn_config(assignments: &[&str]) -> Arc<AppConfig> {
    let mut partial = PartialAppConfig::new_test();
    for assignment in assignments {
        partial
            .assign(assignment.parse::<KvAssignment>().unwrap())
            .unwrap();
    }

    Arc::new(jp_config::util::build(partial).unwrap())
}

/// What every plugin call of a turn under `config` is told.
fn plugin_init(config: Arc<AppConfig>) -> PluginInit {
    PluginInit {
        workspace_id: "ws-abc".to_owned(),
        storage: Some("/ws/.jp".into()),
        paths: PathsInfo::default(),
        config,
        log_level: 0,
    }
}

// --- Which plugins a turn needs ---

/// A turn's tools: two on `ticket`, one disabled on `metrics`, one locked off
/// on `secrets`, and a local tool.
fn turn_tools() -> Arc<AppConfig> {
    turn_config(&[
        "conversation.tools.ticket_create.source=plugin.command.ticket.create",
        "conversation.tools.ticket_show.source=plugin.command.ticket.show",
        "conversation.tools.metrics_dump.source=plugin.command.metrics",
        "conversation.tools.metrics_dump.enable=false",
        "conversation.tools.secrets_read.source=plugin.command.secrets",
        "conversation.tools.secrets_read.enable.state=false",
        "conversation.tools.secrets_read.enable.allow_toggle=never",
        "conversation.tools.word_count.source=local",
        "conversation.tools.word_count.command=wc",
    ])
}

#[test]
fn a_turn_needs_the_plugins_its_enabled_tools_run_through() {
    let config = turn_tools();

    let needed = plugins_needed(&config.conversation.tools, None);

    assert_eq!(needed, BTreeSet::from(["ticket".to_owned()]));
}

/// A forced tool is offered even when disabled, so its plugin is needed too; a
/// locked-off one is never offered, forced or not.
#[test]
fn a_forced_tool_adds_its_plugin_unless_it_is_locked_off() {
    let config = turn_tools();

    let forced = plugins_needed(&config.conversation.tools, Some("metrics_dump"));
    let locked = plugins_needed(&config.conversation.tools, Some("secrets_read"));

    assert_eq!(
        forced,
        BTreeSet::from(["metrics".to_owned(), "ticket".to_owned()])
    );
    assert_eq!(locked, BTreeSet::from(["ticket".to_owned()]));
}

// --- Admission ---

/// A plugin binary in a temporary directory, and an approval store beside it.
struct Machine {
    dir: Utf8TempDir,
    plugins: Vec<LocalPlugin>,
}

impl Machine {
    fn new() -> Self {
        Self {
            dir: tempdir().unwrap(),
            plugins: vec![],
        }
    }

    /// Put a `jp-{name}` binary on this machine, with `contents`.
    fn with_plugin(mut self, name: &str, contents: &str) -> Self {
        let path = self.dir.path().join(format!("jp-{name}"));
        fs::write(&path, contents).unwrap();
        self.plugins.push(LocalPlugin {
            name: name.to_owned(),
            path: path.canonicalize_utf8().unwrap(),
            location: Location::Path,
            manifest: ManifestState::Missing,
        });
        self
    }

    fn path(&self, name: &str) -> Utf8PathBuf {
        self.dir
            .path()
            .join(format!("jp-{name}"))
            .canonicalize_utf8()
            .unwrap()
    }

    fn approvals(&self) -> ApprovalStore {
        ApprovalStore::load_from(Some(self.dir.path().join("approvals.json")))
    }

    /// Record an approval for the binary `name` as it is now.
    fn approve(&self, approvals: &mut ApprovalStore, name: &str) {
        let path = self.path(name);
        approvals
            .record(name, ApprovedPlugin {
                sha256: registry::sha256_file(&path).unwrap(),
                path,
                approved_at: Utc::now(),
                installed: false,
                manifest: None,
            })
            .unwrap();
    }

    /// Admit `needed` without a terminal, under `config`.
    fn admit(&self, needed: &[&str], config: &AppConfig) -> TurnPlugins {
        self.admit_with(needed, config, &mut self.approvals())
    }

    fn admit_with(
        &self,
        needed: &[&str],
        config: &AppConfig,
        approvals: &mut ApprovalStore,
    ) -> TurnPlugins {
        let needed = needed.iter().map(|name| (*name).to_owned()).collect();

        TurnPlugins::admit_from(&needed, Admitter {
            local: &self.plugins,
            registry: None,
            plugins_config: &config.plugins,
            approvals,
            interactive: false,
            printer: &Printer::sink(),
        })
    }
}

#[test]
fn a_plugin_allowed_by_configuration_is_admitted() {
    let machine = Machine::new().with_plugin("ticket", "v1");

    let plugins = machine.admit(
        &["ticket"],
        &turn_config(&["plugins.command.ticket.run=allow"]),
    );

    assert!(plugins.refused().is_empty(), "{:?}", plugins.refused());
    assert_eq!(plugins.admitted["ticket"].binary, machine.path("ticket"));
}

/// What admission recorded is what the tool service checks before each call, so
/// an admitted binary that has not changed runs.
#[test]
fn an_admitted_plugin_passes_the_check_before_each_call() {
    let machine = Machine::new().with_plugin("ticket", "v1");
    let plugins = machine.admit(
        &["ticket"],
        &turn_config(&["plugins.command.ticket.run=allow"]),
    );

    let command_plugins = plugins.into_command_plugins(plugin_init(turn_config(&[])));

    assert_eq!(
        command_plugins.verify("ticket").map(|p| p.binary.clone()),
        Ok(machine.path("ticket"))
    );
}

/// Without a terminal, a binary nobody approved does not run, exactly as `jp
/// ticket` without a terminal refuses it.
#[test]
fn an_unapproved_plugin_is_refused_without_a_terminal() {
    let machine = Machine::new().with_plugin("ticket", "v1");
    let path = machine.path("ticket");

    let plugins = machine.admit(&["ticket"], &turn_config(&[]));

    assert!(plugins.admitted.is_empty());
    assert_eq!(
        plugins.refused(),
        &IndexMap::from([(
            "ticket".to_owned(),
            format!(
                "plugin `ticket` at {path} is not approved. Approve it with `jp plugin approve \
                 {path}`, or set plugins.command.ticket.run = \"allow\" in config."
            )
        )])
    );
}

/// An approval recorded with `jp plugin approve`, or `Y` at the prompt, admits
/// the binary while its contents are the approved ones.
#[test]
fn an_approved_plugin_is_admitted() {
    let machine = Machine::new().with_plugin("ticket", "v1");
    let mut approvals = machine.approvals();
    machine.approve(&mut approvals, "ticket");

    let plugins = machine.admit_with(&["ticket"], &turn_config(&[]), &mut approvals);

    assert!(plugins.refused().is_empty(), "{:?}", plugins.refused());
    assert!(plugins.admitted.contains_key("ticket"));
}

/// The turn's baseline is the digest admission approved.
/// A binary replaced after that is refused at its next call rather than trusted
/// for the rest of the turn.
#[test]
fn the_turn_pins_the_approved_contents() {
    let machine = Machine::new().with_plugin("ticket", "v1");
    let mut approvals = machine.approvals();
    machine.approve(&mut approvals, "ticket");
    let approved = registry::sha256_file(&machine.path("ticket")).unwrap();

    let plugins = machine.admit_with(&["ticket"], &turn_config(&[]), &mut approvals);
    assert_eq!(plugins.admitted["ticket"].sha256, approved);

    fs::write(machine.path("ticket"), "v2").unwrap();
    let command_plugins = plugins.into_command_plugins(plugin_init(turn_config(&[])));

    assert_eq!(
        command_plugins.verify("ticket").map(|p| p.binary.clone()),
        Err(format!(
            "{} changed since it was admitted at the start of this turn",
            machine.path("ticket")
        ))
    );
}

#[test]
fn a_denied_plugin_is_refused_even_when_approved() {
    let machine = Machine::new().with_plugin("ticket", "v1");
    let mut approvals = machine.approvals();
    machine.approve(&mut approvals, "ticket");

    let plugins = machine.admit_with(
        &["ticket"],
        &turn_config(&["plugins.command.ticket.run=deny"]),
        &mut approvals,
    );

    assert!(plugins.admitted.is_empty());
    assert_eq!(
        plugins.refused()["ticket"],
        "plugin `ticket` is denied by configuration (plugins.command.ticket.run = \"deny\")"
    );
}

#[test]
fn a_missing_plugin_is_refused() {
    let machine = Machine::new();

    let plugins = machine.admit(&["ticket"], &turn_config(&[]));

    assert_eq!(
        plugins.refused()["ticket"],
        "no `jp-ticket` binary in the plugin install directory or on $PATH"
    );
}

/// A refused plugin is not handed to the tool service, so its tools cannot run
/// even if one were offered.
#[test]
fn a_refused_plugin_is_not_handed_to_the_tool_service() {
    let machine = Machine::new().with_plugin("ticket", "v1");

    let command_plugins = machine
        .admit(&["ticket"], &turn_config(&[]))
        .into_command_plugins(plugin_init(turn_config(&[])));

    assert_eq!(
        command_plugins.verify("ticket").map(|p| p.binary.clone()),
        Err("it was not admitted for this turn".to_owned())
    );
}

// --- Leaving refused plugins out of the turn ---

fn refused_ticket() -> IndexMap<String, String> {
    IndexMap::from([("ticket".to_owned(), "it is not approved".to_owned())])
}

/// The tool service is handed only admitted plugins, so a tool on any other
/// plugin is left out of the turn: refused, or never needed because the tool is
/// not offered.
#[test]
fn a_tool_on_a_plugin_not_admitted_is_left_out_of_the_turn() {
    let config = turn_tools();
    let names = |plugins: &CommandPlugins| {
        let mut names: Vec<&str> =
            without_unadmitted_plugins(config.conversation.tools.iter(), plugins)
                .into_iter()
                .map(|(name, _)| name)
                .collect();
        names.sort_unstable();
        names
    };
    let ticket =
        CommandPlugins::new(plugin_init(turn_config(&[]))).with("ticket", AdmittedPlugin {
            binary: "/bin/jp-ticket".into(),
            sha256: "aaa".to_owned(),
        });

    assert_eq!(names(&CommandPlugins::default()), ["word_count"]);
    assert_eq!(names(&ticket), [
        "ticket_create",
        "ticket_show",
        "word_count"
    ]);
}

/// Naming a tool with `--tool` asks for it; a turn without it is not what was
/// asked for, so the query fails with the reason.
#[test]
fn forcing_a_tool_of_a_refused_plugin_fails_the_turn() {
    let config = turn_tools();
    let tools = &config.conversation.tools;

    let error = refuse_forced_tool(tools, &refused_ticket(), Some("ticket_show")).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Command plugin `ticket` cannot be run from here: it is not approved"
    );
}

/// Only the forced tool's own plugin matters: a refused plugin the forced tool
/// does not run through leaves the turn to start without it.
#[test]
fn forcing_a_tool_of_another_source_starts_the_turn() {
    let config = turn_tools();
    let tools = &config.conversation.tools;

    assert!(refuse_forced_tool(tools, &refused_ticket(), Some("word_count")).is_ok());
    assert!(refuse_forced_tool(tools, &refused_ticket(), None).is_ok());
}

#[test]
fn refused_plugin_report_names_the_tools_and_the_reason() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);

    report_refused_plugins(
        &printer,
        &turn_tools().conversation.tools,
        &refused_ticket(),
        None,
    );
    printer.flush();

    assert_eq!(
        *err.lock(),
        "Plugin 'ticket' did not run; unavailable tools: ticket_create, ticket_show\n  it is not \
         approved\n"
    );
}

#[test]
fn refused_plugin_report_is_ndjson_under_json_format() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Json);

    report_refused_plugins(
        &printer,
        &turn_tools().conversation.tools,
        &refused_ticket(),
        None,
    );
    printer.flush();

    assert_eq!(
        serde_json::from_str::<Value>(err.lock().trim()).unwrap(),
        json!({
            "event": "plugin_unavailable",
            "plugin": "ticket",
            "reason": "it is not approved",
            "tools": ["ticket_create", "ticket_show"],
        })
    );
}
