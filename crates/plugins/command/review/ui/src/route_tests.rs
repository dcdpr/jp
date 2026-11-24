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
fn a_broken_escape_is_kept_as_written() {
    assert_eq!(review("file=50%25%zz").file.as_deref(), Some("50%%zz"));
}
