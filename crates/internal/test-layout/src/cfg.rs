//! What an attribute says about `test`.

use syn::{Attribute, Meta, MetaList, Token, punctuated::Punctuated};

/// What a `cfg` attribute's predicate says about `test`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestCfg {
    /// The item exists only when compiling tests: the predicate is `test`, an
    /// `all(…)` holding one that requires it, or an `any(…)` whose every
    /// branch does.
    Requires,

    /// The predicate negates `test`, so the item exists in production and not
    /// under test.
    Excludes,

    /// The predicate ties the item to neither, as `any(test, feature = "mock")`
    /// does: a feature turns it on in production too.
    Neither,
}

/// Read a `cfg` attribute's predicate.
///
/// Any other attribute, `cfg_attr` included, and a predicate that does not
/// parse, is [`TestCfg::Neither`].
pub(crate) fn classify(attr: &Attribute) -> TestCfg {
    if !attr.path().is_ident("cfg") {
        return TestCfg::Neither;
    }
    let Ok(predicate) = attr.parse_args::<Meta>() else {
        return TestCfg::Neither;
    };

    if requires(&predicate) {
        TestCfg::Requires
    } else if negates(&predicate) {
        TestCfg::Excludes
    } else {
        TestCfg::Neither
    }
}

/// The first attribute in `attrs` whose `cfg` requires `test`.
pub(crate) fn requiring_test(attrs: &[Attribute]) -> Option<&Attribute> {
    attrs
        .iter()
        .find(|attr| classify(attr) == TestCfg::Requires)
}

/// Whether `attr` marks a test function: `#[test]`, or any attribute whose path
/// ends in `test`, such as `#[tokio::test]`.
pub(crate) fn is_test_function(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "test")
}

fn requires(predicate: &Meta) -> bool {
    match predicate {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) if list.path.is_ident("all") => arguments(list).iter().any(requires),
        Meta::List(list) if list.path.is_ident("any") => {
            let branches = arguments(list);
            !branches.is_empty() && branches.iter().all(requires)
        }
        _ => false,
    }
}

fn negates(predicate: &Meta) -> bool {
    match predicate {
        Meta::List(list) if list.path.is_ident("not") => arguments(list).iter().any(mentions_test),
        Meta::List(list) => arguments(list).iter().any(negates),
        _ => false,
    }
}

fn mentions_test(predicate: &Meta) -> bool {
    match predicate {
        Meta::Path(path) => path.is_ident("test"),
        Meta::List(list) => arguments(list).iter().any(mentions_test),
        Meta::NameValue(_) => false,
    }
}

/// The comma-separated predicates inside `all(…)`, `any(…)`, or `not(…)`.
fn arguments(list: &MetaList) -> Vec<Meta> {
    list.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map_or_default(|arguments| arguments.into_iter().collect())
}

#[cfg(test)]
#[path = "cfg_tests.rs"]
mod tests;
