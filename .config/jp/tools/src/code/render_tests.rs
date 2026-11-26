use std::rc::Rc;

use pretty_assertions::assert_eq;

use super::{super::tests::fixture, *};

fn stream_source() -> Rc<[String]> {
    fixture::stream_lines().into()
}

fn group(label: &str, start: usize, end: usize, lines: &[usize], test: bool) -> Group {
    Group {
        item: Some(GroupItem {
            label: label.to_owned(),
            start,
            end,
        }),
        lines: lines.to_vec(),
        ends: lines.to_vec(),
        matches: vec![],
        count: lines.len(),
        test,
    }
}

fn iter_header() -> Header {
    Header {
        qualified: "fixture::stream::Stream::iter".to_owned(),
        location: "src/stream.rs:15".to_owned(),
        signature: vec!["pub fn iter(&self) -> impl Iterator<Item = &u32>".to_owned()],
        doc: vec!["Iterate over the items.".to_owned()],
    }
}

const OPTIONS: Options = Options {
    budget: 200,
    tests: false,
};

#[test]
fn context_runs_to_the_end_of_the_statement_a_reference_starts() {
    let source: Rc<[String]> = [
        "fn build() -> Event {",
        "    let a = 1;",
        "    let b = 2;",
        "    let c = 3;",
        "    Event::now(Request {",
        "        id,",
        "        name,",
        "        arguments,",
        "        metadata,",
        "    })",
        "}",
    ]
    .iter()
    .map(|line| (*line).to_owned())
    .collect();
    let mut group = group("fn build", 0, 10, &[4], false);
    group.ends = vec![9];

    assert_eq!(render_group(&source, &group, false), vec![
        "in fn build (1-11):",
        "3-let b = 2;",
        "4-let c = 3;",
        "5:Event::now(Request {",
        "6-    id,",
        "7-    name,",
        "8-    arguments,",
        "9-    metadata,",
        "10-})",
    ]);
}

#[test]
fn renders_a_header_definition_and_references() {
    let source = stream_source();
    let report = Report {
        header: iter_header(),
        definition: Some(Snippet {
            path: "src/stream.rs".to_owned(),
            start: 13,
            lines: source[13..=16].to_vec(),
        }),
        implementations: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(Hits {
            total: 3,
            files: 1,
            outside_tests: Tally { count: 2, files: 1 },
            in_tests: Tally { count: 1, files: 1 },
            shown: vec![FileHits {
                path: "src/stream.rs".to_owned(),
                source,
                groups: vec![
                    group("fn <Stream as Render>::render", 20, 23, &[21], false),
                    group("fn total", 26, 28, &[27], false),
                    group("fn tests::counts", 35, 38, &[37], true),
                ],
            }],
            ..Hits::default()
        }),
    };

    assert_eq!(
        render(&report, OPTIONS),
        "\
fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

Definition (src/stream.rs:14-17):
14-/// Iterate over the items.
15-pub fn iter(&self) -> impl Iterator<Item = &u32> {
16-    self.items.iter()
17-}

References: 3 in 1 file: 2 in 1 file outside tests, 1 in 1 file in tests.
src/stream.rs (2)
in fn <Stream as Render>::render (21-24):
21-fn render(&self) -> String {
22:    let total: u32 = self.iter().sum();
23-    format!(\"{total} {}\", self.config)
24-}
in fn total (27-29):
27-pub fn total(stream: &Stream) -> u32 {
28:    stream.iter().sum()
29-}
In tests, 1 in 1 file. Pass `tests: true` to show them:
src/stream.rs (1)"
    );
}

#[test]
fn test_references_are_shown_on_request() {
    let report = Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(Hits {
            total: 2,
            files: 1,
            outside_tests: Tally { count: 1, files: 1 },
            in_tests: Tally { count: 1, files: 1 },
            shown: vec![FileHits {
                path: "src/stream.rs".to_owned(),
                source: stream_source(),
                groups: vec![
                    group("fn total", 26, 28, &[27], false),
                    group("fn tests::counts", 35, 38, &[37], true),
                ],
            }],
            ..Hits::default()
        }),
    };

    let rendered = render(&report, Options {
        budget: 200,
        tests: true,
    });

    assert_eq!(
        rendered,
        "\
fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

References: 2 in 1 file: 1 in 1 file outside tests, 1 in 1 file in tests.
src/stream.rs (1)
in fn total (27-29):
27-pub fn total(stream: &Stream) -> u32 {
28:    stream.iter().sum()
29-}
In tests:
src/stream.rs (1)
in fn tests::counts (36-39):
36-fn counts() {
37-    let stream = Stream::default();
38:    assert_eq!(stream.iter().count(), 0);
39-}"
    );
}

fn long_file() -> FileHits {
    FileHits {
        path: "src/long.rs".to_owned(),
        source: (1..=40)
            .map(|n| format!("    line {n}"))
            .collect::<Vec<_>>()
            .into(),
        groups: vec![group("fn long", 0, 39, &[5, 7, 30], false)],
    }
}

fn references_only(file: FileHits) -> Report {
    Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(Hits {
            total: 3,
            files: 1,
            shown: vec![file],
            ..Hits::default()
        }),
    }
}

fn after_header(rendered: &str) -> &str {
    rendered.split_once("\n\n").unwrap().1
}

fn lines(text: &[&str]) -> Rc<[String]> {
    text.iter().map(|line| (*line).to_owned()).collect()
}

#[test]
fn a_line_two_items_would_show_is_shown_once() {
    // `Stream` in the `impl` header and twice in `new` inside it: the impl block
    // shown whole already holds every line of `new`.
    let file = FileHits {
        path: "src/a.rs".to_owned(),
        source: lines(&[
            "impl Stream {",
            "    fn new() -> Stream {",
            "        Stream {}",
            "    }",
            "}",
        ]),
        groups: vec![
            group("impl Stream", 0, 4, &[0], false),
            group("fn Stream::new", 1, 3, &[1, 2], false),
        ],
    };

    assert_eq!(
        after_header(&render(&references_only(file), OPTIONS)),
        "\
References: 3 in 1 file.
src/a.rs (3)
in impl Stream (1-5):
1:impl Stream {
2:    fn new() -> Stream {
3:        Stream {}
4-    }
5-}"
    );
}

#[test]
fn lines_matching_the_pattern_are_marked_and_shown() {
    let mut matched = group("fn render", 0, 9, &[1], false);
    matched.matches = vec![8];
    let file = FileHits {
        path: "src/a.rs".to_owned(),
        source: (0..10)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .into(),
        groups: vec![matched],
    };
    let mut report = references_only(file);
    if let Some(hits) = &mut report.references {
        hits.pattern = Some("config".to_owned());
    }

    // A budget that fits each line alone, but not the context around them.
    let rendered = render(&report, Options {
        budget: 3,
        tests: false,
    });
    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 1 file. 1 in 1 item where `config` also matches.
src/a.rs (1)
2:line 1
9=line 8"
    );

    // One line short of the whole item: the context around each marked line.
    let rendered = render(&report, Options {
        budget: 11,
        tests: false,
    });
    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 1 file. 1 in 1 item where `config` also matches.
src/a.rs (1)
in fn render (1-10):
1-line 0
2:line 1
3-line 2
4-line 3
--
7-line 6
8-line 7
9=line 8
10-line 9"
    );
}

fn calls_only(callers: Option<Callers>, calls: Option<Calls>) -> Report {
    Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        references: None,
        callers,
        calls,
        members: None,
    }
}

fn caller(label: &str, location: &str, calls: usize, level: usize) -> Caller {
    Caller {
        label: label.to_owned(),
        location: location.to_owned(),
        calls,
        level,
        repeated: false,
    }
}

#[test]
fn callers_are_a_tree_with_tests_counted() {
    let mut repeated = caller("fn middle", "src/lib.rs:20", 1, 1);
    repeated.repeated = true;
    let callers = Callers {
        note: None,
        depth: 2,
        shown: vec![
            caller("fn middle", "src/lib.rs:6", 2, 0),
            caller("fn top", "src/lib.rs:10", 1, 1),
            caller("fn other", "src/lib.rs:14", 1, 0),
            repeated,
        ],
        hidden_tests: 1,
        stopped: false,
    };

    assert_eq!(
        after_header(&render(&calls_only(Some(callers), None), OPTIONS)),
        "\
Callers, 2 levels up:
fn middle (src/lib.rs:6), 2 calls
  fn top (src/lib.rs:10)
fn other (src/lib.rs:14)
  fn middle (src/lib.rs:20), its callers are listed above
In tests, 1 test function. Pass `tests: true` to show them."
    );
}

#[test]
fn a_symbol_without_callers_says_so() {
    let none = Callers {
        depth: 2,
        ..Callers::default()
    };
    let not_a_function = Callers {
        note: Some("the symbol is not a function or method.".to_owned()),
        ..Callers::default()
    };

    assert_eq!(
        after_header(&render(&calls_only(Some(none), None), OPTIONS)),
        "Callers: none in the workspace."
    );
    assert_eq!(
        after_header(&render(&calls_only(Some(not_a_function), None), OPTIONS)),
        "Callers: the symbol is not a function or method."
    );
}

#[test]
fn calls_list_workspace_functions_and_name_the_rest() {
    let calls = Calls {
        note: None,
        inside: vec![Callee {
            label: "fn middle".to_owned(),
            location: "src/lib.rs:5".to_owned(),
            lines: vec![10, 12],
        }],
        outside: vec!["map".to_owned(), "unwrap_or".to_owned()],
        named: vec!["struct Pair".to_owned(), "variant Shape::Circle".to_owned()],
    };

    assert_eq!(
        after_header(&render(&calls_only(None, Some(calls)), OPTIONS)),
        "\
Calls: 3, 2 outside the workspace.
fn middle (src/lib.rs:5), at 10, 12
Outside the workspace: map, unwrap_or
Types and variants it names: struct Pair, variant Shape::Circle"
    );
}

#[test]
fn a_function_that_only_constructs_has_no_calls() {
    let calls = Calls {
        named: vec!["struct Pair".to_owned()],
        ..Calls::default()
    };

    assert_eq!(
        after_header(&render(&calls_only(None, Some(calls)), OPTIONS)),
        "\
Calls: none.
Types and variants it names: struct Pair"
    );
}

#[test]
fn a_long_item_is_shown_whole_when_the_budget_allows() {
    let rendered = render(&references_only(long_file()), OPTIONS);
    let shown = after_header(&rendered).lines().collect::<Vec<_>>();

    // The summary, the path, the item label, then all 40 lines of the item.
    assert_eq!(shown.len(), 43);
    assert_eq!(shown[1..4], [
        "src/long.rs (3)",
        "in fn long (1-40):",
        "1-line 1"
    ]);
    assert_eq!(shown[42], "40-line 40");
    assert!(!shown.contains(&"--"));
}

#[test]
fn a_long_item_shows_context_around_each_reference_when_the_budget_is_short() {
    // 15 lines fit the context around each reference, not the 42 of the
    // whole item.
    let rendered = render(&references_only(long_file()), Options {
        budget: 20,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 1 file.
src/long.rs (3)
in fn long (1-40):
4-line 4
5-line 5
6:line 6
7-line 7
8:line 8
9-line 9
10-line 10
--
29-line 29
30-line 30
31:line 31
32-line 32
33-line 33"
    );
}

fn numbered_source(count: usize) -> Rc<[String]> {
    (1..=count)
        .map(|n| format!("    line {n}"))
        .collect::<Vec<_>>()
        .into()
}

fn references_in(shown: Vec<FileHits>) -> Report {
    let count = shown
        .iter()
        .flat_map(|file| &file.groups)
        .map(|group| group.count)
        .sum();
    Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(Hits {
            total: count,
            files: shown.len(),
            shown,
            ..Hits::default()
        }),
    }
}

#[test]
fn the_smallest_items_are_shown_whole_first() {
    // Context costs 13 lines: the path, then a label and 5 lines per item.
    // Showing `fn small` whole adds 5 and `fn big` whole adds 15, so a budget
    // of 20 takes the first and not the second.
    let file = FileHits {
        path: "src/two.rs".to_owned(),
        source: numbered_source(30),
        groups: vec![
            group("fn small", 0, 9, &[5], false),
            group("fn big", 10, 29, &[20], false),
        ],
    };

    let rendered = render(&references_in(vec![file]), Options {
        budget: 20,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 2 in 1 file.
src/two.rs (2)
in fn small (1-10):
1-line 1
2-line 2
3-line 3
4-line 4
5-line 5
6:line 6
7-line 7
8-line 8
9-line 9
10-line 10
in fn big (11-30):
19-line 19
20-line 20
21:line 21
22-line 22
23-line 23"
    );
}

#[test]
fn items_outside_tests_are_shown_whole_before_test_items() {
    // Context costs 15 lines, the `In tests:` heading included. The budget
    // leaves 16, enough to show either item whole but not both. Smallest
    // first alone would pick the 5-line test item.
    let source = numbered_source(30);
    let outside = FileHits {
        path: "src/a.rs".to_owned(),
        source: source.clone(),
        groups: vec![group("fn big", 10, 29, &[20], false)],
    };
    let test = FileHits {
        path: "src/b.rs".to_owned(),
        source,
        groups: vec![group("fn tests::small", 0, 9, &[5], true)],
    };

    let rendered = render(&references_in(vec![outside, test]), Options {
        budget: 31,
        tests: true,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 2 in 2 files.
src/a.rs (1)
in fn big (11-30):
11-line 11
12-line 12
13-line 13
14-line 14
15-line 15
16-line 16
17-line 17
18-line 18
19-line 19
20-line 20
21:line 21
22-line 22
23-line 23
24-line 24
25-line 25
26-line 26
27-line 27
28-line 28
29-line 29
30-line 30
In tests:
src/b.rs (1)
in fn tests::small (1-10):
4-line 4
5-line 5
6:line 6
7-line 7
8-line 8"
    );
}

fn one_reference_in(path: &str) -> FileHits {
    FileHits {
        path: path.to_owned(),
        source: numbered_source(30),
        groups: vec![group("fn big", 0, 29, &[20], false)],
    }
}

#[test]
fn every_reference_gets_its_line_before_any_item_is_expanded() {
    // Each file costs 2 lines as its referencing line alone and 7 with the
    // context around it. The budget of 15 fits every file's line plus the
    // context of one: expanding files in order instead would show `a.rs` and
    // `b.rs` with context and drop `c.rs`.
    let report = references_in(vec![
        one_reference_in("src/a.rs"),
        one_reference_in("src/b.rs"),
        one_reference_in("src/c.rs"),
    ]);

    let rendered = render(&report, Options {
        budget: 15,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 3 files.
src/a.rs (1)
in fn big (1-30):
19-line 19
20-line 20
21:line 21
22-line 22
23-line 23
src/b.rs (1)
21:line 21
src/c.rs (1)
21:line 21"
    );
}

#[test]
fn files_with_the_fewest_references_are_kept_when_their_lines_alone_overflow() {
    // The lines alone take 4 for `long.rs` and 2 each for the others. A budget
    // of 5 fits the two small files, not `long.rs` first and then nothing.
    let report = references_in(vec![
        long_file(),
        one_reference_in("src/b.rs"),
        one_reference_in("src/c.rs"),
    ]);

    let rendered = render(&report, Options {
        budget: 5,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 5 in 3 files.
src/b.rs (1)
21:line 21
src/c.rs (1)
21:line 21
Not shown, over the output budget:
src/long.rs (3)"
    );
}

#[test]
fn the_budget_is_shared_by_weight_above_a_minimum() {
    // 20 each first, then the remaining 140 by weight; the rounding remainder
    // goes to the heaviest.
    assert_eq!(shares(200, &[34, 2, 24], 20), [100, 24, 76]);
    // Without weights, evenly.
    assert_eq!(shares(200, &[0, 0, 0], 20), [68, 66, 66]);
    // A budget too small for the minimum is split evenly.
    assert_eq!(shares(30, &[5, 0, 0], 20), [10, 10, 10]);
    assert_eq!(shares(200, &[7], 20), [200]);
}

#[test]
fn a_report_weighs_what_it_would_show() {
    let hits = |outside: usize, tests: usize| Hits {
        outside_tests: Tally {
            count: outside,
            files: 1,
        },
        in_tests: Tally {
            count: tests,
            files: 1,
        },
        ..Hits::default()
    };
    let report = Report {
        header: iter_header(),
        definition: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(hits(3, 10)),
        implementations: Some(hits(2, 4)),
    };

    // Implementations in tests are always shown; references in tests only on
    // request.
    assert_eq!(report.weight(false), 3 + 2 + 4);
    assert_eq!(report.weight(true), 3 + 10 + 2 + 4);
}

#[test]
fn a_file_over_the_budget_shows_only_its_referencing_lines() {
    let rendered = render(&references_only(long_file()), Options {
        budget: 5,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 1 file.
src/long.rs (3)
6:line 6
8:line 8
31:line 31"
    );
}

#[test]
fn a_file_that_does_not_fit_at_all_is_listed_by_count() {
    let rendered = render(&references_only(long_file()), Options {
        budget: 3,
        tests: false,
    });

    assert_eq!(
        after_header(&rendered),
        "\
References: 3 in 1 file.
Not shown, over the output budget:
src/long.rs (3)"
    );
}

#[test]
fn the_summary_says_when_no_reference_or_every_reference_is_in_tests() {
    let summary = |outside_tests: Tally, in_tests: Tally| {
        let report = Report {
            header: iter_header(),
            definition: None,
            implementations: None,
            callers: None,
            calls: None,
            members: None,
            references: Some(Hits {
                total: 2,
                files: 1,
                outside_tests,
                in_tests,
                ..Hits::default()
            }),
        };
        let rendered = render(&report, OPTIONS);
        after_header(&rendered).lines().next().unwrap().to_owned()
    };
    let two = Tally { count: 2, files: 1 };

    assert_eq!(
        summary(two, Tally::default()),
        "References: 2 in 1 file, none in tests."
    );
    assert_eq!(
        summary(Tally::default(), two),
        "References: 2 in 1 file, all in tests."
    );
    assert_eq!(
        summary(Tally::default(), Tally::default()),
        "References: 2 in 1 file."
    );
}

#[test]
fn the_summary_accounts_for_every_reference() {
    let report = Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        callers: None,
        calls: None,
        members: None,
        references: Some(Hits {
            total: 5,
            files: 3,
            outside_tests: Tally { count: 3, files: 2 },
            in_tests: Tally { count: 1, files: 1 },
            withheld: 1,
            outside_workspace: 2,
            filtered: true,
            pattern: None,
            shown: vec![FileHits {
                path: "src/stream.rs".to_owned(),
                source: stream_source(),
                groups: vec![group("fn total", 26, 28, &[27], false)],
            }],
        }),
    };

    let rendered = render(&report, OPTIONS);

    assert_eq!(
        after_header(&rendered).lines().next(),
        Some(
            "References: 5 in 3 files: 3 in 2 files outside tests, 1 in 1 file in tests. 1 in 1 \
             file under the given paths. 1 in files this tool may not read. 2 more outside the \
             workspace."
        )
    );
}

fn implementations_only(file: FileHits, references: Option<Hits>) -> Report {
    let count = file.groups.iter().map(|group| group.count).sum();
    Report {
        header: iter_header(),
        definition: None,
        implementations: Some(Hits {
            total: count,
            files: 1,
            shown: vec![file],
            ..Hits::default()
        }),
        references,
        callers: None,
        calls: None,
        members: None,
    }
}

#[test]
fn implementations_are_shown_with_their_bodies() {
    let report = implementations_only(
        FileHits {
            path: "src/stream.rs".to_owned(),
            source: stream_source(),
            groups: vec![group("impl Render for Stream", 19, 24, &[19], false)],
        },
        None,
    );

    assert_eq!(
        after_header(&render(&report, OPTIONS)),
        "\
Implementations: 1 in 1 file.
src/stream.rs (1)
in impl Render for Stream (20-25):
20:impl Render for Stream {
21-    fn render(&self) -> String {
22-        let total: u32 = self.iter().sum();
23-        format!(\"{total} {}\", self.config)
24-    }
25-}"
    );
}

#[test]
fn implementations_take_half_the_budget_when_references_are_shown() {
    // The 40-line item fits a budget of 60 whole, but not half of it.
    let budget = Options {
        budget: 60,
        tests: false,
    };

    let alone = render(&implementations_only(long_file(), None), budget);
    let shared = render(
        &implementations_only(long_file(), Some(Hits::default())),
        budget,
    );

    assert!(!alone.contains("\n--\n"), "{alone}");
    assert!(alone.contains("\n40-line 40"), "{alone}");
    assert!(shared.contains("\n--\n"), "{shared}");
    assert!(!shared.contains("\n40-line 40"), "{shared}");
    assert!(
        shared.ends_with("\n\nReferences: none in the workspace."),
        "{shared}"
    );
}

#[test]
fn implementations_in_tests_are_shown_without_asking() {
    let report = implementations_only(
        FileHits {
            path: "src/stream_tests.rs".to_owned(),
            source: stream_source(),
            groups: vec![group("fn total", 26, 28, &[26], true)],
        },
        None,
    );

    assert_eq!(
        after_header(&render(&report, OPTIONS)),
        "\
Implementations: 1 in 1 file.
In tests:
src/stream_tests.rs (1)
in fn total (27-29):
27:pub fn total(stream: &Stream) -> u32 {
28-    stream.iter().sum()
29-}"
    );
}

fn member(label: &str, outside_tests: usize, in_tests: usize, places: &[&str]) -> Member {
    Member {
        label: label.to_owned(),
        location: "src/style.rs:9".to_owned(),
        outside_tests,
        in_tests,
        withheld: 0,
        places: places.iter().map(|place| (*place).to_owned()).collect(),
    }
}

#[test]
fn members_are_listed_with_their_counts_and_where_they_are_used() {
    let report = Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        references: None,
        callers: None,
        calls: None,
        members: Some(Members {
            note: None,
            members: vec![
                member("field color", 2, 1, &[
                    "fn paint (src/paint.rs:4)",
                    "fn tint (src/paint.rs:9)",
                ]),
                member("field file_link", 0, 3, &[]),
                member("fn unused", 0, 0, &[]),
                Member {
                    withheld: 2,
                    ..member("field hidden", 6, 0, &["a", "b", "c", "d", "e", "f"])
                },
            ],
            omitted: 0,
        }),
    };

    assert_eq!(
        after_header(&render(&report, OPTIONS)),
        "\
Members: 4. References to each, outside tests and in tests:
field color (src/style.rs:9): 2 outside tests, 1 in tests
  in fn paint (src/paint.rs:4)
  in fn tint (src/paint.rs:9)
field file_link (src/style.rs:9): 0 outside tests, 3 in tests
fn unused (src/style.rs:9): no references
field hidden (src/style.rs:9): 6 outside tests, 0 in tests, 2 in files this tool may not read
  in a
  in b
  in c
  in d
  in e
  and 1 more places"
    );
}

#[test]
fn members_say_why_a_symbol_has_none() {
    let report = Report {
        header: iter_header(),
        definition: None,
        implementations: None,
        references: None,
        callers: None,
        calls: None,
        members: Some(Members {
            note: Some("`total` is a fn; only structs, enums and traits have members.".to_owned()),
            ..Members::default()
        }),
    };

    assert_eq!(
        after_header(&render(&report, OPTIONS)),
        "Members: `total` is a fn; only structs, enums and traits have members."
    );
}
