use indoc::formatdoc;
use url::Url;

use super::auth;
use crate::{
    github::{ORG, REPO},
    to_xml,
    util::OneOrMany,
    Result,
};

pub(crate) async fn github_create_issue_enhancement(
    title: String,
    description: String,
    context: String,
    complexity: String,
    alternatives: Option<String>,
    proposed_implementation: Option<String>,
    tasks: Option<OneOrMany<String>>,
    resource_links: Option<OneOrMany<String>>,
    labels: Option<OneOrMany<String>>,
    assignees: Option<OneOrMany<String>>,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Issue {
        url: Url,
    }

    auth().await?;

    if assignees.as_ref().is_some_and(|v| !v.is_empty()) {
        check_assignees(assignees.as_deref()).await?;
    }

    if labels.as_ref().is_some_and(|v| !v.is_empty()) {
        check_labels(labels.as_deref()).await?;
    }

    let mut body = formatdoc!(
        "{description}

        ## Context

        {context}"
    );

    if let Some(v) = alternatives {
        body.push_str("\n\n## Alternatives\n\n");
        body.push_str(&v);
    }

    if let Some(v) = proposed_implementation {
        body.push_str("\n\n## Proposed Implementation\n\n");
        body.push_str(&v);
    }

    if let Some(tasks) = tasks
        && !tasks.is_empty()
    {
        body.push_str("\n\n## Tasks\n- [ ] ");
        body.push_str(&tasks.join("\n- [ ] "));
    }

    if let Some(resource_links) = resource_links
        && !resource_links.is_empty()
    {
        let resource_links = resource_links
            .into_iter()
            .map(|link| {
                if link.starts_with("http") {
                    link
                } else {
                    format!("- https://github.com/{ORG}/{REPO}/{link}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        body.push_str("\n\n## Resources\n\n");
        body.push_str(&resource_links);
    }

    let mut labels = labels.unwrap_or_default().into_vec();
    labels.push("enhancement".to_owned());

    match complexity.as_str() {
        "low" => labels.push("good first issue".to_owned()),
        "medium" | "high" => {}
        _ => return Err("Invalid complexity, must be one of `low`, `medium`, or `high`.".into()),
    }

    let issue = octocrab::instance()
        .issues(ORG, REPO)
        .create(&title)
        .body(&body)
        .labels(Some(labels))
        .assignees(assignees.map(Into::into))
        .send()
        .await?;

    to_xml(Issue {
        url: issue.html_url,
    })
}

async fn check_labels(as_ref: Option<&[String]>) -> Result<()> {
    let page = octocrab::instance()
        .issues(ORG, REPO)
        .list_labels_for_repo()
        .send()
        .await?;

    let labels = octocrab::instance().all_pages(page).await?;

    let mut invalid_labels = vec![];
    for label in as_ref.into_iter().flatten() {
        if labels.iter().any(|l| &l.name == label) {
            continue;
        }

        invalid_labels.push(label);
    }

    if !invalid_labels.is_empty() {
        return Err(formatdoc!(
            "The following labels do not exist on the project, and cannot be assigned to the \
             issue:

             {}

             Valid labels are:

             {}",
            invalid_labels
                .iter()
                .map(|l| format!("- {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
            labels
                .iter()
                .map(|l| format!(
                    "- {}{}",
                    l.name,
                    l.description
                        .as_ref()
                        .map(|d| format!(" ({d})"))
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into());
    }

    Ok(())
}

async fn check_assignees(assignees: Option<&[String]>) -> Result<()> {
    let page = octocrab::instance()
        .repos(ORG, REPO)
        .list_collaborators()
        .send()
        .await?;

    let collaborators = octocrab::instance().all_pages(page).await?;

    let mut invalid_assignees = vec![];
    for assignee in assignees.into_iter().flatten() {
        if collaborators.iter().any(|c| &c.author.login == assignee) {
            continue;
        }

        invalid_assignees.push(assignee);
    }

    if !invalid_assignees.is_empty() {
        return Err(formatdoc!(
            "The following assignees are not collaborators on the project, and cannot be assigned \
             to the issue:

             {}

             Valid assignees are:

             {}",
            invalid_assignees
                .iter()
                .map(|a| format!("- {a}"))
                .collect::<Vec<_>>()
                .join("\n"),
            collaborators
                .iter()
                .map(|c| format!("- {}", c.author.login))
                .collect::<Vec<_>>()
                .join("\n")
        )
        .into());
    }

    Ok(())
}
