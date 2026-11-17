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
    fs::write(site_dir.join(".gitignore"), "").unwrap();

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
            "docs/package.json, docs/.vitepress/theme/index.js, docs/index.md, docs/.gitignore \
             are missing, re-run `jp vite init`."
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

/// `packageManager` in `package.json` takes precedence over lockfiles, and its
/// version distinguishes Yarn 1 from Yarn 2+.
#[test]
fn the_package_manager_is_read_from_package_json() {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("pnpm-lock.yaml"), "").unwrap();

    for (named, expected) in [
        ("yarn@4.9.1", Manager::Yarn),
        ("yarn@4.9.1+sha512.abc", Manager::Yarn),
        ("yarn@1.22.22", Manager::YarnClassic),
        ("pnpm@9.0.0", Manager::Pnpm),
        ("bun@1.2.0", Manager::Bun),
        ("npm@10.0.0", Manager::Npm),
    ] {
        fs::write(
            dir.path().join("package.json"),
            json!({ "packageManager": named }).to_string(),
        )
        .unwrap();

        assert_eq!(manager(dir.path()), expected, "{named}");
    }
}

#[test]
fn a_yarn_lockfile_tells_yarn_1_from_later_yarns() {
    let dir = Utf8TempDir::new().unwrap();

    fs::write(
        dir.path().join("yarn.lock"),
        "# yarn lockfile v1\n\nvue@^3:\n  version \"3.5.0\"\n",
    )
    .unwrap();
    assert_eq!(manager(dir.path()), Manager::YarnClassic);

    fs::write(
        dir.path().join("yarn.lock"),
        "__metadata:\n  version: 8\n\n\"vue@npm:^3\":\n  version: 3.5.0\n",
    )
    .unwrap();
    assert_eq!(manager(dir.path()), Manager::Yarn);

    fs::write(
        dir.path().join("package.json"),
        r#"{"packageManager": "yarn"}"#,
    )
    .unwrap();
    assert_eq!(manager(dir.path()), Manager::Yarn);
}

fn site(package_json: &str, lockfile: Option<(&str, &str)>) -> Utf8TempDir {
    let dir = Utf8TempDir::new().unwrap();
    fs::write(dir.path().join("package.json"), package_json).unwrap();
    if let Some((name, contents)) = lockfile {
        fs::write(dir.path().join(name), contents).unwrap();
    }

    dir
}

const YARN_4: &str = r#"{"packageManager": "yarn@4.9.1"}"#;
const BERRY_LOCK: &str = "__metadata:\n  version: 8\n";

#[test]
fn an_install_with_a_lockfile_is_held_to_it() {
    for (package_json, lockfile, expected) in [
        ("{}", ("package-lock.json", "{}"), "npm ci"),
        ("{}", ("npm-shrinkwrap.json", "{}"), "npm ci"),
        (
            YARN_4,
            ("yarn.lock", BERRY_LOCK),
            "yarn install --immutable",
        ),
        (
            "{}",
            ("yarn.lock", "# yarn lockfile v1\n"),
            "yarn install --frozen-lockfile",
        ),
        (
            "{}",
            ("pnpm-lock.yaml", ""),
            "pnpm install --frozen-lockfile",
        ),
        ("{}", ("bun.lock", ""), "bun install --frozen-lockfile"),
        ("{}", ("bun.lockb", ""), "bun install --frozen-lockfile"),
    ] {
        let dir = site(package_json, Some(lockfile));
        let install = install(dir.path(), false);

        assert_eq!(install.to_string(), expected, "{lockfile:?}");
        assert_eq!(install.lockfile, Some(lockfile.0), "{lockfile:?}");
    }
}

#[test]
fn an_install_without_a_lockfile_writes_one() {
    for (package_json, expected) in [
        ("{}", "npm install"),
        (YARN_4, "yarn install --no-immutable"),
        (r#"{"packageManager": "yarn@1.22.22"}"#, "yarn install"),
        (
            r#"{"packageManager": "pnpm@9.0.0"}"#,
            "pnpm install --no-frozen-lockfile",
        ),
        (r#"{"packageManager": "bun@1.2.0"}"#, "bun install"),
    ] {
        let dir = site(package_json, None);
        let install = install(dir.path(), false);

        assert_eq!(install.to_string(), expected, "{package_json}");
        assert_eq!(install.lockfile, None, "{package_json}");
    }
}

#[test]
fn an_offline_install_is_kept_off_the_network() {
    for (package_json, lockfile, expected) in [
        ("{}", ("package-lock.json", "{}"), "npm ci --offline"),
        (
            YARN_4,
            ("yarn.lock", BERRY_LOCK),
            "COREPACK_ENABLE_NETWORK=0 YARN_ENABLE_NETWORK=0 yarn install --immutable",
        ),
        (
            "{}",
            ("yarn.lock", "# yarn lockfile v1\n"),
            "COREPACK_ENABLE_NETWORK=0 yarn install --frozen-lockfile --offline",
        ),
        (
            "{}",
            ("pnpm-lock.yaml", ""),
            "COREPACK_ENABLE_NETWORK=0 pnpm install --frozen-lockfile --offline",
        ),
        (
            "{}",
            ("bun.lock", ""),
            "bun install --frozen-lockfile --offline",
        ),
    ] {
        let dir = site(package_json, Some(lockfile));

        assert_eq!(
            install(dir.path(), true).to_string(),
            expected,
            "{lockfile:?}"
        );
    }
}

#[test]
fn a_missing_package_manager_is_named_with_how_to_get_it() {
    let dir = site(YARN_4, Some(("yarn.lock", BERRY_LOCK)));
    let missing = io::Error::from(io::ErrorKind::NotFound);

    assert_eq!(
        unstarted(&install(dir.path(), false), &missing),
        "`yarn` isn't on your PATH, and the site's dependencies are installed with it. Install \
         it, or run `corepack enable` if your Node.js comes with Corepack, then run `jp vite \
         serve` again."
    );

    let npm = site("{}", None);
    assert_eq!(
        unstarted(&install(npm.path(), false), &missing),
        "`npm` isn't on your PATH, and the site's dependencies are installed with it. Install \
         Node.js, which comes with it, then run `jp vite serve` again."
    );
}

#[test]
fn a_package_manager_that_fails_to_start_otherwise_says_why() {
    let dir = site("{}", None);
    let denied = io::Error::new(io::ErrorKind::PermissionDenied, "permission denied");

    assert_eq!(
        unstarted(&install(dir.path(), false), &denied),
        "`npm install` couldn't be started: permission denied"
    );
}

#[test]
fn a_failed_install_against_a_lockfile_says_how_to_update_it() {
    let dir = site(YARN_4, Some(("yarn.lock", BERRY_LOCK)));

    assert_eq!(
        failed(
            &install(dir.path(), false),
            Utf8Path::new("docs"),
            "exit status: 1"
        ),
        "`yarn install --immutable` failed in docs (exit status: 1). If yarn.lock doesn't match \
         package.json, run `yarn install` in docs to update it."
    );
}

#[test]
fn a_failed_offline_install_says_the_cache_may_lack_a_dependency() {
    let dir = site("{}", None);

    assert_eq!(
        failed(
            &install(dir.path(), true),
            Utf8Path::new("docs"),
            "exit status: 1"
        ),
        "`npm install --offline` failed in docs (exit status: 1). With `--offline`, a dependency \
         missing from the package manager's cache fails the install: run without it to download \
         what's missing."
    );
}

#[cfg(unix)]
fn spawn(script: &str) -> Child {
    Command::new("sh")
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[cfg(unix)]
fn printed(output: &[u8]) -> Vec<(String, String)> {
    String::from_utf8(output.to_vec())
        .unwrap()
        .lines()
        .map(|line| match serde_json::from_str(line).unwrap() {
            PluginToHost::Print(print) => (print.channel, print.text),
            other => panic!("expected a print, got {other:?}"),
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn everything_a_child_prints_reaches_the_host_before_its_status() {
    let (events, received) = mpsc::channel();
    let mut output = vec![];
    let child = spawn("echo one; echo two >&2; echo three; exit 3");
    // Wait for the child to exit before calling `follow`, so `try_wait`
    // succeeds on the first poll while all output is still buffered in the
    // pipes.
    thread::sleep(Duration::from_millis(300));

    let status = follow(&mut output, child, &events, &received)
        .unwrap()
        .unwrap();

    assert_eq!(status.code(), Some(3));
    // stdout and stderr are read on separate threads, so their relative order
    // is non-deterministic.
    let mut lines = printed(&output);
    lines.sort();
    assert_eq!(lines, [
        ("chrome".to_owned(), "one\n".to_owned()),
        ("chrome".to_owned(), "three\n".to_owned()),
        ("chrome".to_owned(), "two\n".to_owned()),
    ]);
}

#[cfg(unix)]
#[test]
fn a_shutdown_stops_the_child() {
    let (events, received) = mpsc::channel();
    events
        .send(Event::Host(Ok(HostToPlugin::Shutdown)))
        .unwrap();
    let started = Instant::now();

    let status = follow(&mut vec![], spawn("exec sleep 30"), &events, &received).unwrap();

    assert_eq!(status, None);
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "{:?}",
        started.elapsed()
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
