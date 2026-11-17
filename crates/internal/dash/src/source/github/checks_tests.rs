use super::*;

#[test]
fn test_contexts_tally_check_runs_and_commit_statuses() {
    let runs = r#"{"total_count": 4, "check_runs": [
        {"name": "lint", "status": "completed", "conclusion": "success", "started_at": "2026-09-22T10:00:00Z"},
        {"name": "shear", "status": "completed", "conclusion": "skipped", "started_at": "2026-09-22T10:00:00Z"},
        {"name": "test", "status": "in_progress", "conclusion": null, "started_at": "2026-09-22T10:00:00Z"},
        {"name": "docs", "status": "completed", "conclusion": "failure", "started_at": "2026-09-22T10:00:00Z"}
    ]}"#;
    let status = r#"{"state": "pending", "statuses": [
        {"context": "coverage/coveralls", "state": "pending"}
    ]}"#;

    assert_eq!(checks(contexts(runs, status).unwrap(), &[]), Checks {
        passed: 2,
        pending: 2,
        failed: 1,
    });
}

#[test]
fn test_contexts_of_a_commit_without_checks() {
    let runs = r#"{"total_count": 0, "check_runs": []}"#;
    let status = r#"{"state": "pending", "statuses": []}"#;

    assert_eq!(
        checks(contexts(runs, status).unwrap(), &[]),
        Checks::default()
    );
}

#[test]
fn test_parse_required_reads_only_required_status_check_rules() {
    let rules = r#"[
        {"type": "deletion"},
        {"type": "pull_request", "parameters": {"required_approving_review_count": 0}},
        {"type": "required_status_checks", "parameters": {
            "strict_required_status_checks_policy": false,
            "required_status_checks": [{"context": "lint"}, {"context": "test", "integration_id": 15368}]
        }}
    ]"#;

    assert_eq!(parse_required(rules).unwrap(), ["lint", "test"]);
    assert_eq!(parse_required("[]").unwrap(), Vec::<String>::new());
}

#[test]
fn test_required_checks_that_have_not_reported_count_as_pending() {
    let runs = r#"{"check_runs": [
        {"name": "lint", "status": "completed", "conclusion": "success", "started_at": "2026-01-01T00:00:00Z"}
    ]}"#;
    let status = r#"{"statuses": []}"#;
    let required = ["lint".to_owned(), "test".to_owned(), "build".to_owned()];

    assert_eq!(checks(contexts(runs, status).unwrap(), &required), Checks {
        passed: 1,
        pending: 2,
        failed: 0,
    });
}

#[test]
fn test_checks_count_only_the_latest_attempt_of_a_rerun() {
    let contexts = vec![
        Context::CheckRun {
            name: "test".to_owned(),
            status: "COMPLETED".to_owned(),
            conclusion: Some("FAILURE".to_owned()),
            started_at: Some("2026-09-22T10:00:00Z".to_owned()),
        },
        Context::CheckRun {
            name: "test".to_owned(),
            status: "COMPLETED".to_owned(),
            conclusion: Some("SUCCESS".to_owned()),
            started_at: Some("2026-09-22T11:00:00Z".to_owned()),
        },
    ];

    assert_eq!(checks(contexts, &[]), Checks {
        passed: 1,
        pending: 0,
        failed: 0,
    });
}

#[test]
fn test_checks_treat_an_unstarted_rerun_as_the_latest() {
    // The queued re-run has no start time yet, and must win over the failed
    // attempt whichever order GitHub lists them in.
    let failed = || Context::CheckRun {
        name: "test".to_owned(),
        status: "COMPLETED".to_owned(),
        conclusion: Some("FAILURE".to_owned()),
        started_at: Some("2026-09-22T10:00:00Z".to_owned()),
    };
    let queued = || Context::CheckRun {
        name: "test".to_owned(),
        status: "QUEUED".to_owned(),
        conclusion: None,
        started_at: None,
    };
    let pending = Checks {
        passed: 0,
        pending: 1,
        failed: 0,
    };

    assert_eq!(checks(vec![failed(), queued()], &[]), pending);
    assert_eq!(checks(vec![queued(), failed()], &[]), pending);
}
