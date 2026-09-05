use assert_matches::assert_matches;
use camino_tempfile::Utf8TempDir;
use jp_tool::{AccessPolicy, Action, EnvRule, FsRule};
use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

/// A workspace containing `crates/lib.rs` and `secrets/key`.
fn workspace() -> Utf8TempDir {
    let dir = camino_tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("crates")).unwrap();
    std::fs::write(dir.path().join("crates/lib.rs"), "").unwrap();
    std::fs::create_dir(dir.path().join("secrets")).unwrap();
    std::fs::write(dir.path().join("secrets/key"), "").unwrap();
    dir
}

fn ctx(dir: &Utf8TempDir, access: Option<AccessPolicy>) -> Context {
    Context {
        root: dir.path().to_path_buf(),
        action: Action::Run,
        access,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

fn tool(arguments: &Value) -> Tool {
    Tool {
        name: "bash".to_owned(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: Map::new(),
        options: Map::new(),
    }
}

fn env_policy(rules: &[(&str, bool)]) -> AccessPolicy {
    AccessPolicy {
        env: rules
            .iter()
            .map(|(name, read)| EnvRule {
                name: (*name).to_owned(),
                read: *read,
            })
            .collect(),
        ..AccessPolicy::default()
    }
}

/// Resolve with an environment that has no variables set at all.
fn resolve_bare(ctx: &Context, t: &Tool) -> Resolution {
    resolve(ctx, t, |_| None).unwrap()
}

fn message(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Error { message, .. } => message.clone(),
        other => panic!("expected an error outcome, got {other:?}"),
    }
}

#[test]
fn commands_become_one_script_under_strict_mode() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["echo one", "echo two"]}));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(plan.script, "set -euo pipefail\necho one\necho two\n");
    assert_eq!(plan.image, DEFAULT_IMAGE);
    assert_eq!(plan.install, None);
    assert!(plan.mounts.is_empty());
    assert!(plan.envs.is_empty());
}

#[test]
fn an_install_script_defaults_to_the_unprivileged_user() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options.insert(
        "install_script".to_owned(),
        json!("apk add --no-cache python3"),
    );

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(
        plan.install,
        Some(Install {
            script: "apk add --no-cache python3".to_owned(),
            run_as: "nonroot".to_owned(),
        })
    );
}

#[test]
fn run_as_overrides_the_default_user() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options.insert(
        "install_script".to_owned(),
        json!("apk add --no-cache python3"),
    );
    t.options.insert("run_as".to_owned(), json!("root"));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(plan.install.unwrap().run_as, "root");
}

/// `run_as` only takes effect through the image build, so accepting it without
/// `install_script` would be config that silently does nothing.
#[test]
fn run_as_without_an_install_script_is_rejected() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options.insert("run_as".to_owned(), json!("root"));

    let error = resolve(&ctx(&dir, None), &t, |_| None).unwrap_err();

    assert_eq!(
        error.to_string(),
        "The `run_as` option for tool 'bash' requires `install_script`; without a build there is \
         no image to set a user on."
    );
}

#[test]
fn a_non_string_install_script_fails_the_invocation() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options
        .insert("install_script".to_owned(), json!(["apk", "add"]));

    let error = resolve(&ctx(&dir, None), &t, |_| None).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid `install_script` option for tool 'bash': expected a string, got [\"apk\",\"add\"]"
    );
}

#[test]
fn a_single_command_may_be_passed_as_a_string() {
    let dir = workspace();
    let t = tool(&json!({"commands": "echo hi"}));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(plan.script, "set -euo pipefail\necho hi\n");
}

#[test]
fn an_empty_command_list_is_rejected() {
    let dir = workspace();
    let t = tool(&json!({"commands": []}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert_eq!(
        message(&outcome),
        "`commands` must contain at least one command."
    );
}

#[test]
fn the_image_option_overrides_the_default() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options
        .insert("image".to_owned(), json!("ghcr.io/example/jp-bash:v1"));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(plan.image, "ghcr.io/example/jp-bash:v1");
}

/// A malformed `image` fails the invocation rather than silently falling back
/// to the default, which would run a different image than the operator
/// configured and report success for it.
#[test]
fn a_non_string_image_option_fails_the_invocation() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"]}));
    t.options.insert("image".to_owned(), json!(42));

    let error = resolve(&ctx(&dir, None), &t, |_| None).unwrap_err();

    assert_eq!(
        error.to_string(),
        "Invalid `image` option for tool 'bash': expected a string, got 42"
    );
}

// ---------------------------------------------------------------------------
// Environment variables
// ---------------------------------------------------------------------------

#[test]
fn a_granting_rule_forwards_the_variable_without_asking() {
    let dir = workspace();
    let ctx = ctx(&dir, Some(env_policy(&[("CI", true)])));
    let t = tool(&json!({"commands": ["true"], "envs": ["CI"]}));

    let plan = assert_matches!(
        resolve(&ctx, &t, |_| Some("1".to_owned())).unwrap(),
        Resolution::Run(plan) => plan
    );

    assert_eq!(plan.envs, vec![("CI".to_owned(), "1".to_owned())]);
}

/// A denying rule is the operator's decision, so it is refused outright rather
/// than escalated to the user.
#[test]
fn a_denying_rule_refuses_the_call() {
    let dir = workspace();
    let ctx = ctx(
        &dir,
        Some(env_policy(&[("AWS_*", true), ("AWS_SECRET", false)])),
    );
    let t = tool(&json!({"commands": ["true"], "envs": ["AWS_SECRET"]}));

    let outcome = assert_matches!(
        resolve(&ctx, &t, |_| Some("s3cret".to_owned())).unwrap(),
        Resolution::Stop(o) => o
    );

    assert_matches!(outcome, Outcome::Error {
        transient: false,
        ..
    });
    assert_eq!(
        message(&outcome),
        "Access to environment variable 'AWS_SECRET' is denied by this tool's access.env \
         configuration."
    );
}

/// An unmentioned variable escalates to the user.
/// The question names the variable but never carries its value.
#[test]
fn an_unmentioned_variable_asks_the_user() {
    let dir = workspace();
    let ctx = ctx(&dir, Some(env_policy(&[("CI", true)])));
    let t = tool(&json!({"commands": ["true"], "envs": ["GITHUB_TOKEN"]}));

    let outcome = assert_matches!(
        resolve(&ctx, &t, |_| Some("ghp_secret".to_owned())).unwrap(),
        Resolution::Stop(o) => o
    );

    let question = assert_matches!(outcome, Outcome::NeedsInput { question } => question);
    assert_eq!(question.id, "expose_env_GITHUB_TOKEN");
    assert_eq!(
        question.text,
        "Expose the environment variable 'GITHUB_TOKEN' to the container?"
    );
    assert_eq!(question.default, Some(Value::Bool(false)));
    assert_eq!(question.pre_amble, None);
}

/// No `access.env` at all denies rather than permits: `access.fs` treats an
/// empty rule list as unrestricted, but silently handing over any variable the
/// assistant names is not a default worth having.
#[test]
fn an_absent_policy_still_asks() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "envs": ["HOME"]}));

    let outcome = assert_matches!(
        resolve(&ctx(&dir, None), &t, |_| Some("/root".to_owned())).unwrap(),
        Resolution::Stop(o) => o
    );

    assert_matches!(outcome, Outcome::NeedsInput { .. });
}

#[test]
fn an_affirmative_answer_forwards_the_variable() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"], "envs": ["GITHUB_TOKEN"]}));
    t.answers
        .insert("expose_env_GITHUB_TOKEN".to_owned(), json!(true));

    let plan = assert_matches!(
        resolve(&ctx(&dir, None), &t, |_| Some("ghp_secret".to_owned())).unwrap(),
        Resolution::Run(plan) => plan
    );

    assert_eq!(plan.envs, vec![(
        "GITHUB_TOKEN".to_owned(),
        "ghp_secret".to_owned()
    )]);
}

#[test]
fn a_declined_answer_refuses_the_call() {
    let dir = workspace();
    let mut t = tool(&json!({"commands": ["true"], "envs": ["GITHUB_TOKEN"]}));
    t.answers
        .insert("expose_env_GITHUB_TOKEN".to_owned(), json!(false));

    let outcome = assert_matches!(
        resolve(&ctx(&dir, None), &t, |_| Some("ghp_secret".to_owned())).unwrap(),
        Resolution::Stop(o) => o
    );

    assert_eq!(
        message(&outcome),
        "Not exposing environment variable 'GITHUB_TOKEN': the user declined."
    );
}

/// Under `set -u` a granted-but-unset variable would fail deep inside the
/// script; failing here names the actual problem.
#[test]
fn a_granted_but_unset_variable_refuses_the_call() {
    let dir = workspace();
    let ctx = ctx(&dir, Some(env_policy(&[("CI", true)])));
    let t = tool(&json!({"commands": ["true"], "envs": ["CI"]}));

    let outcome = assert_matches!(resolve_bare(&ctx, &t), Resolution::Stop(o) => o);

    assert_eq!(
        message(&outcome),
        "Environment variable 'CI' is not set on the host."
    );
}

#[test]
fn a_malformed_variable_name_is_rejected() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "envs": ["PATH=/evil"]}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert_eq!(
        message(&outcome),
        "'PATH=/evil' is not a valid environment variable name; names must match \
         [A-Za-z_][A-Za-z0-9_]*."
    );
}

// ---------------------------------------------------------------------------
// Mounts
// ---------------------------------------------------------------------------

#[test]
fn a_mount_lands_under_the_workspace_root_in_the_container() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "mounts": ["crates", "./crates/lib.rs"]}));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    let targets: Vec<&str> = plan.mounts.iter().map(|(_, t)| t.as_str()).collect();
    assert_eq!(targets, vec![
        "/workspace/crates",
        "/workspace/crates/lib.rs"
    ]);
}

#[test]
fn mounting_the_workspace_root_targets_the_mount_root() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "mounts": ["."]}));

    let plan = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Run(plan) => plan);

    assert_eq!(plan.mounts.len(), 1);
    assert_eq!(plan.mounts[0].1, "/workspace");
}

#[test]
fn a_path_denied_by_access_fs_is_refused() {
    let dir = workspace();
    let policy = AccessPolicy {
        fs: vec![FsRule::new("crates").with_read(true)],
        ..AccessPolicy::default()
    };
    let ctx = ctx(&dir, Some(policy));
    let t = tool(&json!({"commands": ["true"], "mounts": ["secrets"]}));

    let outcome = assert_matches!(resolve_bare(&ctx, &t), Resolution::Stop(o) => o);

    assert!(
        message(&outcome).starts_with("Cannot mount 'secrets': access denied: read on secrets"),
        "got: {}",
        message(&outcome)
    );
}

#[test]
fn an_escaping_mount_is_refused() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "mounts": ["../elsewhere"]}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert_eq!(
        message(&outcome),
        "Cannot mount '../elsewhere': path escapes the workspace: ../elsewhere"
    );
}

#[test]
fn an_absolute_mount_is_refused() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "mounts": ["/etc"]}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert_eq!(
        message(&outcome),
        "Cannot mount '/etc': absolute paths are not permitted: /etc"
    );
}

/// A symlink pointing out of the workspace resolves outside it, and no
/// `external` rule approves that target.
#[test]
fn a_mount_resolving_outside_the_workspace_is_refused() {
    let dir = workspace();
    let outside = camino_tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();

    let t = tool(&json!({"commands": ["true"], "mounts": ["link"]}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert!(
        message(&outcome).contains("resolves outside the workspace"),
        "got: {}",
        message(&outcome)
    );
}

#[test]
fn a_missing_mount_is_refused() {
    let dir = workspace();
    let t = tool(&json!({"commands": ["true"], "mounts": ["nope"]}));

    let outcome = assert_matches!(resolve_bare(&ctx(&dir, None), &t), Resolution::Stop(o) => o);

    assert_eq!(message(&outcome), "Cannot mount 'nope': no such path.");
}

// ---------------------------------------------------------------------------
// Argument preview
// ---------------------------------------------------------------------------

#[test]
fn the_preview_lists_variables_mounts_and_commands() {
    let dir = workspace();
    let mut ctx = ctx(&dir, None);
    ctx.action = Action::FormatArguments;
    let t = tool(&json!({
        "commands": ["curl -s https://example.com", "jq .name"],
        "envs": ["ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
        "mounts": ["crates", "./crates/lib.rs"],
    }));

    let outcome = assert_matches!(resolve_bare(&ctx, &t), Resolution::Stop(o) => o);

    assert_eq!(
        outcome.unwrap_content(),
        "**Environment variables**\n\n- `ANTHROPIC_API_KEY`\n- `OPENAI_API_KEY`\n\n**Workspace \
         mounts**\n\n- `crates`\n- `./crates/lib.rs`\n\n**Commands**\n\n```bash\ncurl -s \
         https://example.com\njq .name\n```\n"
    );
}

#[test]
fn the_preview_omits_empty_sections() {
    let dir = workspace();
    let mut ctx = ctx(&dir, None);
    ctx.action = Action::FormatArguments;
    let t = tool(&json!({"commands": ["uname -a"]}));

    let outcome = assert_matches!(resolve_bare(&ctx, &t), Resolution::Stop(o) => o);

    assert_eq!(
        outcome.unwrap_content(),
        "**Commands**\n\n```bash\nuname -a\n```\n"
    );
}

/// The preview renders what the assistant asked for, so the user approves the
/// real request.
/// It must not need a container runtime, a readable mount, or a granted
/// variable to produce one.
#[test]
fn the_preview_runs_before_any_policy_check() {
    let dir = workspace();
    let mut ctx = ctx(&dir, Some(env_policy(&[("GITHUB_TOKEN", false)])));
    ctx.action = Action::FormatArguments;
    let t = tool(&json!({
        "commands": ["true"],
        "envs": ["GITHUB_TOKEN"],
        "mounts": ["/etc/passwd"],
    }));

    let outcome = assert_matches!(resolve_bare(&ctx, &t), Resolution::Stop(o) => o);

    assert_eq!(
        outcome.unwrap_content(),
        "**Environment variables**\n\n- `GITHUB_TOKEN`\n\n**Workspace mounts**\n\n- \
         `/etc/passwd`\n\n**Commands**\n\n```bash\ntrue\n```\n"
    );
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

#[test]
fn output_beyond_the_cap_is_truncated_with_a_note() {
    let dir = workspace();
    let plan = Plan {
        image: DEFAULT_IMAGE.to_owned(),
        install: None,
        mounts: vec![],
        envs: vec![],
        script: "set -euo pipefail\nyes\n".to_owned(),
    };
    let runner = crate::util::runner::MockProcessRunner::success("x".repeat(MAX_OUTPUT_BYTES + 10));

    let content = execute(dir.path(), Runtime::Docker, &plan, &runner)
        .unwrap()
        .unwrap_content();

    assert!(
        content.contains(&format!(
            "[Truncated: showing {MAX_OUTPUT_BYTES} of {} bytes]",
            MAX_OUTPUT_BYTES + 10
        )),
        "got: {content}"
    );
}

/// A shell script's exit code is its primary signal; an omitted `status` is
/// easy to misread as "no output".
#[test]
fn a_successful_run_still_reports_its_exit_code() {
    let dir = workspace();
    let plan = Plan {
        image: DEFAULT_IMAGE.to_owned(),
        install: None,
        mounts: vec![],
        envs: vec![],
        script: "set -euo pipefail\necho hi\n".to_owned(),
    };
    let runner = crate::util::runner::MockProcessRunner::success("hi");

    let content = execute(dir.path(), Runtime::Docker, &plan, &runner)
        .unwrap()
        .unwrap_content();

    assert_eq!(
        content,
        "```xml\n<CommandOutput>\n  <stdout>hi</stdout>\n  \
         <status>0</status>\n</CommandOutput>\n```"
    );
}

/// The install script's whole point: a base image without python gets it, and
/// the built image is what the commands then run in.
///
/// Also covers the `USER` round-trip — the script installs as root, and
/// `python3 -c` runs as `nonroot` afterwards, which only works if the restore
/// resolved to a user the base image actually defines.
///
/// Needs a container runtime and network access for the package install.
/// The first run builds; later runs hit the cached tag and take about as long
/// as any other call.
#[test]
#[ignore = "builds a real image"]
fn an_install_script_adds_a_tool_the_base_image_lacks() {
    let dir = workspace();
    let plan = Plan {
        image: DEFAULT_IMAGE.to_owned(),
        install: Some(Install {
            script: "apk add --no-cache python3".to_owned(),
            run_as: DEFAULT_RUN_AS.to_owned(),
        }),
        mounts: vec![],
        envs: vec![],
        script: "set -euo pipefail\npython3 -c 'print(1 + 1)'\nid -un\n".to_owned(),
    };
    let runtime = detect().expect("a container runtime must be installed");

    let content = execute(dir.path(), runtime, &plan, &DuctProcessRunner)
        .unwrap()
        .unwrap_content();

    assert_eq!(
        content,
        "```xml\n<CommandOutput>\n  <stdout>2\nnonroot</stdout>\n  \
         <status>0</status>\n</CommandOutput>\n```"
    );
}

/// Read-only mounts are what keep this tool from being able to replace the
/// workspace-editing tools, so it is worth proving against a real runtime: a
/// runtime that ignored `:ro` would drop the guarantee silently.
///
/// Runs against [`DEFAULT_IMAGE`], so it also covers the two things about an
/// image that this tool depends on: that `bash` is present, and that no
/// `ENTRYPOINT` swallows the `bash -c` invocation.
///
/// Needs a container runtime that is allowed to share the temp directory
/// (Docker Desktop restricts which host paths it will bind-mount).
#[test]
#[ignore = "starts a real container"]
fn a_mounted_path_is_read_only_in_the_container() {
    let dir = workspace();
    let plan = Plan {
        image: DEFAULT_IMAGE.to_owned(),
        install: None,
        mounts: vec![(
            dir.path().join("crates").canonicalize_utf8().unwrap(),
            "/workspace/crates".to_owned(),
        )],
        envs: vec![],
        script: "set -euo pipefail\necho written > /workspace/crates/lib.rs\n".to_owned(),
    };
    let runtime = detect().expect("a container runtime must be installed");

    let content = execute(dir.path(), runtime, &plan, &DuctProcessRunner)
        .unwrap()
        .unwrap_content();

    // Under `set -e` bash exits 1 when the redirect cannot open the file.
    assert!(content.contains("<status>1</status>"), "got: {content}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("crates/lib.rs")).unwrap(),
        ""
    );
}
