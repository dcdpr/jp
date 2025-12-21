use base64::{Engine as _, prelude::BASE64_STANDARD};
use serde_json::{Value, json};

use super::auth;
use crate::{
    Result,
    github::{ORG, REPO},
    to_xml,
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

pub(crate) async fn github_read_file(
    repository: Option<String>,
    ref_: Option<String>,
    path: String,
) -> Result<String> {
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

    let client = octocrab::instance();
    let files = client.repos(org, repo);
    let mut files = files.get_content().path(path);

    if let Some(ref_) = ref_ {
        files = files.r#ref(ref_);
    }

    let files = files
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
            kind: item.r#type.clone(),
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

pub(crate) async fn github_list_files(
    repository: Option<String>,
    ref_: Option<String>,
    path: Option<String>,
) -> Result<String> {
    #[derive(serde::Serialize)]
    struct Files {
        files: Vec<File>,
    }

    #[derive(serde::Serialize)]
    struct File {
        path: String,
        size: Option<u64>,
    }

    async fn fetch(
        org: &str,
        repo: &str,
        ref_: &str,
        prefix: &str,
        files: &mut Vec<File>,
    ) -> Result<()> {
        let query = indoc::indoc! {"
            repository(owner: $owner, name: $name) {
                object(expression: $expr) {
                    ... on Tree {
                        entries {
                            name
                            type
                            object {
                                ... on Blob {
                                    byteSize
                                    isBinary
                                }
                            }
                        }
                    }
                }
            }
        "};

        let result: Value = octocrab::instance()
            .graphql(&json!({
                "query": query,
                "variables": {
                    "owner": org,
                    "name": repo,
                    "expr": format!("{ref_}:{prefix}"),
                }
            }))
            .await?;

        let iter = result
            .pointer("/data/repository/object/entries")
            .and_then(|v: &Value| v.as_array())
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_object());

        for data in iter {
            // Skip binary files
            if data.get("object").is_some_and(|v| {
                v.get("isBinary")
                    .and_then(Value::as_bool)
                    .is_some_and(|is_binary| is_binary)
            }) {
                continue;
            }

            let Some(name) = data.get("name").and_then(|v| v.as_str()) else {
                continue;
            };

            let Some(kind) = data.get("type").and_then(|v| v.as_str()) else {
                continue;
            };

            let size = data
                .get("object")
                .and_then(|v| v.as_object())
                .and_then(|v| v.get("byteSize"))
                .and_then(Value::as_u64);

            let path = if prefix.is_empty() {
                name.to_owned()
            } else {
                format!("{prefix}/{name}")
            };

            match kind {
                "tree" => Box::pin(fetch(org, repo, ref_, &path, files)).await?,
                "blob" => files.push(File { path, size }),
                _ => {}
            }
        }

        Ok(())
    }

    auth().await?;

    let repository = repository.unwrap_or_else(|| format!("{ORG}/{REPO}"));
    let (org, repo) = repository
        .split_once('/')
        .ok_or("`repository` must be in the form of <org>/<repo>")?;

    let prefix = path.unwrap_or_default();
    let ref_ = ref_.unwrap_or_else(|| "HEAD".to_owned());

    let mut files = vec![];
    fetch(org, repo, &ref_, &prefix, &mut files).await?;

    to_xml(Files { files })
}
