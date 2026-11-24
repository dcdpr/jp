use pretty_assertions::assert_eq;

use super::*;

fn review(query: &str) -> ReviewView {
    match Route::parse("/reviews/R-1h4k2m9", query) {
        Route::Review(view) => view,
        other => panic!("expected a review, got {other:?}"),
    }
}

#[test]
fn the_root_is_the_list_and_anything_else_unknown() {
    assert_eq!(Route::parse("/", ""), Route::List);
    assert_eq!(Route::parse("", ""), Route::List);
    assert_eq!(
        Route::parse("/conversations", ""),
        Route::Unknown("/conversations".to_owned())
    );
    assert_eq!(
        Route::parse("/reviews/R-1/x", ""),
        Route::Unknown("/reviews/R-1/x".to_owned())
    );
}

#[test]
fn a_review_opens_on_its_first_file_in_its_latest_version() {
    assert_eq!(review(""), ReviewView::of("R-1h4k2m9"));
    assert_eq!(
        Route::parse("/reviews/R-1h4k2m9/", ""),
        Route::Review(ReviewView::of("R-1h4k2m9"))
    );
}

#[test]
fn a_file_and_version_are_read_from_the_query() {
    assert_eq!(
        review("file=docs%2Fa%20b.md&version=3&resolved=1"),
        ReviewView {
            id: "R-1h4k2m9".to_owned(),
            file: Some("docs/a b.md".to_owned()),
            version: Some(3),
            changes: None,
            resolved: true,
        }
    );
}

#[test]
fn a_changes_view_names_a_pass_or_two_versions() {
    assert_eq!(
        review("pass=2&diff=unified").changes,
        Some(Changes {
            span: Span::Pass(2),
            unified: true,
        })
    );
    assert_eq!(
        review("from=1&to=3").changes,
        Some(Changes {
            span: Span::Versions {
                from: Some(1),
                to: Some(3),
            },
            unified: false,
        })
    );
    assert_eq!(
        review("changes=1").changes,
        Some(Changes {
            span: Span::Versions {
                from: None,
                to: None,
            },
            unified: false,
        })
    );
    assert_eq!(review("version=x").version, None, "not a number");
}

#[test]
fn a_route_writes_the_location_it_reads_back() {
    let routes = [
        Route::List,
        Route::Review(ReviewView::of("R-1h4k2m9")),
        Route::Review(ReviewView {
            id: "R-1h4k2m9".to_owned(),
            file: Some("docs/a b&c.md".to_owned()),
            version: Some(2),
            changes: Some(Changes {
                span: Span::Versions {
                    from: Some(1),
                    to: None,
                },
                unified: true,
            }),
            resolved: true,
        }),
        Route::Review(ReviewView {
            changes: Some(Changes {
                span: Span::Versions {
                    from: None,
                    to: None,
                },
                unified: false,
            }),
            ..ReviewView::of("R-1h4k2m9")
        }),
    ];

    for route in routes {
        let href = route.href();
        let (path, query) = href.split_once('?').unwrap_or((&href, ""));
        assert_eq!(Route::parse(path, query), route, "{href}");
    }
}

#[test]
fn a_location_encodes_what_a_query_would_misread() {
    let route = Route::Review(ReviewView {
        file: Some("docs/a b&c.md".to_owned()),
        changes: Some(Changes {
            span: Span::Pass(2),
            unified: false,
        }),
        ..ReviewView::of("R-1h4k2m9")
    });

    assert_eq!(
        route.href(),
        "/reviews/R-1h4k2m9?file=docs/a%20b%26c.md&pass=2"
    );
}

#[test]
fn a_view_says_what_it_shows_of_its_file() {
    assert_eq!(review("").showing(), Showing::Latest);
    assert_eq!(review("version=2").showing(), Showing::Version(2));
    assert_eq!(review("version=2&pass=3").showing(), Showing::Pass(3));
    assert_eq!(review("from=1&to=3").showing(), Showing::Compare);
    assert_eq!(review("changes=1").showing(), Showing::Compare);
}

#[test]
fn showing_a_version_leaves_the_changes_view() {
    let changes = review("file=docs/a.md&pass=2&diff=unified&resolved=1");

    assert_eq!(
        changes.show(Showing::Version(1)),
        review("file=docs/a.md&version=1&resolved=1")
    );
    assert_eq!(
        review("file=docs/a.md&version=1").show(Showing::Latest),
        review("file=docs/a.md")
    );
}

#[test]
fn showing_changes_keeps_the_version_and_how_changes_are_shown() {
    let unified = review("version=2&pass=1&diff=unified");

    assert_eq!(
        review("version=2").show(Showing::Pass(3)),
        review("version=2&pass=3")
    );
    assert_eq!(
        unified.show(Showing::Pass(3)),
        review("version=2&pass=3&diff=unified")
    );
}

/// The server compares with the version before `to`, and with the latest when
/// `to` is absent too.
#[test]
fn comparing_starts_at_the_version_shown() {
    assert_eq!(
        review("version=2").show(Showing::Compare),
        review("version=2&to=2")
    );
    assert_eq!(review("").show(Showing::Compare), review("changes=1"));
}

#[test]
fn an_option_of_the_picker_reads_back_as_what_it_shows() {
    let all = [
        Showing::Latest,
        Showing::Version(3),
        Showing::Pass(12),
        Showing::Compare,
    ];

    for showing in all {
        assert_eq!(Showing::parse(&showing.value()), Some(showing));
    }
    assert_eq!(Showing::Version(3).value(), "v3");
    assert_eq!(Showing::Pass(12).value(), "p12");
    assert_eq!(Showing::parse(""), None);
    assert_eq!(Showing::parse("v"), None);
    assert_eq!(Showing::parse("x3"), None);
    assert_eq!(Showing::parse("é1"), None, "not split inside a character");
}

#[test]
fn a_broken_escape_is_kept_as_written() {
    assert_eq!(review("file=50%25%zz").file.as_deref(), Some("50%%zz"));
}
