//! A Release's description: the release's notes, then its upgrade guide unless
//! the notes link to one elsewhere, then the workspace's footer.

use changelog::{Headings, Release, markdown};
use semver::Version;

use crate::options::{DOWNLOAD, VERSION};

/// The heading the upgrade guide follows the notes under, when the description
/// includes it.
const UPGRADE_GUIDE: &str = "Upgrade guide";

/// The description of the Release of `version`, whose files GitHub serves from
/// `download`.
///
/// Breaking entries link to the upgrade guide at `upgrade_guide`, an address
/// with `{version}` in it.
/// Without one, the guide follows the notes under a heading of its own, where
/// the entries' links can't reach it: GitHub gives a description's headings no
/// anchors.
/// `footer` ends the description, with `{version}` and `{download}` in it
/// replaced.
pub(crate) fn description(
    release: &Release,
    headings: &Headings,
    version: &Version,
    upgrade_guide: Option<&str>,
    footer: Option<&str>,
    download: &str,
) -> String {
    let version = version.to_string();
    let address = upgrade_guide.map(|address| address.replace(VERSION, &version));

    let mut text = markdown::notes(release, headings, address.as_deref().unwrap_or(""));
    if address.is_none() {
        let guide = markdown::upgrade_guide(release);
        if !guide.is_empty() {
            text = format!("{text}\n# {UPGRADE_GUIDE}\n\n{guide}");
        }
    }

    let footer = footer
        .map(|footer| {
            footer
                .replace(VERSION, &version)
                .replace(DOWNLOAD, download)
        })
        .unwrap_or_default();
    let footer = footer.trim();
    if !footer.is_empty() {
        text = format!("{text}\n{footer}\n");
    }

    text
}

/// Whether two descriptions read the same: GitHub hands back a description
/// edited on its site with `\r\n` line endings, and may drop trailing
/// whitespace.
pub(crate) fn same(published: &str, rendered: &str) -> bool {
    let normalize = |text: &str| text.replace("\r\n", "\n").trim_end().to_owned();

    normalize(published) == normalize(rendered)
}

#[cfg(test)]
#[path = "description_tests.rs"]
mod tests;
