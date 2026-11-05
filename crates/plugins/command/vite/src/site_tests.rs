use std::collections::BTreeMap;

use camino::Utf8Path;

use super::*;
use crate::settings::{ReleaseSettings, RfdSettings, TicketSettings, WikiSettings};

/// Settings with a site in `docs` and the workflows `rfd` and `ticket` name.
fn settings(rfd: Option<&str>, ticket: Option<&str>) -> Settings {
    Settings {
        revision: REVISION.to_owned(),
        site: "docs".to_owned(),
        repository: None,
        branch: None,
        wiki: None,
        release: None,
        redirects: BTreeMap::new(),
        rfd: rfd.map(|route| RfdSettings {
            dir: format!("docs/{route}"),
            route: route.to_owned(),
            categories: vec!["design".to_owned()],
            tracking_key: "tracking-rfd".to_owned(),
            relevant_key: "relevant-rfd".to_owned(),
        }),
        ticket: ticket.map(|route| TicketSettings {
            dir: format!("docs/{route}"),
            route: route.to_owned(),
            done_limit: 8,
            labels: BTreeMap::new(),
        }),
    }
}

fn paths(owned: &[Owned]) -> Vec<&str> {
    owned.iter().map(|file| file.path.as_str()).collect()
}

fn contents<'a>(owned: &'a [Owned], path: &str) -> &'a str {
    let file = owned
        .iter()
        .find(|file| file.path == path)
        .unwrap_or_else(|| panic!("{path} isn't owned"));

    std::str::from_utf8(&file.contents).unwrap()
}

/// Anything else in `site/` would be built into the binary and never written.
#[test]
fn every_site_file_has_a_place_in_a_project() {
    for (path, _) in FILES {
        assert!(
            [
                "jp-vite/",
                "pages/rfd/",
                "pages/ticket/",
                "pages/wiki/",
                "pages/release/",
            ]
            .iter()
            .any(|prefix| path.starts_with(prefix)),
            "{path} has nowhere to go"
        );
    }
}

/// The snapshot is written by `sync` from the configuration, not built in.
#[test]
fn the_binary_carries_no_snapshot() {
    assert!(
        FILES
            .iter()
            .all(|(path, _)| *path != "jp-vite/settings.json")
    );
}

#[test]
fn the_owned_directory_is_written_as_it_is() {
    let owned = owned(&settings(None, None));

    for (path, bytes) in FILES {
        let Some(rest) = path.strip_prefix("jp-vite/") else {
            continue;
        };
        let file = owned
            .iter()
            .find(|file| file.path == Utf8Path::new("docs/.vitepress/jp-vite").join(rest))
            .unwrap_or_else(|| panic!("{path} isn't written"));
        assert_eq!(file.contents, *bytes, "{path}");
    }
}

/// Pages are the project's once written, so `jp vite` owns only its own
/// directory.
#[test]
fn only_the_owned_directory_is_owned() {
    let owned = owned(&settings(Some("rfd"), Some("ticket")));

    assert!(
        paths(&owned)
            .iter()
            .all(|path| path.starts_with("docs/.vitepress/jp-vite/")),
        "{:?}",
        paths(&owned)
    );
}

#[test]
fn a_project_without_workflows_gets_no_pages() {
    assert!(pages(&settings(None, None)).is_empty());
}

#[test]
fn each_workflow_gets_its_pages() {
    let pages = pages(&settings(Some("rfd"), Some("ticket")));

    assert_eq!(paths(&pages), [
        "docs/rfd/drafts/index.md",
        "docs/rfd/index.md",
        "docs/rfd/priority.md",
        "docs/ticket/board.md",
        "docs/ticket/index.md",
    ]);
}

/// The wiki gets its front page and its list of every page in its own
/// directory, where `jp wiki` reads neither as a page: a page's file is named
/// for its id.
#[test]
fn the_wiki_gets_its_front_page_and_list() {
    let settings = Settings {
        wiki: Some(WikiSettings {
            dir: "docs/knowledge".to_owned(),
            route: "knowledge".to_owned(),
            relevant_key: "relevant-rfd".to_owned(),
        }),
        ..settings(None, None)
    };

    let pages = pages(&settings);

    assert_eq!(paths(&pages), [
        "docs/knowledge/all.md",
        "docs/knowledge/index.md"
    ]);
    for (path, tag) in [
        ("docs/knowledge/all.md", "<WikiIndex />"),
        ("docs/knowledge/index.md", "<WikiChildren />"),
    ] {
        assert!(
            contents(&pages, path).contains(&format!("\n{tag}\n")),
            "{path} renders no {tag}"
        );
    }
}

/// The release pages go below the source directory, where the site serves the
/// releases, wherever the fragments are: the index, and the templates and paths
/// loaders VitePress renders each release's pages from.
#[test]
fn the_releases_get_their_index_and_page_templates() {
    let release = Some(ReleaseSettings {
        dir: "changelog.d".to_owned(),
        route: "releases".to_owned(),
        forge: None,
        headings: BTreeMap::new(),
    });
    let settings = Settings {
        release,
        ..settings(None, None)
    };

    let written = pages(&settings);

    // Paths order by component, so `[version]/` comes before `[version].md`.
    assert_eq!(paths(&written), [
        "docs/releases/[version]/upgrade.md",
        "docs/releases/[version]/upgrade.paths.mjs",
        "docs/releases/[version].md",
        "docs/releases/[version].paths.mjs",
        "docs/releases/index.md",
    ]);
    assert!(contents(&written, "docs/releases/index.md").contains("\n<ReleaseIndex />\n"));
    for template in [
        "docs/releases/[version].md",
        "docs/releases/[version]/upgrade.md",
    ] {
        assert!(
            contents(&written, template).contains("\n<!-- @content -->\n"),
            "{template} renders no content"
        );
    }

    let at_root = Settings {
        site: ".".to_owned(),
        ..settings
    };
    assert!(
        paths(&pages(&at_root)).contains(&"releases/index.md"),
        "a site at the workspace root serves the releases from `releases/`"
    );
}

/// Each paths loader defers to the owned module, at the path it has from where
/// the page sits.
#[test]
fn the_release_paths_loaders_defer_to_the_owned_module() {
    let site_file = |path: &str| {
        let (_, contents) = FILES
            .iter()
            .find(|(file, _)| *file == path)
            .unwrap_or_else(|| panic!("{path} isn't a site file"));

        String::from_utf8_lossy(contents).into_owned()
    };
    let owned = site_file("jp-vite/releases.mjs");

    for (path, export) in [
        (
            "pages/release/[version].paths.mjs",
            "export { notesRoutes as default } from '../.vitepress/jp-vite/releases.mjs'",
        ),
        (
            "pages/release/[version]/upgrade.paths.mjs",
            "export { upgradeRoutes as default } from '../../.vitepress/jp-vite/releases.mjs'",
        ),
    ] {
        assert!(
            site_file(path).contains(export),
            "{path} doesn't export its routes"
        );
    }
    for routes in ["export const notesRoutes", "export const upgradeRoutes"] {
        assert!(owned.contains(routes), "`releases.mjs` has no `{routes}`");
    }
}

/// A page renders its list with a tag the theme registers, so it reads the same
/// wherever the workflow's directory sits.
#[test]
fn a_page_renders_its_list_with_a_registered_tag() {
    let pages = pages(&settings(Some("design/rfd"), Some("ticket")));

    for (path, tag) in [
        ("docs/design/rfd/index.md", "<RfdIndex />"),
        ("docs/design/rfd/drafts/index.md", "<RfdDraftIndex />"),
        ("docs/design/rfd/priority.md", "<RfdBoard />"),
        ("docs/ticket/index.md", "<TicketIndex />"),
        ("docs/ticket/board.md", "<TicketBoard />"),
    ] {
        assert!(
            contents(&pages, path).contains(&format!("\n{tag}\n")),
            "{path} renders no {tag}"
        );
    }
}

/// Every tag a starting page uses is one the theme registers.
#[test]
fn the_theme_registers_every_tag_the_pages_use() {
    let theme = FILES
        .iter()
        .find(|(path, _)| *path == "jp-vite/theme/index.mjs")
        .map(|(_, contents)| String::from_utf8_lossy(contents).into_owned())
        .unwrap();

    for tag in [
        "RfdIndex",
        "RfdDraftIndex",
        "RfdBoard",
        "TicketIndex",
        "TicketBoard",
        "WikiIndex",
        "WikiChildren",
        "ReleaseIndex",
    ] {
        assert!(
            theme.contains(&format!("app.component('{tag}'")),
            "{tag} isn't registered"
        );
    }
}

#[test]
fn the_snapshot_is_owned_with_the_rest() {
    let settings = settings(Some("rfd"), None);
    let owned = owned(&settings);

    assert_eq!(
        contents(&owned, "docs/.vitepress/jp-vite/settings.json"),
        settings.render()
    );
}

#[test]
fn a_site_at_the_workspace_root_owns_its_own_vitepress_directory() {
    let settings = Settings {
        site: ".".to_owned(),
        ..settings(None, None)
    };

    assert_eq!(owned_dir(&settings), ".vitepress/jp-vite");
    assert_eq!(settings_path(&settings), ".vitepress/jp-vite/settings.json");
}
