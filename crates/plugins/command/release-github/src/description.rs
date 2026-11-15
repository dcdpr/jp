//! Formatting and comparison for GitHub Release descriptions.

use changelog::{Headings, Release, markdown};
use semver::Version;

use crate::options::{DOWNLOAD, VERSION};

const UPGRADE_GUIDE: &str = "Upgrade guide";

/// Render the GitHub Release description for `version`.
///
/// When `upgrade_guide` is `Some`, breaking entries link to that URL (with
/// `{version}` substituted).
/// When `None`, the upgrade guide is appended inline under `# Upgrade guide`.
/// If `footer` is present, it is appended at the end with `{version}` and
/// `{download}` substituted.
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

/// Compare two descriptions after normalizing `\r\n` line endings and trailing
/// whitespace, which GitHub may alter when a release is edited in the web UI.
pub(crate) fn same(published: &str, rendered: &str) -> bool {
    let normalize = |text: &str| text.replace("\r\n", "\n").trim_end().to_owned();

    normalize(published) == normalize(rendered)
}

#[cfg(test)]
#[path = "description_tests.rs"]
mod tests;
