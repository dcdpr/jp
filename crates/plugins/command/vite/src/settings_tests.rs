use std::fs;

use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

/// A workspace with a site in `docs` and both workflows inside it.
fn workspace() -> Utf8TempDir {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/rfd")).unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();

    root
}

fn options(dir: &str) -> Options {
    Options {
        dir: Utf8PathBuf::from(dir),
        repository: None,
    }
}

#[test]
fn both_workflows_are_read_where_the_plugins_keep_them() {
    let root = workspace();
    let config = json!({
        "plugins": { "command": { "ticket": { "options": {
            "board": { "done_limit": 3 },
            "labels": {
                "type": {
                    "description": "The type",
                    "values": ["bug"],
                    "retired": ["chore"],
                    "min": 1,
                    "max": 1
                },
                "triaged": { "description": "Looked at" }
            }
        } } } }
    });

    let settings = snapshot(root.path(), &options("docs"), &config, None).unwrap();

    assert_eq!(settings, Settings {
        revision: site::REVISION.to_owned(),
        site: "docs".to_owned(),
        repository: None,
        rfd: Some(RfdSettings {
            dir: "docs/rfd".to_owned(),
            route: "rfd".to_owned(),
            categories: vec![
                "design".to_owned(),
                "decision".to_owned(),
                "guide".to_owned(),
                "process".to_owned(),
            ],
        }),
        ticket: Some(TicketSettings {
            dir: "docs/ticket".to_owned(),
            route: "ticket".to_owned(),
            done_limit: 3,
            labels: BTreeMap::from([
                ("triaged".to_owned(), Label {
                    description: "Looked at".to_owned(),
                    values: vec![],
                    retired: vec![],
                    min: 0,
                    max: None,
                }),
                ("type".to_owned(), Label {
                    description: "The type".to_owned(),
                    values: vec!["bug".to_owned()],
                    retired: vec!["chore".to_owned()],
                    min: 1,
                    max: Some(1),
                }),
            ]),
        }),
    });
}

/// "Each when that directory exists": a project without tickets has no ticket
/// pages, and its site loads none.
#[test]
fn a_workflow_without_its_directory_is_left_out() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("docs/ticket")).unwrap();

    let settings = snapshot(root.path(), &options("docs"), &json!({}), None).unwrap();

    assert_eq!(settings.rfd, None);
    assert_eq!(settings.ticket.unwrap().route, "ticket");
}

/// A directory's route follows its path below the source directory.
#[test]
fn a_route_follows_the_directory_below_the_site() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("site/design/rfd")).unwrap();
    let config = json!({
        "plugins": { "command": { "rfd": { "options": { "dir": "site/./design/../design/rfd" } } } }
    });

    let rfd = snapshot(root.path(), &options("site"), &config, None)
        .unwrap()
        .rfd
        .unwrap();

    assert_eq!(rfd.dir, "site/design/rfd");
    assert_eq!(rfd.route, "design/rfd");
}

#[test]
fn a_site_at_the_workspace_root_is_named_dot() {
    let root = workspace();

    let settings = snapshot(root.path(), &options("."), &json!({}), None).unwrap();

    assert_eq!(settings.site, ".");
    assert_eq!(settings.rfd.unwrap().route, "docs/rfd");
}

/// VitePress serves pages from the source directory and nowhere else.
#[test]
fn a_workflow_outside_the_site_is_refused() {
    let root = workspace();
    fs::create_dir_all(root.path().join("site")).unwrap();

    let error = snapshot(root.path(), &options("site"), &json!({}), None).unwrap_err();

    assert_eq!(
        error,
        "`plugins.command.rfd.options.dir` (docs/rfd) isn't inside the site's source directory \
         (site), and VitePress serves pages from nowhere else. Move it there, or point \
         `plugins.command.vite.options.dir` at a directory that holds it."
    );
}

/// A workflow that doesn't exist can't be served, so where it would be doesn't
/// matter.
#[test]
fn a_missing_workflow_outside_the_site_is_no_problem() {
    let root = Utf8TempDir::new().unwrap();
    fs::create_dir_all(root.path().join("site")).unwrap();

    let settings = snapshot(root.path(), &options("site"), &json!({}), None).unwrap();

    assert_eq!((settings.rfd, settings.ticket), (None, None));
}

#[test]
fn a_site_outside_the_workspace_is_refused() {
    let root = workspace();

    assert_eq!(
        snapshot(root.path(), &options("../elsewhere"), &json!({}), None).unwrap_err(),
        "`plugins.command.vite.options.dir` (../elsewhere) is outside the workspace."
    );
}

#[test]
fn the_repository_is_the_configured_one_or_origins_web_address() {
    let root = workspace();
    let configured = Options {
        repository: Some("https://example.com/team/site".to_owned()),
        ..options("docs")
    };

    assert_eq!(
        snapshot(
            root.path(),
            &configured,
            &json!({}),
            Some("git@github.com:dcdpr/jp.git")
        )
        .unwrap()
        .repository
        .as_deref(),
        Some("https://example.com/team/site")
    );
    assert_eq!(
        snapshot(
            root.path(),
            &options("docs"),
            &json!({}),
            Some("git@github.com:dcdpr/jp.git")
        )
        .unwrap()
        .repository
        .as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
    assert_eq!(
        snapshot(
            root.path(),
            &options("docs"),
            &json!({}),
            Some("/srv/git/jp")
        )
        .unwrap()
        .repository,
        None
    );
}

/// The snapshot is committed, so the same settings always write the same text.
#[test]
fn the_snapshot_renders_in_a_fixed_shape() {
    let settings = Settings {
        revision: "0123456789abcdef".to_owned(),
        site: "docs".to_owned(),
        repository: Some("https://github.com/dcdpr/jp".to_owned()),
        rfd: None,
        ticket: Some(TicketSettings {
            dir: "docs/ticket".to_owned(),
            route: "ticket".to_owned(),
            done_limit: 8,
            labels: BTreeMap::from([
                ("type".to_owned(), Label {
                    description: "The type".to_owned(),
                    values: vec!["bug".to_owned()],
                    retired: vec![],
                    min: 1,
                    max: Some(1),
                }),
                ("area".to_owned(), Label {
                    description: "The area".to_owned(),
                    values: vec!["cli".to_owned()],
                    retired: vec![],
                    min: 0,
                    max: None,
                }),
            ]),
        }),
    };

    assert_eq!(
        settings.render(),
        r#"{
  "revision": "0123456789abcdef",
  "site": "docs",
  "repository": "https://github.com/dcdpr/jp",
  "rfd": null,
  "ticket": {
    "dir": "docs/ticket",
    "route": "ticket",
    "doneLimit": 8,
    "labels": {
      "area": {
        "description": "The area",
        "values": [
          "cli"
        ]
      },
      "type": {
        "description": "The type",
        "values": [
          "bug"
        ],
        "min": 1,
        "max": 1
      }
    }
  }
}
"#
    );
}

#[test]
fn below_is_strictly_below() {
    assert_eq!(
        below(Utf8Path::new("/repo"), Utf8Path::new("/repo/docs/rfd")).as_deref(),
        Some("docs/rfd")
    );
    assert_eq!(below(Utf8Path::new("/repo"), Utf8Path::new("/repo")), None);
    assert_eq!(
        below(Utf8Path::new("/repo/docs"), Utf8Path::new("/repo/doc")),
        None
    );
    assert_eq!(
        below(Utf8Path::new("/repo/docs"), Utf8Path::new("/repo/docsite")),
        None
    );
}
