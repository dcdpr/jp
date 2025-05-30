use base64::{prelude::BASE64_STANDARD, Engine as _};

use super::auth;
use crate::{
    github::{ORG, REPO},
    to_xml, Result,
};

pub(crate) async fn github_code_search(
    repository: Option<String>,
    query: String,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct CodeMatches {
        matches: Vec<CodeMatch>,
    }

    #[derive(serde::Serialize)]
    struct CodeMatch {
        path: String,
        sha: String,
        repository: String,
    }

    auth().await?;

    let repository = repository.unwrap_or_else(|| format!("{ORG}/{REPO}"));
    let page = octocrab::instance()
        .search()
        .code(&format!("{query} repo:{repository}"))
        .send()
        .await?;

    let matches = octocrab::instance()
        .all_pages(page)
        .await?
        .into_iter()
        .map(|code| CodeMatch {
            path: code.path,
            sha: code.sha,
            repository: repository.clone(),
        })
        .collect();

    to_xml(CodeMatches { matches })
}

pub(crate) async fn github_read_file(repository: Option<String>, path: String) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Files {
        files: Vec<File>,
    }

    #[derive(serde::Serialize)]
    struct File {
        path: String,
        #[serde(rename = "type")]
        kind: String,
        content: Option<String>,
    }

    auth().await?;

    let repository = repository.unwrap_or_else(|| format!("{ORG}/{REPO}"));
    let (org, repo) = repository
        .split_once('/')
        .ok_or("`repository` must be in the form of <org>/<repo>")?;

    let files = octocrab::instance()
        .repos(org, repo)
        .get_content()
        .path(path)
        .send()
        .await
        .map_err(|err| match err {
            octocrab::Error::GitHub { source, .. } if source.status_code == 404 => {
                "file does not exist for the provided repository".to_owned()
            }
            _ => format!("failed to fetch file: {err:?}"),
        })?
        .take_items()
        .into_iter()
        .map(|item| File {
            path: item.path,
            kind: item.r#type.to_string(),
            content: item.content.map(|content| match item.encoding.as_deref() {
                Some("base64") => BASE64_STANDARD
                    .decode(
                        content
                            .chars()
                            .filter(|c| !c.is_whitespace())
                            .collect::<String>(),
                    )
                    .map_err(|e| e.to_string())
                    .and_then(|v| String::from_utf8(v).map_err(|e| e.to_string()))
                    .unwrap_or_else(|e| format!("Error decoding base64: {e}")),
                _ => content,
            }),
        })
        .collect();

    to_xml(Files { files })
}
