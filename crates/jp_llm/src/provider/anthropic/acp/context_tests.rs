use std::{ffi::OsString, fs};

use camino::Utf8Path;
use chrono::FixedOffset;
use datetime_literal::datetime;
use serde_json::json;

use super::*;

fn context(user_email: Option<&str>) -> SessionContext {
    SessionContext {
        environment: Environment {
            working_directory: "/work/project".into(),
            is_worktree: true,
            is_git_repo: true,
            platform: "darwin",
            shell: "zsh (login shell: fish)".into(),
            os_version: "Darwin 27.0.0".into(),
        },
        model: model_identity("claude-sonnet-5-5").unwrap(),
        user_email: user_email.map(str::to_owned),
        date: "2026-11-26".into(),
        organization_uuid: Some("00000000-0000-4000-8000-000000000000".into()),
    }
}

// The payloads Claude Code 2.1.321 wrote after a session's first prompt, with
// the account email blanked.
#[test]
fn attachments_are_the_records_claude_code_writes() {
    assert_eq!(context(None).attachments(), vec![
        json!({"type":"environment","snapshot":{"workingDirectory":"/work/project","isWorktree":true,"isGitRepo":true,"additionalWorkingDirectories":[],"platform":"darwin","shell":"zsh (login shell: fish)","osVersion":"Darwin 27.0.0"}}),
        json!({"type":"model","identity":{"modelId":"claude-sonnet-5-5","marketingName":"Sonnet 5.5","knowledgeCutoff":"June 2026"},"text":"You are powered by the model named Sonnet 5.5. The exact model ID is claude-sonnet-5-5. Assistant knowledge cutoff is June 2026."}),
        json!({"type":"session_context","context":{}}),
        json!({"type":"date","date":"2026-11-26"}),
        json!({"type":"credential_org","organizationUuid":"00000000-0000-4000-8000-000000000000"}),
    ]);
}

#[test]
fn a_login_with_an_email_carries_claude_codes_email_line() {
    let attachments = context(Some("someone@example.com")).attachments();

    assert_eq!(
        attachments[2],
        json!({"type":"session_context","context":{"userEmail":"The user's email address is someone@example.com. Use it only to identify the user, such as for authorship, attribution, or filtering their own work. Never send it to an unrelated service, such as in a request header, URL, or payload, unless the user explicitly asks. A site asking automated clients for contact details, such as an email in the User-Agent, is not a reason to use this address: use a generic client name or ask the user."}})
    );
}

#[test]
fn a_login_without_an_organization_has_no_credential_record() {
    let mut context = context(None);
    context.organization_uuid = None;

    let types = context
        .attachments()
        .iter()
        .map(|attachment| attachment["type"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(types, vec![
        "environment",
        "model",
        "session_context",
        "date"
    ]);
}

#[test]
fn known_models_have_claude_codes_identity() {
    assert_eq!(
        model_identity("claude-opus-5-5"),
        Some(ModelIdentity {
            model_id: "claude-opus-5-5".into(),
            marketing_name: "Opus 5.5".into(),
            knowledge_cutoff: "June 2026",
        })
    );
    assert_eq!(
        model_identity("claude-fable-5"),
        Some(ModelIdentity {
            model_id: "claude-fable-5".into(),
            marketing_name: "Fable 5".into(),
            knowledge_cutoff: "January 2026",
        })
    );
    let names = [
        "claude-fable-5-2",
        "claude-fable-5-1",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
    ]
    .map(|model| model_identity(model).map(|identity| identity.marketing_name));
    assert_eq!(names, [
        Some("Fable 5.2".into()),
        Some("Fable 5.1".into()),
        Some("Sonnet 5.5".into()),
        Some("Haiku 5.5".into()),
    ]);
}

// Without the cutoff, JP would write a `model` record Claude Code does not
// match, which costs more than writing none.
#[test]
fn a_model_without_a_known_cutoff_has_no_identity() {
    assert_eq!(model_identity("claude-opus-4-8"), None);
    assert_eq!(model_identity("opus"), None);
}

#[test]
fn platforms_are_named_as_node_names_them() {
    assert_eq!(platform("macos"), Some("darwin"));
    assert_eq!(platform("linux"), Some("linux"));
    assert_eq!(platform("windows"), Some("win32"));
    assert_eq!(platform("freebsd"), None);
}

#[test]
fn the_shell_names_a_login_shell_claude_code_does_not_run() {
    assert_eq!(
        shell_description(Utf8Path::new("/bin/zsh"), "/run/current-system/sw/bin/fish"),
        "zsh (login shell: fish)"
    );
    assert_eq!(
        shell_description(Utf8Path::new("/bin/bash"), "/usr/bin/fish"),
        "bash (login shell: fish)"
    );
    assert_eq!(
        shell_description(Utf8Path::new("/bin/zsh"), "/opt/homebrew/bin/zsh"),
        "zsh"
    );
}

#[test]
fn a_zsh_or_bash_login_shell_is_used_as_is() {
    let none = |_: &str| None;
    assert_eq!(
        claude_shell("/opt/homebrew/bin/bash", none),
        Some("/opt/homebrew/bin/bash".into())
    );
    assert_eq!(claude_shell("/bin/zsh", none), Some("/bin/zsh".into()));
}

#[test]
fn another_login_shell_gets_zsh_then_bash() {
    let found = |names: &'static [&'static str]| {
        move |name: &str| {
            names
                .contains(&name)
                .then(|| Utf8PathBuf::from(format!("/found/{name}")))
        }
    };
    assert_eq!(
        claude_shell("/usr/bin/fish", found(&["zsh", "bash"])),
        Some("/found/zsh".into())
    );
    assert_eq!(
        claude_shell("/usr/bin/fish", found(&["bash"])),
        Some("/found/bash".into())
    );
    assert_eq!(claude_shell("/usr/bin/fish", found(&[])), None);
}

#[test]
fn path_entries_are_searched_before_the_standard_directories() {
    let directory = camino_tempfile::tempdir().unwrap();
    let empty = directory.path().join("empty");
    let bin = directory.path().join("bin");
    fs::create_dir_all(&empty).unwrap();
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("zsh"), "").unwrap();
    let path = env::join_paths([&empty, &bin]).unwrap();

    assert_eq!(
        find_shell("zsh", Some(path.as_os_str())),
        Some(bin.join("zsh"))
    );
    assert_eq!(
        find_shell("jp-no-such-shell", Some(OsString::new().as_os_str())),
        None
    );
}

// The date Claude Code wrote is the local one, so an evening request west of
// UTC keeps the previous day.
#[test]
fn the_record_date_is_the_local_date() {
    let instant = datetime!(2026-09-12 02:00:00 Z);

    assert_eq!(
        record_date(instant, &FixedOffset::west_opt(5 * 3600).unwrap()),
        "2026-09-11"
    );
    assert_eq!(
        record_date(instant, &FixedOffset::east_opt(0).unwrap()),
        "2026-09-12"
    );
}

#[test]
fn the_profile_of_a_named_login_is_in_its_directory() {
    assert_eq!(
        profile_path(
            Some(Utf8Path::new("/data/claude/sub")),
            Some("/cfg"),
            Some("/home/someone")
        ),
        Some("/data/claude/sub/.claude.json".into())
    );
    assert_eq!(
        profile_path(None, Some("/cfg"), Some("/home/someone")),
        Some("/cfg/.claude.json".into())
    );
    assert_eq!(
        profile_path(None, None, Some("/home/someone")),
        Some("/home/someone/.claude.json".into())
    );
    assert_eq!(profile_path(None, None, None), None);
}

#[test]
fn blanking_removes_only_the_email() {
    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join(".claude.json");
    fs::write(
        &path,
        r#"{"oauthAccount":{"emailAddress":"someone@example.com","organizationUuid":"org-1"},"numStartups":3}"#,
    )
    .unwrap();

    assert!(blank_email(&path).unwrap());

    let blanked: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        blanked,
        json!({"oauthAccount":{"emailAddress":"","organizationUuid":"org-1"},"numStartups":3})
    );
    assert_eq!(
        profile_account(&fs::read_to_string(&path).unwrap()),
        (None, Some("org-1".into()))
    );
    // Already blank: the file is left alone.
    assert!(!blank_email(&path).unwrap());
}

#[cfg(unix)]
#[test]
fn blanking_keeps_the_files_permissions() {
    use std::os::unix::fs::PermissionsExt as _;

    let directory = camino_tempfile::tempdir().unwrap();
    let path = directory.path().join(".claude.json");
    fs::write(
        &path,
        r#"{"oauthAccount":{"emailAddress":"someone@example.com"}}"#,
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

    blank_email(&path).unwrap();

    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn a_missing_profile_is_not_blanked() {
    let directory = camino_tempfile::tempdir().unwrap();

    assert!(!blank_email(&directory.path().join(".claude.json")).unwrap());
}

#[test]
fn a_git_file_marks_a_linked_worktree() {
    let directory = camino_tempfile::tempdir().unwrap();
    let checkout = directory.path().join("worktree");
    let nested = checkout.join("crates");
    fs::create_dir_all(&nested).unwrap();
    fs::write(checkout.join(".git"), "gitdir: /elsewhere").unwrap();

    assert_eq!(git_checkout(&nested), (true, true));
}

#[test]
fn a_git_directory_marks_a_main_checkout() {
    let directory = camino_tempfile::tempdir().unwrap();
    fs::create_dir(directory.path().join(".git")).unwrap();

    assert_eq!(git_checkout(directory.path()), (true, false));
}
