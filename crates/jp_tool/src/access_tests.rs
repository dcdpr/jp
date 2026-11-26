use super::*;

#[test]
fn normalize_collapses_dot_and_parent() {
    assert_eq!(
        normalize_lexical(Utf8Path::new("a/./b/../c")),
        Some(Utf8PathBuf::from("a/c"))
    );
    assert_eq!(
        normalize_lexical(Utf8Path::new(".")),
        Some(Utf8PathBuf::new())
    );
    assert_eq!(
        normalize_lexical(Utf8Path::new("a/..")),
        Some(Utf8PathBuf::new())
    );
}

#[test]
fn normalize_rejects_escape_and_absolute() {
    assert_eq!(normalize_lexical(Utf8Path::new("../a")), None);
    assert_eq!(normalize_lexical(Utf8Path::new("a/../../b")), None);
    assert_eq!(normalize_lexical(Utf8Path::new("/etc/passwd")), None);
}

#[test]
fn prefix_match_is_component_aware() {
    assert_eq!(
        prefix_specificity(Utf8Path::new("src"), Utf8Path::new("src/lib.rs")),
        Some(1)
    );
    // `src` must not prefix-match `src_generated`.
    assert_eq!(
        prefix_specificity(Utf8Path::new("src"), Utf8Path::new("src_generated/x")),
        None
    );
    // The root matches everything.
    assert_eq!(
        prefix_specificity(Utf8Path::new(""), Utf8Path::new("any/where")),
        Some(0)
    );
}

#[test]
fn longest_prefix_wins_then_last() {
    let rules = vec![
        FsRule::new("").with_read(true),
        FsRule::new("src").with_read(false),
        FsRule::new("src").with_read(true),
    ];
    // Two `src` rules tie on specificity; the last declared wins.
    let rule = find_matching_rule(&rules, Utf8Path::new("src/lib.rs")).unwrap();
    assert!(rule.read());

    // The root rule wins for an unmatched subtree.
    let rule = find_matching_rule(&rules, Utf8Path::new("tests/x.rs")).unwrap();
    assert!(rule.read());
}

#[test]
fn granting_paths_drops_a_grant_a_later_rule_shadows() {
    // Append-merged config layers routinely produce two rules on one path.
    // `permits` breaks the tie toward the last, so the earlier grant is dead
    // and naming it in a refusal points the reader back at what was refused.
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("src").with_read(true),
            FsRule::new("src").with_read(false),
        ],
        ..AccessPolicy::default()
    };

    assert!(!policy.permits(Capability::Read, Utf8Path::new("src/lib.rs")));
    assert_eq!(policy.granting_paths(Capability::Read).count(), 0);
}

#[test]
fn granting_paths_keeps_a_grant_a_later_rule_restores() {
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("src").with_read(false),
            FsRule::new("src").with_read(true),
        ],
        ..AccessPolicy::default()
    };

    assert!(policy.permits(Capability::Read, Utf8Path::new("src/lib.rs")));
    assert_eq!(
        policy.granting_paths(Capability::Read).collect::<Vec<_>>(),
        vec![Utf8Path::new("src")]
    );
}

#[test]
fn granting_paths_keeps_a_broader_grant_a_deeper_rule_denies() {
    // Cross-path shadowing is not collapsed: the root is still reachable
    // everywhere except the one subtree the deeper rule closes.
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("").with_read(true),
            FsRule::new("src").with_read(false),
        ],
        ..AccessPolicy::default()
    };

    assert_eq!(
        policy.granting_paths(Capability::Read).collect::<Vec<_>>(),
        vec![Utf8Path::new(".")]
    );
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

#[test]
fn an_exact_env_rule_does_not_prefix_match() {
    let policy = env_policy(&[("GITHUB_TOKEN", true)]);

    assert!(policy.matching_env_rule("GITHUB_TOKEN").unwrap().read);
    assert_eq!(policy.matching_env_rule("GITHUB_TOKEN_LOG"), None);
    assert_eq!(policy.matching_env_rule("HOME"), None);
}

#[test]
fn a_star_suffix_matches_by_prefix() {
    let policy = env_policy(&[("AWS_*", true)]);

    assert!(policy.matching_env_rule("AWS_REGION").unwrap().read);
    assert!(policy.matching_env_rule("AWS_").unwrap().read);
    assert_eq!(policy.matching_env_rule("AW"), None);
}

#[test]
fn a_bare_star_matches_every_variable() {
    let policy = env_policy(&[("*", false)]);

    assert!(!policy.matching_env_rule("ANYTHING").unwrap().read);
}

#[test]
fn the_longest_literal_env_name_wins() {
    let policy = env_policy(&[("AWS_*", true), ("AWS_SECRET_ACCESS_KEY", false)]);

    assert!(policy.matching_env_rule("AWS_REGION").unwrap().read);
    assert!(
        !policy
            .matching_env_rule("AWS_SECRET_ACCESS_KEY")
            .unwrap()
            .read
    );
}

/// Specificity decides, not declaration order: the 21-byte exact rule wins over
/// the 4-byte `AWS_` prefix even though the prefix is declared last.
#[test]
fn a_longer_exact_rule_beats_a_shorter_prefix_declared_after_it() {
    let policy = env_policy(&[("AWS_SECRET_ACCESS_KEY", false), ("AWS_*", true)]);

    assert!(
        !policy
            .matching_env_rule("AWS_SECRET_ACCESS_KEY")
            .unwrap()
            .read
    );
}

/// `AWS_TOKEN` (exact, 9 bytes) beats `AWS_TOKEN*` (prefix, 9 bytes) regardless
/// of declaration order.
#[test]
fn exact_beats_prefix_at_equal_literal_size() {
    let policy = env_policy(&[("AWS_TOKEN", false), ("AWS_TOKEN*", true)]);
    assert!(!policy.matching_env_rule("AWS_TOKEN").unwrap().read);
    // The prefix rule still covers everything the exact rule does not.
    assert!(policy.matching_env_rule("AWS_TOKEN_ID").unwrap().read);

    let policy = env_policy(&[("AWS_TOKEN*", true), ("AWS_TOKEN", false)]);
    assert!(!policy.matching_env_rule("AWS_TOKEN").unwrap().read);
}

#[test]
fn equal_specificity_env_rules_break_toward_the_last() {
    let policy = env_policy(&[("CI", true), ("CI", false)]);

    assert!(!policy.matching_env_rule("CI").unwrap().read);
}

fn subtree_policy() -> AccessPolicy {
    AccessPolicy {
        fs: vec![
            FsRule::new("").with_read(true).with_write(true),
            FsRule::new("docs/ticket").with_read(true).with_write(false),
        ],
        ..AccessPolicy::default()
    }
}

/// The whole point: a grant on a directory cannot cover a subtree one of its
/// own rules closes, or moving the parent walks straight past the deny.
#[test]
fn a_subtree_grant_is_refused_by_a_deeper_deny() {
    let policy = subtree_policy();

    assert!(policy.permits(Capability::Delete, Utf8Path::new("docs")));
    assert!(!policy.permits_subtree(Capability::Delete, Utf8Path::new("docs")));

    // The sibling subtree is untouched by the deny.
    assert!(policy.permits_subtree(Capability::Delete, Utf8Path::new("docs/rfd")));
}

/// A deny only speaks for the capability it denies.
#[test]
fn a_deeper_deny_leaves_other_capabilities_alone() {
    let policy = subtree_policy();

    assert!(policy.permits_subtree(Capability::Read, Utf8Path::new("docs")));
    assert!(!policy.permits_subtree(Capability::Create, Utf8Path::new("docs")));
}

/// The rule on the target itself is not a descendant of it, so it decides
/// through the ordinary longest-prefix match rather than as a subtree deny.
#[test]
fn the_rule_on_the_target_itself_is_not_a_subtree_deny() {
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("").with_write(false),
            FsRule::new("build").with_write(true),
        ],
        ..AccessPolicy::default()
    };

    assert!(policy.permits_subtree(Capability::Delete, Utf8Path::new("build")));
}

/// An append-merged layer that restores a grant leaves nothing to deny: the
/// deeper path is re-evaluated, not read rule by rule.
#[test]
fn a_deeper_deny_a_later_layer_overrides_does_not_refuse() {
    let policy = AccessPolicy {
        fs: vec![
            FsRule::new("").with_write(true),
            FsRule::new("docs/ticket").with_write(false),
            FsRule::new("docs/ticket").with_write(true),
        ],
        ..AccessPolicy::default()
    };

    assert!(policy.permits_subtree(Capability::Delete, Utf8Path::new("docs")));
}

/// An unrestricted policy answers every question with yes, subtree or not.
#[test]
fn an_unrestricted_policy_permits_a_subtree() {
    let policy = AccessPolicy::default();

    assert!(policy.permits_subtree(Capability::Delete, Utf8Path::new("docs")));
}

/// Only a mutating capability on a directory asks the subtree question.
/// A file target has no subtree, and a listing does not reach into one.
#[test]
fn permits_entry_asks_the_subtree_question_only_for_a_directory_write() {
    let policy = subtree_policy();
    let docs = Utf8Path::new("docs");

    assert!(!policy.permits_entry(Capability::Delete, docs, true));
    assert!(policy.permits_entry(Capability::Delete, docs, false));
    assert!(policy.permits_entry(Capability::Read, docs, true));
}

#[test]
fn only_create_update_and_delete_mutate() {
    assert!(Capability::Create.mutates());
    assert!(Capability::Update.mutates());
    assert!(Capability::Delete.mutates());
    assert!(!Capability::Read.mutates());
    assert!(!Capability::Execute.mutates());
}

fn mount_on(target: &str) -> FsRule {
    FsRule::new("up")
        .with_external(true)
        .with_approved_target(Some(target.into()))
        .with_read(true)
}

#[test]
fn exclusion_sits_where_the_workspace_reappears_in_the_mount() {
    let root = Utf8Path::new("/w/a/b");
    assert_eq!(
        mount_on("/w/a").workspace_exclusion(root),
        Some(FsRule::new("up/b"))
    );
    assert_eq!(
        mount_on("/w").workspace_exclusion(root),
        Some(FsRule::new("up/a/b"))
    );
}

#[test]
fn no_exclusion_for_a_target_that_does_not_contain_the_workspace() {
    let root = Utf8Path::new("/w/a/b");
    // `/w/a/b` itself is rejected at compile time; `/w/ab` only shares a
    // name prefix.
    for target in ["/w/a/b", "/w/a/sibling", "/w/ab", "/elsewhere"] {
        assert_eq!(
            mount_on(target).workspace_exclusion(root),
            None,
            "target: {target}"
        );
    }
    assert_eq!(FsRule::new("src").workspace_exclusion(root), None);
}

#[test]
fn write_alias_expands() {
    let rule = FsRule::new("x").with_write(true);
    assert!(rule.create() && rule.update() && rule.delete());
    assert!(!rule.read() && !rule.execute());

    let rule = FsRule::new("x").with_write(true).with_delete(false);
    assert!(rule.create() && rule.update() && !rule.delete());
}
