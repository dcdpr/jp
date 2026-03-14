use url::Url;

use super::auth;
use crate::{
    Result,
    github::{ORG, REPO},
    to_xml,
    util::OneOrMany,
};

/// Create a tracking issue for an RFD.
///
/// The title, label, and body structure are fixed. Only the RFD number,
/// title text, filename slug, and task list are caller-provided.
pub(crate) async fn github_create_issue_rfd_tracking(
    rfd_number: String,
    rfd_title: String,
    rfd_slug: String,
    tasks: OneOrMany<String>,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Issue {
        number: u64,
        url: Url,
    }

    auth().await?;

    let issue_title = format!("RFD-{rfd_number}: {rfd_title}");

    let task_lines: String = tasks
        .iter()
        .map(|t| format!("- [ ] {t}"))
        .collect::<Vec<_>>()
        .join("\n");

    let body = format!(
        "Tracking issue for [RFD-{rfd_number}: {rfd_title}](https://jp.computer/rfd/{rfd_slug}).\n\
         \n\
         This issue tracks the progress towards the complete implementation of RFD-{rfd_number}.\n\
         \n\
         ## Tasks\n\
         \n\
         {task_lines}"
    );

    let issue = jp_github::instance()
        .issues(ORG, REPO)
        .create(&issue_title)
        .body(&body)
        .labels(Some(vec!["rfd".to_owned()]))
        .send()
        .await?;

    to_xml(Issue {
        number: issue.number,
        url: issue.html_url,
    })
}
