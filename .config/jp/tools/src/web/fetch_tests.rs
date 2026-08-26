use assert_matches::assert_matches;
use jp_tool::Outcome;
use serde_json::json;

use super::*;

mod headless_gate {
    use super::*;

    fn options(value: &Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    async fn fetch_headless(options: &Map<String, Value>) -> Outcome {
        let url = Url::parse("https://example.com").unwrap();

        web_fetch(url, false, None, true, options)
            .await
            .expect("gate answers without failing the tool")
    }

    #[tokio::test]
    async fn refused_when_the_option_is_absent() {
        // No browser is located and no request is made: the refusal happens
        // before either, which is what keeps this test hermetic.
        assert_matches!(fetch_headless(&options(&json!({}))).await, Outcome::Error { message, .. } => {
            assert_eq!(message, HEADLESS_DISABLED);
        });
    }

    #[tokio::test]
    async fn refused_when_the_option_is_explicitly_false() {
        let options = options(&json!({ "allow_headless": false }));

        assert_matches!(fetch_headless(&options).await, Outcome::Error { message, .. } => {
            assert_eq!(message, HEADLESS_DISABLED);
        });
    }

    #[tokio::test]
    async fn unparseable_options_refuse_rather_than_open_up() {
        // Malformed options fall back to the defaults, and the default is off.
        let options = options(&json!({ "strategy": "nonsense" }));

        assert_matches!(fetch_headless(&options).await, Outcome::Error { message, .. } => {
            assert_eq!(message, HEADLESS_DISABLED);
        });
    }

    #[tokio::test]
    async fn a_plain_fetch_is_unaffected_by_the_option() {
        // The gate only guards the `headless` argument; leaving it off must not
        // change how an ordinary fetch is dispatched.
        let url = Url::parse("https://github.com/foo/bar/issues/42").unwrap();
        let outcome = web_fetch(url, false, None, false, &options(&json!({})))
            .await
            .unwrap();

        assert_matches!(outcome, Outcome::Error { message, .. } => {
            assert!(message.contains("github_issues"));
        });
    }
}

mod is_binary {
    use super::*;

    #[test]
    fn image_types() {
        assert!(is_binary("image/png"));
        assert!(is_binary("image/jpeg"));
        assert!(is_binary("Image/PNG"));
    }

    #[test]
    fn audio_video() {
        assert!(is_binary("audio/mpeg"));
        assert!(is_binary("video/mp4"));
    }

    #[test]
    fn application_types() {
        assert!(is_binary("application/octet-stream"));
        assert!(is_binary("application/pdf"));
        assert!(is_binary("application/zip"));
    }

    #[test]
    fn text_types_are_not_binary() {
        assert!(!is_binary("text/html; charset=utf-8"));
        assert!(!is_binary("text/plain"));
        assert!(!is_binary("application/json"));
        assert!(!is_binary("application/xml"));
    }
}

mod github_issue_or_pr_redirect {
    use super::*;

    fn redirect(url: &str) -> Option<String> {
        github_issue_or_pr_redirect(&Url::parse(url).unwrap())
    }

    #[test]
    fn issue_url_suggests_github_issues() {
        let msg = redirect("https://github.com/Swatinem/rust-cache/issues/37").unwrap();
        assert!(msg.contains("`github_issues`"));
        assert!(msg.contains(r#""repository": "Swatinem/rust-cache""#));
        assert!(msg.contains(r#""number": 37"#));
    }

    #[test]
    fn pull_url_suggests_github_pulls() {
        // Bare `/pull/N` (conversation tab) routes to the metadata+comments
        // tool, not the diff tool.
        let msg = redirect("https://github.com/rust-lang/rust/pull/12345").unwrap();
        assert!(msg.contains("`github_pulls`"));
        assert!(!msg.contains("`github_pr_diff`"));
        assert!(msg.contains(r#""repository": "rust-lang/rust""#));
        assert!(msg.contains(r#""number": 12345"#));
    }

    #[test]
    fn pull_files_url_suggests_github_pr_diff() {
        // `/pull/N/files` is the files-changed tab — the common paste
        // target for code review URLs — and routes to the dedicated diff
        // tool.
        let msg = redirect("https://github.com/rust-lang/rust/pull/12345/files").unwrap();
        assert!(msg.contains("`github_pr_diff`"));
        assert!(!msg.contains("`github_pulls`"));
        assert!(msg.contains(r#""repository": "rust-lang/rust""#));
        assert!(msg.contains(r#""number": 12345"#));
    }

    #[test]
    fn pull_commits_url_suggests_github_pr_commits() {
        // `/pull/N/commits` is the commits tab and routes to the dedicated
        // commit-list tool.
        let msg = redirect("https://github.com/rust-lang/rust/pull/12345/commits").unwrap();
        assert!(msg.contains("`github_pr_commits`"));
        assert!(!msg.contains("`github_pulls`"));
        assert!(msg.contains(r#""repository": "rust-lang/rust""#));
        assert!(msg.contains(r#""number": 12345"#));
    }

    #[test]
    fn pull_other_subpaths_fall_back_to_github_pulls() {
        // `/checks`, `/conflicts` etc. don't have dedicated tools — the
        // metadata+conversation answer is the closest fit, so the redirect
        // keeps them on `github_pulls`.
        let msg = redirect("https://github.com/foo/bar/pull/42/checks").unwrap();
        assert!(msg.contains("`github_pulls`"));
        assert!(!msg.contains("`github_pr_diff`"));
    }

    #[test]
    fn host_match_is_case_insensitive() {
        assert!(redirect("https://GitHub.com/o/r/issues/1").is_some());
    }

    #[test]
    fn non_github_url_passes_through() {
        assert!(redirect("https://docs.rs/tokio/latest/tokio/").is_none());
    }

    #[test]
    fn github_blob_url_passes_through() {
        // Blob/tree/release pages render server-side and work fine via
        // the HTML pipeline.
        assert!(redirect("https://github.com/foo/bar/blob/main/README.md").is_none());
    }

    #[test]
    fn github_repo_root_passes_through() {
        assert!(redirect("https://github.com/foo/bar").is_none());
    }

    #[test]
    fn non_numeric_issue_id_passes_through() {
        assert!(redirect("https://github.com/foo/bar/issues/new").is_none());
    }

    #[test]
    fn trailing_slash_is_tolerated() {
        assert!(redirect("https://github.com/foo/bar/issues/42/").is_some());
    }
}

mod truncate {
    use super::*;

    #[test]
    fn under_limit_unchanged() {
        let s = "short content";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn over_limit_truncates_with_note() {
        let s = "a".repeat(50);
        let result = truncate(&s, 20);
        assert!(result.starts_with("aaaaaaaaaaaaaaaaaaaa"));
        assert!(result.contains("[Content truncated:"));
        assert!(result.contains("20 of 50 bytes"));
    }

    #[test]
    fn exact_limit() {
        let s = "exactly10!";
        assert_eq!(truncate(s, 10), s);
    }
}
