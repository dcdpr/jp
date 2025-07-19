use indoc::formatdoc;
use url::Url;

use super::auth;
use crate::{
    github::{ORG, REPO},
    to_xml, Result,
};

pub(crate) async fn github_create_issue_bug(
    title: String,
    description: String,
    expected_behavior: String,
    actual_behavior: String,
    complexity: String,
    reproduce: Option<String>,
    proposed_solution: Option<String>,
    tasks: Option<Vec<String>>,
    resource_links: Option<Vec<String>>,
    labels: Option<Vec<String>>,
    assignees: Option<Vec<String>>,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Issue {
        url: Url,
    }

    auth().await?;

    if assignees.as_ref().is_some_and(|v| !v.is_empty()) {
        check_assignees(assignees.as_ref()).await?;
    }

    if labels.as_ref().is_some_and(|v| !v.is_empty()) {
        check_labels(labels.as_ref()).await?;
    }

    let mut body = formatdoc!(
        "{description}

        ## Expected Behavior

        {expected_behavior}

        ## Actual Behavior

        {actual_behavior}"
    );

    if let Some(reproduce) = reproduce {
        body.push_str("\n\n## Reproduce\n\n");
        body.push_str(&reproduce);
    }

    if let Some(proposed_solution) = proposed_solution {
        body.push_str("\n\n## Proposed Solution\n\n");
        body.push_str(&proposed_solution);
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

    let mut labels = labels.unwrap_or_default();
    labels.push("bug".to_owned());

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
        .assignees(assignees)
        .send()
        .await?;

    to_xml(Issue {
        url: issue.html_url,
    })
}

async fn check_labels(as_ref: Option<&Vec<String>>) -> Result<()> {
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

async fn check_assignees(assignees: Option<&Vec<String>>) -> Result<()> {
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
