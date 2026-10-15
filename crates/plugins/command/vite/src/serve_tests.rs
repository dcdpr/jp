use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;
use crate::site::Owned;

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
    };

    let problem = problems(&[], Utf8Path::new(SNAPSHOT), &revisions).unwrap();

    assert_eq!(
        problem,
        format!(
            "`jp ticket` was built with site files ffffffffffffffff, and `jp vite` with {}, so the \
             site may misread the tickets `jp ticket` writes. Install the three from one JP \
             revision:\n  cargo install --git https://github.com/dcdpr/jp --rev <rev> jp-ticket \
             jp-rfd jp-vite",
            site::REVISION
        )
    );
}

/// "A plugin missing for an existing directory fails the check, naming the
/// command that installs it."
#[test]
fn a_missing_plugin_is_named_with_how_to_install_it() {
    let revisions = Revisions {
        rfd: Some(Err("unknown command `rfd`".to_owned())),
        ticket: None,
    };

    assert_eq!(
        problems(&[], Utf8Path::new(SNAPSHOT), &revisions).unwrap(),
        "`jp rfd` didn't answer (unknown command `rfd`). The site shows RFDs, and reads them the \
         way `jp rfd` writes them. Install it from the JP revision `jp-vite` came from, in one \
         command with the other two:\n  cargo install --git https://github.com/dcdpr/jp --rev \
         <rev> jp-ticket jp-rfd jp-vite"
    );
}

#[test]
fn a_plugin_older_than_the_revision_check_is_named() {
    let revisions = Revisions {
        rfd: None,
        ticket: Some(Ok("jp ticket 0.1.0\n".to_owned())),
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

fn settings(root: &Utf8TempDir) -> Settings {
    snapshot(root.path(), &Options::default(), &json!({}), None).unwrap()
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

/// A configuration that changes while the site runs reaches it; one that
/// doesn't is no reason to write.
#[test]
fn a_reloaded_configuration_gives_new_settings_only_when_they_differ() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();
    let current = settings(&root);

    assert_eq!(
        reloaded(root.path(), &Options::default(), &json!({}), None, &current),
        Ok(None)
    );

    let labelled = json!({ "plugins": { "command": { "ticket": { "options": {
        "labels": { "type": { "description": "The type", "values": ["bug"] } }
    } } } } });
    let fresh = reloaded(root.path(), &Options::default(), &labelled, None, &current)
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
        reloaded(root.path(), &Options::default(), &moved, None, &current),
        Ok(None)
    );
}
