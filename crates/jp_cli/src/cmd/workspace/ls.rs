use chrono::{DateTime, Local, Utc};
use comfy_table::Row;
use jp_printer::Printer;
use jp_workspace::roots;

use crate::{
    DEFAULT_STORAGE_DIR,
    cmd::{Output, workspace::target::TargetEnv},
    output::print_table,
};

/// List known workspaces and their checkouts.
///
/// Reads the user-global registries only — no workspace is bootstrapped, so
/// the listing works from anywhere (RFD 087).
/// One row per live checkout; the session-active checkout is marked with `*`.
#[derive(Debug, clap::Args)]
pub(crate) struct Ls {}

impl Ls {
    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(crate) fn run(self, printer: &Printer, env: &TargetEnv<'_>) -> Output {
        let known = roots::known_workspaces(&env.workspaces_dir, DEFAULT_STORAGE_DIR);
        if known.is_empty() {
            printer.println(
                "No known workspaces. JP registers a workspace when a command runs from inside it."
                    .to_owned(),
            );
            return Ok(());
        }

        let active = env.session.and_then(|session| env.store.active(session));
        let is_active = |id: &jp_workspace::Id, root: &camino::Utf8Path| {
            active.as_ref().is_some_and(|entry| {
                entry.id().is_some_and(|active_id| active_id == *id) && entry.root == root
            })
        };

        let header = Row::from(vec!["", "ID", "Name", "Checkout", "Last used"]);
        let mut rows = Vec::new();

        for workspace in known {
            let name = workspace.slug.as_deref().unwrap_or("").to_owned();

            if workspace.roots.is_empty() {
                rows.push(Row::from(vec![
                    String::new(),
                    workspace.id.to_string(),
                    name,
                    "(no live checkouts)".to_owned(),
                    String::new(),
                ]));
                continue;
            }

            for (index, entry) in workspace.roots.into_iter().enumerate() {
                // Repeat the ID and name only on the workspace's first row;
                // further checkouts read as a continuation.
                let (id, name) = if index == 0 {
                    (workspace.id.to_string(), name.clone())
                } else {
                    (String::new(), String::new())
                };
                let marker = if is_active(&workspace.id, &entry.path) {
                    "*".to_owned()
                } else {
                    String::new()
                };

                rows.push(Row::from(vec![
                    marker,
                    id,
                    name,
                    entry.path.to_string(),
                    humanize(entry.last_used),
                ]));
            }
        }

        print_table(printer, header, rows, false);
        Ok(())
    }
}

/// A stable local-time rendering for the listing.
fn humanize(at: DateTime<Utc>) -> String {
    at.with_timezone(&Local)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

#[cfg(test)]
#[path = "ls_tests.rs"]
mod tests;
