//! Synchronizing published GitHub Release descriptions with the release notes
//! in `changelog.d/<version>/`.

use jp_github::{Octocrab, params::ReleaseUpdate};
use semver::Version;

use crate::{description::same, github::Repository, publish::version_of};

/// Update the description of every GitHub Release in `repository` whose current
/// body differs from `describe(version)`, returning the updated versions sorted
/// from lowest to highest.
///
/// Skips releases whose tag does not match `v<version>` or for which `describe`
/// returns `None`.
pub(crate) async fn sync(
    client: &Octocrab,
    repository: &Repository,
    describe: &mut dyn FnMut(&Version) -> Result<Option<String>, String>,
) -> Result<Vec<Version>, String> {
    let handler = client.releases(&repository.owner, &repository.name);
    let releases = handler
        .list()
        .await
        .map_err(|error| format!("The Releases of {} can't be read: {error}", repository.web))?;

    let mut updated = vec![];
    for release in releases {
        let Some(version) = version_of(&release.tag_name) else {
            continue;
        };
        let Some(body) = describe(&version)? else {
            continue;
        };
        if same(release.body.as_deref().unwrap_or_default(), &body) {
            continue;
        }

        handler
            .update(release.id, &ReleaseUpdate {
                body: Some(body),
                ..ReleaseUpdate::default()
            })
            .await
            .map_err(|error| {
                format!(
                    "The description of the Release of {} can't be updated: {error}",
                    release.tag_name
                )
            })?;
        updated.push(version);
    }

    updated.sort();
    Ok(updated)
}

pub(crate) fn summary(updated: &[Version]) -> String {
    let versions: Vec<String> = updated.iter().map(Version::to_string).collect();
    match versions.as_slice() {
        [] => "Every Release's description matches its notes.\n".to_owned(),
        [one] => format!("Updated the description of {one} to match its notes.\n"),
        [rest @ .., last] => format!(
            "Updated the descriptions of {} and {last} to match their notes.\n",
            rest.join(", ")
        ),
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
