use super::*;

fn attr(source: &str) -> Attribute {
    let item: syn::ItemFn = syn::parse_str(&format!("{source} fn f() {{}}")).unwrap();
    item.attrs.into_iter().next().unwrap()
}

#[test]
fn test_alone_requires_test() {
    assert_eq!(classify(&attr("#[cfg(test)]")), TestCfg::Requires);
}

#[test]
fn all_holding_test_requires_test() {
    assert_eq!(
        classify(&attr("#[cfg(all(test, unix))]")),
        TestCfg::Requires
    );
    assert_eq!(
        classify(&attr("#[cfg(all(unix, all(test, feature = \"x\")))]")),
        TestCfg::Requires
    );
}

/// A feature turns the item on outside tests, so it is production code.
#[test]
fn any_with_a_non_test_branch_requires_nothing() {
    assert_eq!(
        classify(&attr("#[cfg(any(test, feature = \"mock\"))]")),
        TestCfg::Neither
    );
}

#[test]
fn any_whose_every_branch_requires_test_requires_test() {
    assert_eq!(
        classify(&attr("#[cfg(any(test, all(test, unix)))]")),
        TestCfg::Requires
    );
}

#[test]
fn negating_test_excludes_tests() {
    assert_eq!(classify(&attr("#[cfg(not(test))]")), TestCfg::Excludes);
    assert_eq!(
        classify(&attr("#[cfg(all(unix, not(test)))]")),
        TestCfg::Excludes
    );
    assert_eq!(
        classify(&attr("#[cfg(not(any(test, miri)))]")),
        TestCfg::Excludes
    );
}

#[test]
fn predicates_without_test_say_nothing() {
    assert_eq!(classify(&attr("#[cfg(unix)]")), TestCfg::Neither);
    assert_eq!(classify(&attr("#[cfg(not(unix))]")), TestCfg::Neither);
}

/// `cfg_attr` adds attributes, not code, so the rule leaves it alone.
#[test]
fn cfg_attr_says_nothing() {
    assert_eq!(
        classify(&attr("#[cfg_attr(test, derive(Debug))]")),
        TestCfg::Neither
    );
}

#[test]
fn test_function_attributes_end_in_test() {
    assert!(is_test_function(&attr("#[test]")));
    assert!(is_test_function(&attr("#[tokio::test]")));
    assert!(is_test_function(&attr("#[test_log::test]")));
    assert!(!is_test_function(&attr("#[cfg(test)]")));
    assert!(!is_test_function(&attr("#[should_panic]")));
}
