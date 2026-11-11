use std::ffi::OsStr;

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;
use crate::{git::Remote, site::Owned};

/// What a plugin built alongside this binary's site files prints.
fn current(plugin: &str) -> String {
    format!("jp {plugin} 0.1.0 (site {})\n", site::REVISION)
}

fn changed(path: &str) -> Change {
    Change::Write(Owned {
        path: Utf8PathBuf::from(path),
        contents: vec![],
    })
}

const SNAPSHOT: &str = "docs/.vitepress/jp-vite/settings.json";

#[test]
fn the_revision_is_read_out_of_the_version() {
    assert_eq!(
        reported_revision("jp ticket 0.1.0 (site 0123456789abcdef)\n"),
        Some("0123456789abcdef")
    );
    assert_eq!(reported_revision("jp ticket 0.1.0\n"), None);
    assert_eq!(reported_revision("jp ticket 0.1.0 (site )\n"), None);
}

#[test]
fn a_site_whose_plugins_agree_has_no_problem() {
    let revisions = Revisions {
        rfd: Some(Ok(current("rfd"))),
        ticket: Some(Ok(current("ticket"))),
        wiki: Some(Ok(current("wiki"))),
    };

    assert_eq!(problems(&[], Utf8Path::new(SNAPSHOT), &revisions), None);
}

/// Serve rewrites the snapshot itself, so it is never a reason to distrust the
/// site.
#[test]
fn a_changed_snapshot_is_no_problem() {
    let revisions = Revisions {
        rfd: None,
        ticket: None,
        wiki: None,
    };

    assert_eq!(
        problems(&[changed(SNAPSHOT)], Utf8Path::new(SNAPSHOT), &revisions),
        None
    );
}

#[test]
fn loaders_from_another_release_are_named() {
    let revisions = Revisions {
        rfd: None,
        ticket: None,
        wiki: None,
    };

    assert_eq!(
        problems(
            &[
                changed("docs/.vitepress/jp-vite/loaders/rfd-shared.mjs"),
                Change::Remove(Utf8PathBuf::from("docs/.vitepress/jp-vite/old.mjs")),
                changed(SNAPSHOT),
            ],
            Utf8Path::new(SNAPSHOT),
            &revisions
        )
        .unwrap(),
        "These files aren't the ones this `jp vite` writes, so the site may run with files from \
         another release:\n  docs/.vitepress/jp-vite/loaders/rfd-shared.mjs\n  \
         docs/.vitepress/jp-vite/old.mjs\nRun `jp vite sync` to write them."
    );
}

#[test]
fn a_plugin_built_from_other_site_files_is_named() {
    let revisions = Revisions {
        rfd: Some(Ok(current("rfd"))),
        ticket: Some(Ok("jp ticket 0.1.0 (site ffffffffffffffff)\n".to_owned())),
        wiki: None,
    };

    let problem = problems(&[], Utf8Path::new(SNAPSHOT), &revisions).unwrap();

    assert_eq!(
        problem,
        format!(
            "`jp ticket` was built with site files ffffffffffffffff, and `jp vite` with {}, so the \
             site may misread the tickets `jp ticket` writes. Install them all from one JP \
             revision:\n  cargo install --git https://github.com/dcdpr/jp --rev <rev> jp-ticket \
             jp-rfd jp-wiki jp-vite",
            site::REVISION
        )
    );
}

/// A project with a wiki asks `jp wiki` too.
#[test]
fn a_wiki_plugin_built_from_other_site_files_is_named() {
    let revisions = Revisions {
        rfd: None,
        ticket: None,
        wiki: Some(Ok("jp wiki 0.1.0 (site ffffffffffffffff)\n".to_owned())),
    };

    let problem = problems(&[], Utf8Path::new(SNAPSHOT), &revisions).unwrap();

    assert!(
        problem.starts_with(
            "`jp wiki` was built with site files ffffffffffffffff, and `jp vite` with "
        ),
        "{problem}"
    );
    assert!(
        problem.contains("so the site may misread the wiki pages `jp wiki` writes."),
        "{problem}"
    );
}

/// "A plugin missing for an existing directory fails the check, naming the
/// command that installs it."
#[test]
fn a_missing_plugin_is_named_with_how_to_install_it() {
    let revisions = Revisions {
        rfd: Some(Err("unknown command `rfd`".to_owned())),
        ticket: None,
        wiki: None,
    };

    assert_eq!(
        problems(&[], Utf8Path::new(SNAPSHOT), &revisions).unwrap(),
        "`jp rfd` didn't answer (unknown command `rfd`). The site shows RFDs, and reads them the \
         way `jp rfd` writes them. Install it from the JP revision `jp-vite` came from, in one \
         command with the others:\n  cargo install --git https://github.com/dcdpr/jp --rev \
         <rev> jp-ticket jp-rfd jp-wiki jp-vite"
    );
}

#[test]
fn a_plugin_older_than_the_revision_check_is_named() {
    let revisions = Revisions {
        rfd: None,
        ticket: Some(Ok("jp ticket 0.1.0\n".to_owned())),
        wiki: None,
    };

    assert!(
        problems(&[], Utf8Path::new(SNAPSHOT), &revisions)
            .unwrap()
            .starts_with("`jp ticket` reports no site revision, so it predates `jp vite`")
    );
}

#[test]
fn every_problem_is_reported_at_once() {
    let revisions = Revisions {
        rfd: Some(Err("unknown command `rfd`".to_owned())),
        ticket: Some(Ok("jp ticket 0.1.0\n".to_owned())),
        wiki: None,
    };

    let problem = problems(
        &[changed("docs/ticket/board.md")],
        Utf8Path::new(SNAPSHOT),
        &revisions,
    )
    .unwrap();

    assert_eq!(problem.split("\n\n").count(), 3, "{problem}");
}

#[test]
fn a_directory_without_any_init_file_has_no_site() {
    let root = Utf8TempDir::new().unwrap();
    let site_dir = root.path().join("docs");
    fs::create_dir_all(&site_dir).unwrap();

    assert_eq!(
        unready(root.path(), &site_dir).as_deref(),
        Some("There is no VitePress site in docs: run `jp vite init` to set one up.")
    );
}

/// A site that lost its `package.json` is named by the file, not reported as
/// absent.
#[test]
fn a_site_missing_its_package_json_names_it() {
    let root = Utf8TempDir::new().unwrap();
    let site_dir = root.path().join("docs");
    fs::create_dir_all(site_dir.join(".vitepress/theme")).unwrap();
    fs::write(site_dir.join(".vitepress/config.mts"), "").unwrap();
    fs::write(site_dir.join(".vitepress/theme/index.js"), "").unwrap();
    fs::write(site_dir.join("index.md"), "").unwrap();

    assert_eq!(
        unready(root.path(), &site_dir).as_deref(),
        Some("docs/package.json is missing, re-run `jp vite init`.")
    );
}

#[test]
fn a_site_missing_several_init_files_names_each() {
    let root = Utf8TempDir::new().unwrap();
    let site_dir = root.path().join("docs");
    fs::create_dir_all(site_dir.join(".vitepress")).unwrap();
    fs::write(site_dir.join(".vitepress/config.mts"), "").unwrap();

    assert_eq!(
        unready(root.path(), &site_dir).as_deref(),
        Some(
            "docs/package.json, docs/.vitepress/theme/index.js, docs/index.md are missing, re-run \
             `jp vite init`."
        )
    );
}

/// Only `package.json` is needed to run `vitepress`; the other files are the
/// project's to remove.
#[test]
fn a_site_with_its_package_json_is_ready() {
    let root = Utf8TempDir::new().unwrap();
    let site_dir = root.path().join("docs");
    fs::create_dir_all(&site_dir).unwrap();
    fs::write(site_dir.join("package.json"), "{}").unwrap();

    assert_eq!(unready(root.path(), &site_dir), None);
}

#[test]
fn vitepress_runs_through_the_package_manager_the_project_names() {
    let dir = Utf8TempDir::new().unwrap();

    assert_eq!(runner(dir.path()), ["npx", "vitepress"]);

    fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(runner(dir.path()), ["pnpm", "exec", "vitepress"]);

    fs::write(
        dir.path().join("package.json"),
        r#"{"packageManager": "yarn@4.9.1"}"#,
    )
    .unwrap();
    assert_eq!(runner(dir.path()), ["yarn", "vitepress"]);
}

/// The loaders render release pages with the binary named in `JP_VITE_BIN`, and
/// nothing the site reads from `jp vite` is taken from this process.
#[test]
fn vitepress_is_told_which_binary_renders_releases() {
    let dir = Utf8TempDir::new().unwrap();
    let renderer = Utf8PathBuf::from("/opt/jp/jp-vite");

    let command = vitepress_command(
        dir.path(),
        "build",
        &["--outDir".to_owned(), "out".to_owned()],
        Some(renderer.as_std_path()),
    );

    assert_eq!(command.get_program(), "npx");
    assert_eq!(command.get_args().collect::<Vec<_>>(), [
        "vitepress",
        "build",
        "--outDir",
        "out"
    ]);
    assert_eq!(command.get_current_dir(), Some(dir.path().as_std_path()));
    let mut envs: Vec<_> = command.get_envs().collect();
    envs.sort();
    assert_eq!(envs, [
        (
            OsStr::new("JP_VITE_BIN"),
            Some(OsStr::new("/opt/jp/jp-vite"))
        ),
        (OsStr::new("JP_VITE_PROBLEM"), None),
        (OsStr::new("JP_VITE_SERVE"), None),
    ]);
}

/// Without a binary to name, the loaders run `changelog` from `PATH`, as they
/// do under a `vitepress build` started by hand.
#[test]
fn without_a_known_binary_none_is_named() {
    let dir = Utf8TempDir::new().unwrap();

    let command = vitepress_command(dir.path(), "dev", &[], None);

    assert!(
        command
            .get_envs()
            .any(|(key, value)| key == "JP_VITE_BIN" && value.is_none())
    );
}

fn settings(root: &Utf8TempDir) -> Settings {
    snapshot(
        root.path(),
        &Options::default(),
        &json!({}),
        &Remote::default(),
    )
    .unwrap()
}

#[test]
fn the_snapshot_is_written_only_when_it_changes() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    let first = settings(&root);

    assert_eq!(
        write_snapshot(root.path(), &first, None)
            .unwrap()
            .as_deref(),
        Some("Wrote the site's settings to docs/.vitepress/jp-vite/settings.json.\n")
    );
    assert_eq!(write_snapshot(root.path(), &first, None).unwrap(), None);

    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();
    let second = settings(&root);
    assert!(
        write_snapshot(root.path(), &second, Some(&first))
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fs::read_to_string(root.path().join(SNAPSHOT)).unwrap(),
        second.render()
    );
}

/// A snapshot checked out with CRLF line endings holds the same settings, so
/// starting the site doesn't rewrite it.
#[test]
fn a_snapshot_differing_only_in_line_endings_is_not_rewritten() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    let settings = settings(&root);
    let path = root.path().join(SNAPSHOT);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let crlf = settings.render().replace('\n', "\r\n");
    fs::write(&path, &crlf).unwrap();

    assert_eq!(write_snapshot(root.path(), &settings, None).unwrap(), None);
    assert_eq!(fs::read_to_string(&path).unwrap(), crlf);
}

/// A configuration that changes while the site runs reaches it; one that
/// doesn't is no reason to write.
#[test]
fn a_reloaded_configuration_gives_new_settings_only_when_they_differ() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();
    let current = settings(&root);

    assert_eq!(
        reloaded(
            root.path(),
            &Options::default(),
            &json!({}),
            &Remote::default(),
            &current
        ),
        Ok(None)
    );

    let labelled = json!({ "plugins": { "command": { "ticket": { "options": {
        "labels": { "type": { "description": "The type", "values": ["bug"] } }
    } } } } });
    let fresh = reloaded(
        root.path(),
        &Options::default(),
        &labelled,
        &Remote::default(),
        &current,
    )
    .unwrap()
    .unwrap();
    assert_eq!(fresh.ticket.unwrap().labels.keys().collect::<Vec<_>>(), [
        "type"
    ]);
}

/// The dev server serves the directory it started in.
#[test]
fn a_reloaded_configuration_keeps_the_source_directory() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    let current = settings(&root);

    let moved = json!({ "plugins": { "command": { "vite": { "options": { "dir": "site" } } } } });

    assert_eq!(
        reloaded(
            root.path(),
            &Options::default(),
            &moved,
            &Remote::default(),
            &current
        ),
        Ok(None)
    );
}
