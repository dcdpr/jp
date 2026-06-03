use super::*;

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
