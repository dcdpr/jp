use assert_matches::assert_matches;
use jp_tool::Outcome;

use super::*;

mod fetch {
    use super::*;

    #[tokio::test]
    async fn a_non_http_url_never_reaches_a_browser() {
        // A browser prints the contents of `file:` URLs just as readily as web
        // pages, so the scheme is checked before any binary is located.
        let url = Url::parse("file:///etc/passwd").unwrap();

        assert_matches!(fetch(&url, false, None).await.unwrap(), Outcome::Error { message, .. } => {
            assert_eq!(
                message,
                "web_fetch reads `http` and `https` pages; `file` URLs are not supported"
            );
        });
    }

    #[tokio::test]
    async fn a_data_url_is_refused_by_the_same_check() {
        let url = Url::parse("data:text/html,<h1>hi</h1>").unwrap();

        assert_matches!(fetch(&url, false, None).await.unwrap(), Outcome::Error { message, .. } => {
            assert_eq!(
                message,
                "web_fetch reads `http` and `https` pages; `data` URLs are not supported"
            );
        });
    }
}

mod browser_args {
    use super::*;

    #[test]
    fn with_a_profile_directory() {
        let url = Url::parse("https://claude.ai/share/3d88614f").unwrap();
        let args = browser_args(&url, Some(Path::new("/tmp/jp-headless-1-0")));

        assert_eq!(args, vec![
            "--headless",
            "--disable-gpu",
            "--dump-dom",
            "--user-agent=Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36",
            "--virtual-time-budget=15000",
            "--timeout=20000",
            "--user-data-dir=/tmp/jp-headless-1-0",
            "--no-first-run",
            "--no-default-browser-check",
            "https://claude.ai/share/3d88614f",
        ]);
    }

    #[test]
    fn without_a_profile_directory() {
        let url = Url::parse("https://claude.ai/share/3d88614f").unwrap();
        let args = browser_args(&url, None);

        assert!(!args.iter().any(|arg| arg.starts_with("--user-data-dir")));
        assert!(!args.iter().any(|arg| arg == "--no-first-run"));
    }

    #[test]
    fn user_agent_never_advertises_headless() {
        // The `HeadlessChrome` token in Chrome's own headless User-Agent is
        // enough for bot protection to serve a challenge page instead of the
        // content, so overriding it is what makes gated pages readable.
        let url = Url::parse("https://example.com").unwrap();
        let args = browser_args(&url, None);

        let ua = args
            .iter()
            .find(|arg| arg.starts_with("--user-agent="))
            .expect("user agent override is always passed");

        assert!(!ua.contains("Headless"));
        assert!(ua.contains("Chrome/"));
    }

    #[test]
    fn url_is_last_so_it_is_never_read_as_a_flag_value() {
        let url = Url::parse("https://example.com/a?b=c#d").unwrap();

        for profile in [Some(Path::new("/tmp/p")), None] {
            let args = browser_args(&url, profile);
            assert_eq!(args.last().unwrap(), "https://example.com/a?b=c#d");
        }
    }
}

mod capture_deadline {
    use super::*;

    #[test]
    fn browser_gives_up_before_the_outer_backstop() {
        // If the browser's own deadline were the looser of the two, every hung
        // render would be killed by the backstop and return nothing at all,
        // instead of dumping the DOM it had.
        assert!(u128::from(CAPTURE_TIMEOUT_MS) < RENDER_TIMEOUT.as_millis());
    }
}

mod temp_profile_dir {
    use super::*;

    #[test]
    fn successive_calls_do_not_collide() {
        // Concurrent renders in one process each need their own profile, or
        // the second browser fails on the first one's lock file.
        assert_ne!(temp_profile_dir(), temp_profile_dir());
    }
}

mod render_error {
    use super::*;

    #[test]
    fn silent_failure_names_the_binary_and_the_way_out() {
        let msg = render_error(
            "timed out after 45s",
            Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            "",
        )
        .to_string();

        assert_eq!(
            msg,
            "headless render timed out after 45s using /Applications/Google \
             Chrome.app/Contents/MacOS/Google Chrome. For a binary that renders reliably, run \
             `npx @puppeteer/browsers install chrome-headless-shell@stable --path <dir>`, then \
             symlink it onto PATH or name it in JP_HEADLESS_BROWSER. It loads `icudtl.dat` from \
             beside the executable, so symlink the unpacked binary rather than copying it out of \
             its directory."
        );
    }

    #[test]
    fn failure_reports_the_browser_output() {
        let msg = render_error(
            "produced an empty document",
            Path::new("/opt/bin/chrome-headless-shell"),
            "  [0821/..] Fatal error\n",
        )
        .to_string();

        assert_eq!(
            msg,
            "headless render produced an empty document using /opt/bin/chrome-headless-shell. \
             Browser output: [0821/..] Fatal error. For a binary that renders reliably, run `npx \
             @puppeteer/browsers install chrome-headless-shell@stable --path <dir>`, then symlink \
             it onto PATH or name it in JP_HEADLESS_BROWSER. It loads `icudtl.dat` from beside \
             the executable, so symlink the unpacked binary rather than copying it out of its \
             directory."
        );
    }
}

mod is_app_bundle {
    use super::*;

    #[test]
    fn matches_an_executable_inside_a_bundle() {
        assert!(is_app_bundle(Path::new(
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"
        )));
    }

    #[test]
    fn does_not_match_a_command_line_binary() {
        assert!(!is_app_bundle(Path::new(
            "/opt/homebrew/bin/chrome-headless-shell"
        )));
        assert!(!is_app_bundle(Path::new(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        )));
    }
}

mod resolve_binary {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn follows_a_symlink_to_the_install_directory() {
        // A `~/.local/bin` symlink is the normal way this binary ends up on
        // PATH, and launching through it leaves the browser unable to find
        // `icudtl.dat` next to the executable.
        let dir = camino_tempfile::tempdir().unwrap();
        let install = dir.path().join("chrome-headless-shell-mac-arm64");
        fs::create_dir_all(&install).unwrap();

        let binary = install.join("chrome-headless-shell");
        fs::write(&binary, "").unwrap();

        let link = dir.path().join("chrome-headless-shell");
        std::os::unix::fs::symlink(&binary, &link).unwrap();

        assert_eq!(
            resolve_binary(link.as_std_path().to_path_buf()),
            fs::canonicalize(&binary).unwrap()
        );
    }

    #[test]
    fn leaves_an_unresolvable_path_alone() {
        let path = PathBuf::from("/nonexistent/chrome-headless-shell");
        assert_eq!(resolve_binary(path.clone()), path);
    }
}

mod no_browser_message {
    use super::*;

    #[test]
    fn names_both_supported_ways_to_supply_a_binary() {
        let msg = no_browser_message();

        assert!(msg.contains("PATH"));
        assert!(msg.contains("JP_HEADLESS_BROWSER"));
        assert!(msg.contains("google-chrome"));
        assert!(msg.contains("chromium"));
        // The binary the hint recommends is worth naming where the user learns
        // they have none.
        assert!(msg.contains("chrome-headless-shell"));
    }
}
