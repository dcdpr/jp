use camino::Utf8PathBuf;
use chrono::TimeZone as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::{
    app::Update,
    model::{JpRun, JpSession, Terminal, Worktree},
    source::jp::JpState,
};

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

fn press(app: &mut App, code: KeyCode) {
    app.handle(KeyEvent::new(code, KeyModifiers::NONE));
}

fn worktree(path: &str, branch: &str) -> Worktree {
    Worktree {
        path: Utf8PathBuf::from(path),
        branch: Some(branch.to_owned()),
    }
}

fn terminal(id: &str, cwd: &str) -> Terminal {
    named(id, cwd, "fish")
}

fn named(id: &str, cwd: &str, name: &str) -> Terminal {
    Terminal {
        id: id.to_owned(),
        cwd: Utf8PathBuf::from(cwd),
        name: name.to_owned(),
        tty: None,
    }
}

fn pr(number: u64, head: &str, checks: Checks, threads: u32) -> PullRequest {
    PullRequest {
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/dcdpr/jp/pull/{number}"),
        head: head.to_owned(),
        draft: false,
        checks,
        threads,
        answered: threads,
    }
}

const PASSED: Checks = Checks {
    passed: 14,
    pending: 0,
    failed: 0,
};

fn at(hour: u32, minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, hour, minute, 0).unwrap()
}

/// The screen's text, one string per row, without trailing blanks.
fn render(app: &App, width: u16, height: u16) -> Vec<String> {
    screen(app, width, height)
        .iter()
        .map(|line| line.text().trim_end().to_owned())
        .collect()
}

/// A worktree with two tabs and a `jp` waiting on the user in one of them, and
/// one with a pull request.
fn queue_and_review() -> App {
    let mut app = App::default();
    app.now = at(10, 0);
    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/gh-merge-queue", "gh-merge-queue"),
        worktree("/repo/review", "fix-replies"),
        worktree("/repo/idle", "idle"),
    ])));
    app.apply(Update::Terminals(Ok(vec![
        named("T1", "/repo/gh-merge-queue", "fish"),
        named("T2", "/repo/gh-merge-queue/crates", "jp query"),
    ])));
    app.apply(Update::Jp(Ok(JpState {
        runs: vec![JpRun {
            pid: 4242,
            cwd: Utf8PathBuf::from("/repo/gh-merge-queue/crates"),
            conversation: "Merge queue".to_owned(),
            waiting_since: Some(at(9, 58)),
        }],
        sessions: vec![JpSession {
            pid: 10,
            shell: 11,
            ghostty: true,
            tty: None,
            cwd: Utf8PathBuf::from("/repo/gh-merge-queue/crates"),
            conversations: vec![
                SessionConversation {
                    id: "jp-c17901234567".to_owned(),
                    title: Some("Merge queue".to_owned()),
                    active: true,
                    pinned: false,
                    events: 128,
                    last_event_at: Some(at(9, 58)),
                    status: Status::Waiting { since: at(9, 58) },
                },
                SessionConversation {
                    id: "jp-c17900000000".to_owned(),
                    title: Some("Dash sorting".to_owned()),
                    active: false,
                    pinned: true,
                    events: 7,
                    last_event_at: Some(at(7, 0)),
                    status: Status::Idle,
                },
            ],
            more: 3,
        }],
    })));
    app.apply(Update::PullRequests(Ok(vec![pr(
        1132,
        "fix-replies",
        PASSED,
        3,
    )])));
    app
}

#[test]
fn test_screen_shows_one_line_per_worktree_until_expanded() {
    let app = queue_and_review();

    assert_eq!(render(&app, 110, 6), vec![
        "dash  2 active of 3 worktrees   enter open · →/space expand · n new · / filter · s sort \
         · o PR · a all · r ref",
        "WORKTREE        BRANCH          JP           CONVERSATION                         PR      \
         CI            REVIEW",
        "gh-merge-queue  gh-merge-queue  waiting 2m   Merge queue",
        "review          fix-replies                                                       #1132   \
         ✓ 14/14       3/3",
        "",
        "",
    ]);
}

#[test]
fn test_screen_lists_an_expanded_rows_tabs_and_their_conversations() {
    let mut app = queue_and_review();

    press(&mut app, KeyCode::Right);

    // Conversations line up in columns under their tab, like `jp c ls`:
    // marker, ID, events, activity, status, title.
    assert_eq!(render(&app, 110, 10)[2..8], [
        "gh-merge-queue  gh-merge-queue  waiting 2m   Merge queue",
        "├ tab 1                                      fish",
        "└ tab 2                                      jp query",
        "    ▶ jp-c17901234567  128   2m ago  waiting 2m   Merge queue",
        "    ★ jp-c17900000000    7   3h ago               Dash sorting",
        "    … 3 older",
    ]);
}

#[test]
fn test_screen_lists_a_conversations_operations_under_its_id() {
    let mut app = queue_and_review();
    press(&mut app, KeyCode::Right);
    for _ in 0..3 {
        press(&mut app, KeyCode::Down);
    }
    press(&mut app, KeyCode::Right);

    // Each operation shows the command it runs in the tab.
    assert_eq!(render(&app, 110, 12)[5..9], [
        "    ▶ jp-c17901234567  128   2m ago  waiting 2m   Merge queue",
        "        start query           jp query --id=jp-c17901234567",
        "        archive conversation  jp conversation archive jp-c17901234567",
        "        delete conversation   jp conversation rm jp-c17901234567",
    ]);
}

#[test]
fn test_conversation_titles_hold_still_as_ages_tick() {
    let mut app = queue_and_review();
    press(&mut app, KeyCode::Right);

    // Where the active conversation's title starts, `seconds` after its last
    // event, with `status`.
    let mut title_column = |seconds: i64, status: Status| {
        let mut jp = app.jp.data.clone().unwrap();
        let conversation = &mut jp.sessions[0].conversations[0];
        conversation.last_event_at = Some(at(10, 0) - chrono::Duration::seconds(seconds));
        conversation.status = status;
        app.apply(Update::Jp(Ok(jp)));

        let line = &render(&app, 110, 10)[5];
        line[..line.find("Merge queue").unwrap()].chars().count()
    };

    let column = title_column(1, Status::Idle);
    assert_eq!(title_column(10, Status::Idle), column);
    assert_eq!(title_column(59, Status::Idle), column);
    assert_eq!(title_column(60, Status::Idle), column);
    assert_eq!(title_column(59 * 60, Status::Idle), column);
    assert_eq!(title_column(1, Status::Running), column);
    assert_eq!(title_column(1, Status::Waiting { since: at(9, 1) }), column);
}

#[test]
fn test_screen_lists_pull_requests_without_a_worktree_last() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![worktree(
        "/repo/review",
        "fix-replies",
    )])));
    let running = Checks {
        passed: 3,
        pending: 2,
        failed: 0,
    };
    app.apply(Update::PullRequests(Ok(vec![
        pr(1193, "feature/stacks", running, 0),
        pr(1132, "fix-replies", PASSED, 3),
    ])));

    // Terminals and `jp` have not answered, but only the worktree shows `…`
    // for them: a pull request without a worktree has neither.
    assert_eq!(render(&app, 110, 6)[1..4], [
        "WORKTREE         BRANCH          JP           CONVERSATION                        PR      \
         CI            REVIEW",
        "review           fix-replies     …                                                #1132   \
         ✓ 14/14       3/3",
        "create worktree  feature/stacks                                                   #1193   \
         ● 3/5         -",
    ]);
}

#[test]
fn test_screen_shows_the_filter_in_force() {
    let mut app = queue_and_review();
    for c in "/rev".chars() {
        app.handle(key(c));
    }

    let screen = render(&app, 110, 5);
    assert!(
        screen[0].starts_with("dash  filter /rev: 1 of 3   enter open"),
        "{screen:?}"
    );
    assert_eq!(screen[4], "/rev   enter keep · esc cancel");

    press(&mut app, KeyCode::Enter);
    let screen = render(&app, 110, 5);
    assert!(
        screen[0].starts_with("dash  filter /rev: 1 of 3 · esc clears   "),
        "{screen:?}"
    );
}

#[test]
fn test_status_line_says_when_a_filter_matches_nothing() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![worktree("/repo/a", "a")])));

    for c in "/zz".chars() {
        app.handle(key(c));
    }

    assert_eq!(
        render(&app, 80, 5)[4],
        "/zz  no match   enter keep · esc cancel"
    );
}

#[test]
fn test_status_line_asks_for_a_new_worktrees_branch() {
    let mut app = queue_and_review();
    for c in "nfix-queue".chars() {
        app.handle(key(c));
    }

    assert_eq!(
        render(&app, 110, 5)[4],
        "new worktree from main: fix-queue   enter create · esc cancel"
    );
}

#[test]
fn test_status_line_reports_a_failed_update_of_main() {
    let mut app = App::default();
    app.apply(Update::Base(Err(
        "`git` failed: not a fast-forward".to_owned()
    )));

    assert_eq!(
        render(&app, 80, 5)[4],
        "updating main: `git` failed: not a fast-forward"
    );
}

#[test]
fn test_screen_draws_the_sort_menu_over_the_table() {
    let mut app = queue_and_review();
    app.handle(key('s'));
    press(&mut app, KeyCode::Down);

    let lines = screen(&app, 60, 12);
    let text: Vec<String> = lines
        .iter()
        .map(|line| line.text().trim_end().to_owned())
        .collect();

    assert_eq!(text[2..10], [
        "              ╭ Sort by ─────────────────────╮",
        "              │ 1 Worktree name ✓            │",
        "              │ 2 Branch name                │",
        "              │ 3 Pull request, newest first │",
        "              │ 4 Recent JP activity         │",
        "              │ 5 CI, failing first          │",
        "              │ enter sort · esc close       │",
        "              ╰──────────────────────────────╯",
    ]);
    assert!(lines[4].spans.iter().any(|span| span.style.reversed));
    assert!(!lines[3].spans.iter().any(|span| span.style.reversed));

    // Clicks do not reach the table under the menu.
    assert_eq!(hit(&app, 60, 12, 20, 4), None);
}

#[test]
fn test_header_names_a_sort_other_than_the_default() {
    let mut app = queue_and_review();
    app.sort = Sort::Checks;

    let screen = render(&app, 120, 5);
    assert!(
        screen[0].starts_with("dash  2 active of 3 worktrees · by ci, failing first   "),
        "{screen:?}"
    );
}

#[test]
fn test_screen_links_only_the_pr_number() {
    let app = queue_and_review();

    let linked: Vec<(String, String)> = screen(&app, 110, 5)[3]
        .spans
        .iter()
        .filter_map(|span| Some((span.text.clone(), span.link.clone()?)))
        .collect();

    assert_eq!(linked, [(
        "#1132".to_owned(),
        "https://github.com/dcdpr/jp/pull/1132".to_owned()
    )]);
}

#[test]
fn test_hit_finds_the_line_and_the_cell() {
    let app = queue_and_review();

    // Screen row 3 is the second table line, `review`.
    let text = &render(&app, 110, 8)[3];
    let column =
        |needle: &str| u16::try_from(text[..text.find(needle).unwrap()].chars().count()).unwrap();

    assert_eq!(hit(&app, 110, 8, 0, 2), Some((0, Target::Worktree)));
    assert_eq!(hit(&app, 110, 8, 2, 3), Some((1, Target::Worktree)));
    assert_eq!(
        hit(&app, 110, 8, column("fix-replies"), 3),
        Some((1, Target::Other))
    );
    assert_eq!(
        hit(&app, 110, 8, column("#1132"), 3),
        Some((1, Target::PullRequest))
    );
    assert_eq!(hit(&app, 110, 8, column("✓"), 3), Some((1, Target::Checks)));
    assert_eq!(
        hit(&app, 110, 8, column("3/3"), 3),
        Some((1, Target::Reviews))
    );

    // The header, the headings, rows past the table, and the status line.
    assert_eq!(hit(&app, 110, 8, 0, 0), None);
    assert_eq!(hit(&app, 110, 8, 0, 1), None);
    assert_eq!(hit(&app, 110, 8, 0, 4), None);
    assert_eq!(hit(&app, 110, 8, 0, 7), None);
}

#[test]
fn test_screen_marks_sources_without_data() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![worktree("/repo/a", "a")])));
    app.apply(Update::Jp(Err("no workspace".to_owned())));
    app.handle(key('a'));

    // JP failed (`?`); GitHub has not answered yet (`…`).
    assert_eq!(render(&app, 80, 5)[1..], [
        "WORKTREE  BRANCH  JP           CONVERSATION         PR      CI            REVIEW",
        "a         a       ?                                 …       …             …",
        "",
        "jp: no workspace",
    ]);
}

#[test]
fn test_screen_reverses_only_the_selected_row() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/a", "a"),
        worktree("/repo/b", "b"),
    ])));
    app.apply(Update::Terminals(Ok(vec![
        terminal("TA", "/repo/a"),
        terminal("TB", "/repo/b"),
    ])));
    press(&mut app, KeyCode::Down);

    let lines = screen(&app, 60, 5);

    assert!(lines[2].spans.iter().all(|span| !span.style.reversed));
    assert!(lines[3].spans.iter().all(|span| span.style.reversed));
    // The highlight spans the whole row, padding included.
    assert_eq!(lines[3].text().chars().count(), 60);
}

#[test]
fn test_screen_shows_rows_from_the_scroll_offset() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/a", "a"),
        worktree("/repo/b", "b"),
        worktree("/repo/c", "c"),
    ])));
    app.handle(key('a'));
    app.scroll = 1;

    let names: Vec<String> = render(&app, 40, 5)[2..4]
        .iter()
        .map(|row| row.split_whitespace().next().unwrap_or("").to_owned())
        .collect();

    assert_eq!(names, ["b", "c"]);
}

#[test]
fn test_age() {
    let now = at(10, 0);
    let before = |seconds| now - chrono::Duration::seconds(seconds);

    assert_eq!(age(now, before(12)), "12s");
    assert_eq!(age(now, before(300)), "5m");
    assert_eq!(age(now, before(3 * 3_600)), "3h");
    assert_eq!(age(now, before(2 * 86_400)), "2d");
    // A clock that ran ahead does not show a negative age.
    assert_eq!(age(now, before(-30)), "0s");
}

#[test]
fn test_fit_cuts_by_display_width_and_pads() {
    let mut line = Line::default();
    line.push("日本語", PLAIN);

    // Each character takes two columns; the third does not fit in five.
    assert_eq!(line.clone().fit(5).text(), "日本 ");
    assert_eq!(line.fit(8).text(), "日本語  ");
}
