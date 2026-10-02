use std::collections::BTreeMap;

use pretty_assertions::assert_eq;

use super::*;

fn command(id: &str, official: bool, requires: &[&str]) -> RegistryPlugin {
    RegistryPlugin {
        id: id.to_owned(),
        description: String::new(),
        official,
        repository: None,
        kind: PluginKind::Command {
            requires: requires.iter().map(|s| (*s).to_owned()).collect(),
            suggests: vec![],
            binaries: BTreeMap::new(),
        },
    }
}

fn registry(entries: Vec<(&str, RegistryPlugin)>) -> Registry {
    Registry {
        version: 1,
        plugins: entries
            .into_iter()
            .map(|(key, plugin)| (key.to_owned(), plugin))
            .collect(),
    }
}

fn ids(steps: &[(&str, &RegistryPlugin)]) -> Vec<String> {
    steps.iter().map(|(_, p)| p.id.clone()).collect()
}

#[test]
fn requirements_install_before_the_plugin_that_needs_them() {
    let reg = registry(vec![
        ("serve web", command("serve-web", true, &["serve core"])),
        ("serve core", command("serve-core", true, &["base"])),
        ("base", command("base", true, &[])),
    ]);

    assert_eq!(ids(&plan(&reg, "serve web").unwrap()), [
        "base",
        "serve-core",
        "serve-web"
    ]);
}

/// A command group has no binary, so requiring one installs nothing.
#[test]
fn a_required_group_needs_nothing_installed() {
    let reg = registry(vec![
        ("serve", RegistryPlugin {
            kind: PluginKind::CommandGroup { suggests: vec![] },
            ..command("serve", true, &[])
        }),
        ("serve web", command("serve-web", true, &["serve"])),
    ]);

    assert_eq!(ids(&plan(&reg, "serve web").unwrap()), ["serve-web"]);
}

#[test]
fn a_shared_requirement_is_installed_once() {
    let reg = registry(vec![
        ("a", command("a", true, &["b", "c"])),
        ("b", command("b", true, &["c"])),
        ("c", command("c", true, &[])),
    ]);

    assert_eq!(ids(&plan(&reg, "a").unwrap()), ["c", "b", "a"]);
}

#[test]
fn a_requirement_the_registry_does_not_list_is_refused() {
    let reg = registry(vec![("a", command("a", true, &["gone"]))]);

    assert_eq!(
        plan(&reg, "a").unwrap_err().message.as_deref(),
        Some("a plugin requires `gone`, which the registry does not list")
    );
}

#[test]
fn a_cycle_is_refused() {
    let reg = registry(vec![
        ("a", command("a", true, &["b"])),
        ("b", command("b", true, &["a"])),
    ]);

    assert_eq!(
        plan(&reg, "a").unwrap_err().message.as_deref(),
        Some("the registry's `requires` for `a` form a cycle")
    );
}

/// Installing an official plugin on first use asks nothing, so it must not
/// bring third-party code with it.
#[tokio::test]
async fn an_official_plugin_requiring_a_third_party_one_is_not_installed_unasked() {
    let reg = registry(vec![
        ("serve web", command("serve-web", true, &["extra"])),
        ("extra", command("extra", false, &[])),
    ]);
    let (printer, _out, err) = Printer::memory(jp_printer::OutputFormat::Text);
    let mut approvals = ApprovalStore::default();

    let error = install_official(&reg, "serve web", &mut approvals, &printer)
        .await
        .unwrap_err();

    assert_eq!(
        error.message.as_deref(),
        Some(
            "the official plugin for `jp serve web` requires the third-party plugin `extra`, \
             which is not installed without asking. Run `jp plugin install` for it first."
        )
    );
    printer.flush();
    assert_eq!(*err.lock(), "", "nothing was downloaded");
}
