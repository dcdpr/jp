use std::borrow::Cow;

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
    let page = jp_github::instance()
        .search()
        .code(&format!("{query} repo:{repository}"))
        .send()
        .await?;

    let matches = jp_github::instance()
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

/// Hard cap on lines returned without an explicit range.
///
/// Files larger than this require `start_line` and `end_line`. Picked to
/// match the kind of "big enough to bloat context" that motivated the
/// limit, while still leaving room for typical source files.
const MAX_LINES_WITHOUT_RANGE: usize = 2000;

pub(crate) async fn github_read_file(
    repository: Option<String>,
    ref_: Option<String>,
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
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

    // Silently treat 0 as 1 — both bounds are 1-based, but rather than
    // bouncing the LLM with an error and forcing a re-call, we just round
    // up to the nearest valid value.
    let start_line = start_line.map(|v| v.max(1));
    let end_line = end_line.map(|v| v.max(1));

    if let (Some(s), Some(e)) = (start_line, end_line)
        && s > e
    {
        return Err("`start_line` must be less than or equal to `end_line`".into());
    }

    let repository = repository.unwrap_or_else(|| format!("{ORG}/{REPO}"));
    let (org, repo) = repository
        .split_once('/')
        .ok_or("`repository` must be in the form of <org>/<repo>")?;

    let client = jp_github::instance();
    let files = client.repos(org, repo);
    let mut files = files.get_content().path(path);

    if let Some(ref_) = ref_ {
        files = files.r#ref(ref_);
    }

    let files = files
        .send()
        .await
        .map_err(|err| match err {
            jp_github::Error::GitHub { source, .. } if source.status_code == 404 => {
                "file does not exist for the provided repository".to_owned()
            }
            _ => format!("failed to fetch file: {err:?}"),
        })?
        .take_items();

    let mut out = Vec::with_capacity(files.len());
    for item in files {
        let kind = item.r#type.clone();
        let content = item
            .content
            .as_deref()
            .map(|c| decode_content(c, item.encoding.as_deref()))
            .map(Cow::into_owned);

        // Apply line range only to file blobs (not directory listings).
        let content = if kind == "file" {
            content
                .map(|text| apply_range(&item.path, &text, start_line, end_line))
                .transpose()?
        } else {
            content
        };

        out.push(File {
            path: item.path,
            kind,
            content,
        });
    }

    to_xml(Files { files: out })
}

fn decode_content<'a>(content: &'a str, encoding: Option<&str>) -> Cow<'a, str> {
    match encoding {
        Some("base64") => Cow::Owned(
            BASE64_STANDARD
                .decode(
                    content
                        .chars()
                        .filter(|c| !c.is_whitespace())
                        .collect::<String>(),
                )
                .map_err(|e| e.to_string())
                .and_then(|v| String::from_utf8(v).map_err(|e| e.to_string()))
                .unwrap_or_else(|e| format!("Error decoding base64: {e}")),
        ),
        _ => Cow::Borrowed(content),
    }
}

fn apply_range(
    path: &str,
    content: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String> {
    let total = content.split('\n').count();

    if let Some(s) = start_line
        && s > total
    {
        return Err(format!(
            "`start_line` ({s}) is greater than the number of lines in `{path}` ({total})"
        )
        .into());
    }

    // Cap the implicit upper bound when the caller didn't provide an
    // explicit `end_line` AND the file is large enough for the cap to
    // matter. Without this guard, a call like `start_line=1` on a 50k-line
    // file would return the entire file — silently bypassing the
    // protection that exists to keep large files out of LLM context.
    let cap_applied = end_line.is_none() && total > MAX_LINES_WITHOUT_RANGE;
    let cap_start = start_line.unwrap_or(1);
    let cap_end = (cap_start + MAX_LINES_WITHOUT_RANGE - 1).min(total);
    let effective_end = if cap_applied { Some(cap_end) } else { end_line };

    let lines: Vec<&str> = content.split('\n').collect();
    let from = start_line.unwrap_or(1).saturating_sub(1);
    let to = effective_end.unwrap_or(total).min(total);

    let mut out = String::new();
    if let Some(s) = start_line {
        out.push_str(&format!("... (starting from line #{s}) ...\n"));
    }
    out.push_str(&lines[from..to].join("\n"));
    if cap_applied {
        out.push_str(&format!(
            "\n... (truncated to lines {cap_start}-{cap_end} of {total}; pass `start_line` and \
             `end_line` to read a different range) ..."
        ));
    } else if let Some(e) = effective_end
        && e < total
    {
        out.push_str(&format!("\n... (truncated after line #{e}) ..."));
    }

    Ok(out)
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

        let result: Value = jp_github::instance()
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
