use camino::Utf8PathBuf;
use chrono::TimeZone as _;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::model::{Checks, JpRun, Status};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_keys(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle(key(KeyCode::Char(c)));
    }
}

fn worktree(path: &str) -> Worktree {
    Worktree {
        path: Utf8PathBuf::from(path),
        branch: Some(path.rsplit('/').next().unwrap().to_owned()),
    }
}

fn terminal(id: &str, cwd: &str) -> Terminal {
    Terminal {
        id: id.to_owned(),
        cwd: Utf8PathBuf::from(cwd),
        name: format!("{id} title"),
        tty: None,
    }
}

fn pr(number: u64, head: &str) -> PullRequest {
    PullRequest {
        number,
        title: format!("PR {number}"),
        url: format!("https://github.com/dcdpr/jp/pull/{number}"),
        head: head.to_owned(),
        draft: false,
        checks: Checks::default(),
        threads: 0,
        answered: 0,
    }
}

fn prs(app: &mut App, prs: Vec<PullRequest>) {
    app.apply(Update::PullRequests(Ok(prs)));
}

fn at(minute: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 23, 10, minute, 0).unwrap()
}

fn conversation(id: &str, active: bool, status: Status) -> SessionConversation {
    SessionConversation {
        id: id.to_owned(),
        title: Some(format!("{id} title")),
        active,
        pinned: false,
        events: 4,
        last_event_at: Some(at(0)),
        status,
    }
}

/// A session led by `pid`, whose shell is `pid + 1` inside Ghostty.
fn session(pid: u32, cwd: &str, conversations: Vec<SessionConversation>) -> JpSession {
    JpSession {
        pid,
        shell: pid + 1,
        ghostty: true,
        tty: None,
        cwd: Utf8PathBuf::from(cwd),
        conversations,
        more: 0,
    }
}

fn jp(app: &mut App, runs: Vec<JpRun>, sessions: Vec<JpSession>) {
    app.apply(Update::Jp(Ok(JpState { runs, sessions })));
}

/// Each selectable line as `worktree`, `#number`, `worktree/tab id`,
/// `worktree/tab id/conversation id`, or `worktree/tab id/conversation
/// id/operation`.
fn lines(app: &App) -> Vec<String> {
    app.items()
        .iter()
        .map(|item| match item {
            Item::Row(row) if row.is_worktree() => row.name.clone(),
            Item::Row(row) => format!("#{}", row.pr.as_ref().unwrap().number),
            Item::Tab { row, tab, .. } => format!("{}/{}", row.name, tab.id),
            Item::Conversation {
                row,
                tab,
                conversation,
            } => format!(
                "{}/{}/{}",
                row.name,
                tab.map_or("-", |tab| tab.id.as_str()),
                conversation.id
            ),
            Item::Operation {
                row,
                tab,
                conversation,
                operation,
                ..
            } => format!(
                "{}/{}/{}/{}",
                row.name,
                tab.map_or("-", |tab| tab.id.as_str()),
                conversation.id,
                operation.label()
            ),
        })
        .collect()
}

/// The notes on screen, in order.
fn notes(app: &App) -> Vec<String> {
    app.display()
        .into_iter()
        .filter_map(|line| match line {
            Display::Note { text, .. } => Some(text),
            Display::Item { .. } => None,
        })
        .collect()
}

fn selected(app: &App) -> String {
    lines(app)[app.selected_index().unwrap()].clone()
}

/// Three worktrees: `a` and `c` have a terminal open, `b` has nothing going on.
fn app() -> App {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/a"),
        worktree("/repo/b"),
        worktree("/repo/c"),
    ])));
    app.apply(Update::Terminals(Ok(vec![
        terminal("TA", "/repo/a"),
        terminal("TC", "/repo/c"),
    ])));
    app
}

/// `a` with two tabs, one of them running a session with two conversations; `c`
/// with one tab.
fn two_tabs() -> App {
    let mut app = app();
    app.apply(Update::Terminals(Ok(vec![
        terminal("TA1", "/repo/a"),
        terminal("TA2", "/repo/a/src"),
        terminal("TC", "/repo/c"),
    ])));
    jp(&mut app, vec![], vec![session(10, "/repo/a/src", vec![
        conversation("jp-c1", true, Status::Running),
        conversation("jp-c2", false, Status::Idle),
    ])]);
    app
}

/// Every worktree `w00`..
/// `w19` shown, with a screen of ten table lines.
fn tall_app() -> App {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok((0..20)
        .map(|n| worktree(&format!("/repo/w{n:02}")))
        .collect())));
    app.handle(key(KeyCode::Char('a')));
    app.fit_scroll(10);
    app
}

#[test]
fn test_only_active_worktrees_show_until_all_are_asked_for() {
    let mut app = app();
    assert_eq!(lines(&app), ["a", "c"]);
    assert_eq!(app.total(), 3);

    assert_eq!(app.handle(key(KeyCode::Char('a'))), Action::None);
    assert_eq!(lines(&app), ["a", "b", "c"]);
}

#[test]
fn test_selection_starts_at_the_top_and_wraps_at_the_ends() {
    let mut app = app();
    assert_eq!(app.selected_index(), Some(0));

    app.handle(key(KeyCode::Up));
    assert_eq!(app.selected_index(), Some(1));

    app.handle(key(KeyCode::Char('j')));
    assert_eq!(app.selected_index(), Some(0));

    app.handle(key(KeyCode::Char('k')));
    assert_eq!(app.selected_index(), Some(1));
}

#[test]
fn test_ctrl_a_and_ctrl_d_jump_half_a_screen_and_stop_at_the_ends() {
    let mut app = tall_app();

    app.handle(ctrl('d'));
    assert_eq!(app.selected_index(), Some(5));
    app.handle(ctrl('d'));
    app.handle(ctrl('d'));
    app.handle(ctrl('d'));
    assert_eq!(app.selected_index(), Some(19));

    app.handle(ctrl('a'));
    assert_eq!(app.selected_index(), Some(14));
    app.handle(ctrl('a'));
    app.handle(ctrl('a'));
    app.handle(ctrl('a'));
    assert_eq!(app.selected_index(), Some(0));

    // Plain `a` still toggles the idle worktrees.
    assert!(app.show_all);
}

#[test]
fn test_capital_g_and_gg_go_to_the_bottom_and_top() {
    let mut app = tall_app();

    app.handle(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));
    assert_eq!(app.selected_index(), Some(19));

    // A single `g` waits for the second; another key in between cancels it.
    app.handle(key(KeyCode::Char('g')));
    assert_eq!(app.selected_index(), Some(19));
    app.handle(key(KeyCode::Char('k')));
    app.handle(key(KeyCode::Char('g')));
    assert_eq!(app.selected_index(), Some(18));
    app.handle(key(KeyCode::Char('g')));
    assert_eq!(app.selected_index(), Some(0));
}

#[test]
fn test_wheel_stops_at_the_ends() {
    let mut app = app();

    app.wheel(false);
    assert_eq!(app.selected_index(), Some(0));

    app.wheel(true);
    app.wheel(true);
    app.wheel(true);
    assert_eq!(app.selected_index(), Some(1));
}

#[test]
fn test_rows_are_collapsed_until_expanded() {
    let app = two_tabs();
    assert_eq!(lines(&app), ["a", "c"]);
}

#[test]
fn test_an_expanded_row_lists_its_tabs_and_each_tabs_conversations() {
    let mut app = two_tabs();

    app.handle(key(KeyCode::Right));

    // The session in `/repo/a/src` belongs to the tab in that directory.
    assert_eq!(lines(&app), [
        "a",
        "a/TA1",
        "a/TA2",
        "a/TA2/jp-c1",
        "a/TA2/jp-c2",
        "c"
    ]);
    assert_eq!(selected(&app), "a");
}

#[test]
fn test_arrows_descend_into_and_climb_out_of_a_row() {
    let mut app = two_tabs();

    // The first `→` expands, the second steps in.
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    assert_eq!(selected(&app), "a/TA1");

    // Up and down move over the children like any other line.
    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2/jp-c1");

    // `←` climbs to the row, and a second one collapses it.
    app.handle(key(KeyCode::Left));
    assert_eq!(selected(&app), "a");
    app.handle(key(KeyCode::Char('h')));
    assert_eq!(lines(&app), ["a", "c"]);
}

#[test]
fn test_space_toggles_the_row_from_any_of_its_lines() {
    let mut app = two_tabs();

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(lines(&app).len(), 6);

    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(lines(&app), ["a", "c"]);
    assert_eq!(selected(&app), "a");
}

#[test]
fn test_plus_and_minus_expand_and_collapse_every_row() {
    let mut app = two_tabs();

    app.handle(key(KeyCode::Char('+')));
    assert_eq!(lines(&app), [
        "a",
        "a/TA1",
        "a/TA2",
        "a/TA2/jp-c1",
        "a/TA2/jp-c2",
        "c",
        "c/TC"
    ]);

    app.handle(key(KeyCode::Char('-')));
    assert_eq!(lines(&app), ["a", "c"]);
}

#[test]
fn test_an_expanded_row_notes_its_pull_request_and_missing_tabs() {
    let mut app = app();
    app.handle(key(KeyCode::Char('a')));
    prs(&mut app, vec![pr(7, "b")]);
    app.handle(key(KeyCode::Down));

    app.handle(key(KeyCode::Right));

    // Notes explain; they are not lines to select.
    assert_eq!(notes(&app), [
        "#7 PR 7",
        "No tab is open here: enter opens one."
    ]);
    assert_eq!(lines(&app), ["a", "b", "c"]);
}

#[test]
fn test_a_session_no_tab_matches_is_listed_under_the_row() {
    let mut app = app();
    jp(&mut app, vec![], vec![JpSession {
        more: 3,
        ..session(10, "/repo/a/elsewhere", vec![conversation(
            "jp-c1",
            true,
            Status::Idle,
        )])
    }]);

    app.handle(key(KeyCode::Right));

    assert_eq!(lines(&app), ["a", "a/TA", "a/-/jp-c1", "c"]);
    assert_eq!(notes(&app), [
        "In shells no tab could be found for:",
        "… 3 older"
    ]);
}

/// `two_tabs`, with `a` expanded and `jp-c2` selected.
fn on_conversation() -> App {
    let mut app = two_tabs();
    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "jjjj");
    assert_eq!(selected(&app), "a/TA2/jp-c2");
    app
}

#[test]
fn test_right_on_a_conversation_lists_its_operations_then_steps_in() {
    let mut app = on_conversation();

    app.handle(key(KeyCode::Right));
    assert_eq!(lines(&app), [
        "a",
        "a/TA1",
        "a/TA2",
        "a/TA2/jp-c1",
        "a/TA2/jp-c2",
        "a/TA2/jp-c2/start query",
        "a/TA2/jp-c2/archive conversation",
        "a/TA2/jp-c2/delete conversation",
        "c",
    ]);
    assert_eq!(selected(&app), "a/TA2/jp-c2");

    app.handle(key(KeyCode::Right));
    assert_eq!(selected(&app), "a/TA2/jp-c2/start query");

    // Nothing opens below an operation.
    app.handle(key(KeyCode::Right));
    assert_eq!(selected(&app), "a/TA2/jp-c2/start query");
}

#[test]
fn test_left_closes_the_operations_before_climbing_to_the_row() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2/jp-c2/archive conversation");

    // From an operation, back to its conversation, the list closed.
    app.handle(key(KeyCode::Left));
    assert_eq!(selected(&app), "a/TA2/jp-c2");
    assert_eq!(lines(&app).len(), 6);

    // Then to the row, and then the row collapses.
    app.handle(key(KeyCode::Left));
    assert_eq!(selected(&app), "a");
    app.handle(key(KeyCode::Left));
    assert_eq!(lines(&app), ["a", "c"]);
}

#[test]
fn test_left_on_a_conversation_with_its_operations_open_closes_them() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));

    app.handle(key(KeyCode::Left));
    assert_eq!(selected(&app), "a/TA2/jp-c2");
    assert_eq!(lines(&app).len(), 6);
}

#[test]
fn test_space_reopens_a_row_where_it_was_closed() {
    let mut app = on_conversation();

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(lines(&app), ["a", "c"]);
    assert_eq!(selected(&app), "a");

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(selected(&app), "a/TA2/jp-c2");
}

#[test]
fn test_space_reopens_a_row_on_an_operation_with_its_list_still_open() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2/jp-c2/archive conversation");

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(lines(&app), ["a", "c"]);

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(lines(&app).len(), 9);
    assert_eq!(selected(&app), "a/TA2/jp-c2/archive conversation");
}

#[test]
fn test_space_reopens_on_the_row_when_the_line_it_left_is_gone() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Char(' ')));

    // `jp-c2` goes while the row is closed.
    jp(&mut app, vec![], vec![session(10, "/repo/a/src", vec![
        conversation("jp-c1", true, Status::Idle),
    ])]);

    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(selected(&app), "a");
}

#[test]
fn test_left_collapsing_a_row_forgets_where_space_left_off() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Char(' ')));

    // Opened and closed with the arrows, the row starts over on itself.
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Left));
    app.handle(key(KeyCode::Char(' ')));
    assert_eq!(selected(&app), "a");
}

#[test]
fn test_enter_on_an_operation_runs_its_command_in_the_conversations_tab() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));

    assert_eq!(app.handle(key(KeyCode::Enter)), Action::RunInTab {
        terminal: "TA2".to_owned(),
        shell: 11,
        command: "jp query --id=jp-c2".to_owned(),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Running `jp query --id=jp-c2` in its tab.")
    );

    type_keys(&mut app, "jj");
    assert_eq!(app.handle(key(KeyCode::Enter)), Action::RunInTab {
        terminal: "TA2".to_owned(),
        shell: 11,
        command: "jp conversation rm jp-c2".to_owned(),
    });
}

#[test]
fn test_an_operation_runs_in_a_new_tab_when_its_tab_is_not_certain() {
    // Two tabs share the session's directory: which one runs its shell is a
    // guess, and typing into the wrong one could feed another program.
    let mut app = app();
    app.apply(Update::Terminals(Ok(vec![
        terminal("TA1", "/repo/a"),
        terminal("TA2", "/repo/a"),
    ])));
    jp(&mut app, vec![], vec![session(10, "/repo/a", vec![
        conversation("jp-c1", true, Status::Idle),
    ])]);
    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "jj");
    assert_eq!(selected(&app), "a/TA1/jp-c1");
    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "jj");

    assert_eq!(app.handle(key(KeyCode::Enter)), Action::RunInNewTab {
        dir: Utf8PathBuf::from("/repo/a"),
        command: "jp conversation archive jp-c1".to_owned(),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Running `jp conversation archive jp-c1` in a new tab.")
    );
}

#[test]
fn test_an_operation_runs_in_the_tab_its_tty_names_when_tabs_share_a_directory() {
    let mut app = app();
    let on_tty = |id: &str, tty: &str| Terminal {
        tty: Some(tty.to_owned()),
        ..terminal(id, "/repo/a")
    };
    app.apply(Update::Terminals(Ok(vec![
        on_tty("TA1", "ttys001"),
        on_tty("TA2", "ttys002"),
    ])));
    jp(&mut app, vec![], vec![JpSession {
        tty: Some("ttys002".to_owned()),
        ..session(10, "/repo/a", vec![conversation(
            "jp-c1",
            true,
            Status::Idle,
        )])
    }]);
    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "jjj");
    assert_eq!(selected(&app), "a/TA2/jp-c1");

    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "ljj");

    assert_eq!(app.handle(key(KeyCode::Enter)), Action::RunInTab {
        terminal: "TA2".to_owned(),
        shell: 11,
        command: "jp conversation rm jp-c1".to_owned(),
    });
}

#[test]
fn test_an_operation_of_a_session_without_a_tab_runs_in_a_new_tab() {
    let mut app = app();
    jp(&mut app, vec![], vec![session(
        10,
        "/repo/a/elsewhere",
        vec![conversation("jp-c1", true, Status::Idle)],
    )]);
    app.handle(key(KeyCode::Right));
    type_keys(&mut app, "jj");
    assert_eq!(selected(&app), "a/-/jp-c1");

    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    assert_eq!(app.handle(key(KeyCode::Enter)), Action::RunInNewTab {
        dir: Utf8PathBuf::from("/repo/a/elsewhere"),
        command: "jp query --id=jp-c1".to_owned(),
    });
}

#[test]
fn test_clicking_an_operation_runs_it() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));

    // 0 `a`, 1 TA1, 2 TA2, 3 jp-c1, 4 jp-c2, 5-7 its operations.
    assert_eq!(app.click(6, Target::Other), Action::RunInTab {
        terminal: "TA2".to_owned(),
        shell: 11,
        command: "jp conversation archive jp-c2".to_owned(),
    });
    assert_eq!(selected(&app), "a/TA2/jp-c2/archive conversation");
}

#[test]
fn test_an_operation_of_a_conversation_that_goes_falls_back_to_the_row() {
    let mut app = on_conversation();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    assert_eq!(selected(&app), "a/TA2/jp-c2/start query");

    // Deleted from its tab: the conversation and its operations are gone.
    jp(&mut app, vec![], vec![session(10, "/repo/a/src", vec![
        conversation("jp-c1", true, Status::Idle),
    ])]);

    assert_eq!(selected(&app), "a");
}

#[test]
fn test_enter_cycles_through_a_worktrees_tabs() {
    let mut app = two_tabs();

    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA1".to_owned())
    );
    assert_eq!(
        app.notice.as_deref(),
        Some("Tab 1 of 2 in a: enter again for the next.")
    );
    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA2".to_owned())
    );
    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA1".to_owned())
    );

    // Any other key starts the cycle over.
    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Up));
    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA1".to_owned())
    );
}

#[test]
fn test_enter_on_a_tab_or_its_conversation_focuses_the_tab() {
    let mut app = two_tabs();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Down));
    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA2".to_owned())
    );

    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2/jp-c2");
    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::Focus("TA2".to_owned())
    );
}

#[test]
fn test_enter_without_a_terminal_opens_a_tab() {
    let mut app = app();
    app.handle(key(KeyCode::Char('a')));
    app.handle(key(KeyCode::Down));

    assert_eq!(
        app.handle(key(KeyCode::Enter)),
        Action::OpenTab(Utf8PathBuf::from("/repo/b"))
    );
    assert_eq!(app.notice.as_deref(), Some("Opening a tab in b…"));
}

#[test]
fn test_selection_follows_its_line_when_rows_shift() {
    let mut app = two_tabs();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2/jp-c1");

    // A conversation joins above it; the selection stays on `jp-c1`.
    jp(&mut app, vec![], vec![session(10, "/repo/a/src", vec![
        conversation("jp-c0", false, Status::Idle),
        conversation("jp-c1", true, Status::Running),
    ])]);

    assert_eq!(selected(&app), "a/TA2/jp-c1");
}

#[test]
fn test_selection_falls_back_to_the_row_when_its_child_goes() {
    let mut app = two_tabs();
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Down));
    assert_eq!(selected(&app), "a/TA2");

    app.apply(Update::Terminals(Ok(vec![
        terminal("TA1", "/repo/a"),
        terminal("TC", "/repo/c"),
    ])));

    assert_eq!(selected(&app), "a");
}

#[test]
fn test_o_opens_the_pull_request_from_a_row_or_its_children() {
    let mut app = two_tabs();
    prs(&mut app, vec![pr(7, "a")]);

    assert_eq!(
        app.handle(key(KeyCode::Char('o'))),
        Action::Open("https://github.com/dcdpr/jp/pull/7".to_owned())
    );

    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Right));
    assert_eq!(
        app.handle(key(KeyCode::Char('o'))),
        Action::Open("https://github.com/dcdpr/jp/pull/7".to_owned())
    );

    type_keys(&mut app, "G");
    assert_eq!(app.handle(key(KeyCode::Char('o'))), Action::None);
    assert_eq!(app.notice.as_deref(), Some("c has no open pull request."));
}

#[test]
fn test_click_targets() {
    let mut app = two_tabs();
    prs(&mut app, vec![pr(7, "a")]);
    let url = "https://github.com/dcdpr/jp/pull/7";

    assert_eq!(
        app.click(0, Target::PullRequest),
        Action::Open(url.to_owned())
    );
    assert_eq!(
        app.click(0, Target::Checks),
        Action::Open(format!("{url}/checks"))
    );
    assert_eq!(
        app.click(0, Target::Reviews),
        Action::Open(format!("{url}/changes"))
    );
    assert_eq!(app.click(1, Target::PullRequest), Action::None);
    assert_eq!(selected(&app), "c");

    // The worktree cell acts as `enter`.
    assert_eq!(
        app.click(1, Target::Worktree),
        Action::Focus("TC".to_owned())
    );

    // In an expanded row, a tab focuses, and a note selects its owner.
    app.handle(key(KeyCode::Up));
    app.handle(key(KeyCode::Right));
    // 0 `a`, 1 note `#7 PR 7`, 2 TA1, 3 TA2.
    assert_eq!(app.click(1, Target::Other), Action::None);
    assert_eq!(selected(&app), "a");
    assert_eq!(app.click(3, Target::Other), Action::Focus("TA2".to_owned()));
}

#[test]
fn test_clicking_a_worktree_twice_cycles_its_tabs() {
    let mut app = two_tabs();

    assert_eq!(
        app.click(0, Target::Worktree),
        Action::Focus("TA1".to_owned())
    );
    assert_eq!(
        app.click(0, Target::Worktree),
        Action::Focus("TA2".to_owned())
    );
}

#[test]
fn test_slash_filters_as_you_type_and_includes_idle_worktrees() {
    let mut app = tall_app();
    app.handle(key(KeyCode::Char('a')));
    assert!(lines(&app).is_empty(), "every worktree is idle");

    type_keys(&mut app, "/w1");
    assert_eq!(lines(&app).len(), 10);
    type_keys(&mut app, "7");
    assert_eq!(lines(&app), ["w17"]);

    // Keys that normally act are part of the query while typing.
    type_keys(&mut app, "q");
    assert!(lines(&app).is_empty());
    app.handle(key(KeyCode::Backspace));

    app.handle(key(KeyCode::Enter));
    assert_eq!(lines(&app), ["w17"]);
    assert!(!app.filter.as_ref().unwrap().editing);

    // Esc clears a filter in force, and quits only once none is.
    assert_eq!(app.handle(key(KeyCode::Esc)), Action::None);
    assert!(app.filter.is_none());
    assert_eq!(app.handle(key(KeyCode::Esc)), Action::Quit);
}

#[test]
fn test_filter_matches_branch_names_and_esc_restores_the_previous_filter() {
    let mut app = App::default();
    app.apply(Update::Worktrees(Ok(vec![
        Worktree {
            path: Utf8PathBuf::from("/repo/one"),
            branch: Some("feature/stacks".to_owned()),
        },
        worktree("/repo/two"),
    ])));
    prs(&mut app, vec![pr(9, "stacks-fix")]);

    type_keys(&mut app, "/STACK");
    app.handle(key(KeyCode::Enter));
    assert_eq!(lines(&app), ["one", "#9"]);

    // `/` edits the filter in force, so this narrows it to `STACKs-f`.
    type_keys(&mut app, "/s-f");
    assert_eq!(lines(&app), ["#9"]);
    app.handle(key(KeyCode::Esc));
    assert_eq!(app.filter.as_ref().unwrap().query, "STACK");
    assert_eq!(lines(&app), ["one", "#9"]);
}

#[test]
fn test_sort_menu_chooses_the_order() {
    let mut app = app();
    prs(&mut app, vec![pr(3, "a"), pr(9, "c"), pr(5, "elsewhere")]);
    assert_eq!(lines(&app), ["a", "c", "#5"]);

    app.handle(key(KeyCode::Char('s')));
    assert_eq!(app.sort_menu, Some(0));

    // Keys move the highlight, not the table.
    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Down));
    assert_eq!(app.sort_menu, Some(2));
    app.handle(key(KeyCode::Enter));

    assert_eq!(app.sort, Sort::PullRequest);
    assert_eq!(app.sort_menu, None);
    assert_eq!(lines(&app), ["c", "#5", "a"]);

    // A digit picks straight away; esc closes without changing anything.
    type_keys(&mut app, "s5");
    assert_eq!(app.sort, Sort::Checks);
    app.handle(key(KeyCode::Char('s')));
    app.handle(key(KeyCode::Esc));
    assert_eq!(app.sort, Sort::Checks);
    assert_eq!(app.sort_menu, None);
}

#[test]
fn test_sort_by_activity_puts_a_turn_waiting_on_the_user_first() {
    let mut app = app();
    app.handle(key(KeyCode::Char('a')));
    let run = |cwd: &str, waiting_since| JpRun {
        pid: 1,
        cwd: Utf8PathBuf::from(cwd),
        conversation: "t".to_owned(),
        waiting_since,
    };
    jp(
        &mut app,
        vec![run("/repo/a", None), run("/repo/c", Some(at(1)))],
        vec![session(10, "/repo/b", vec![conversation(
            "jp-c1",
            true,
            Status::Idle,
        )])],
    );

    app.sort = Sort::Activity;
    assert_eq!(lines(&app), ["c", "a", "b"]);
}

#[test]
fn test_sort_by_checks_puts_failing_first() {
    let mut app = app();
    app.handle(key(KeyCode::Char('a')));
    let failing = PullRequest {
        checks: Checks {
            passed: 3,
            pending: 0,
            failed: 1,
        },
        ..pr(1, "c")
    };
    let running = PullRequest {
        checks: Checks {
            passed: 3,
            pending: 2,
            failed: 0,
        },
        ..pr(2, "b")
    };
    prs(&mut app, vec![failing, running]);

    app.sort = Sort::Checks;
    assert_eq!(lines(&app), ["c", "b", "a"]);
}

#[test]
fn test_n_asks_for_a_branch_and_creates_a_worktree_from_main() {
    let mut app = app();

    app.handle(key(KeyCode::Char('n')));
    assert_eq!(app.new_branch.as_deref(), Some(""));

    // Keys that normally act are part of the name; a space becomes a dash.
    type_keys(&mut app, "fix q");
    app.handle(key(KeyCode::Backspace));
    type_keys(&mut app, "queue");
    assert_eq!(app.new_branch.as_deref(), Some("fix-queue"));

    assert_eq!(app.handle(key(KeyCode::Enter)), Action::NewWorktree {
        branch: "fix-queue".to_owned()
    });
    assert_eq!(app.new_branch, None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Creating a worktree for fix-queue from main…")
    );
}

#[test]
fn test_n_with_an_empty_name_or_esc_creates_nothing() {
    let mut app = app();

    app.handle(key(KeyCode::Char('n')));
    assert_eq!(app.handle(key(KeyCode::Enter)), Action::None);
    assert_eq!(app.new_branch, None);

    type_keys(&mut app, "nx");
    assert_eq!(app.handle(key(KeyCode::Esc)), Action::None);
    assert_eq!(app.new_branch, None);
}

#[test]
fn test_a_created_worktree_is_selected_once_it_shows_up() {
    let mut app = app();

    app.apply(Update::Created {
        pr: None,
        outcome: Ok(Created {
            path: Utf8PathBuf::from("/repo/fix-queue"),
            stale: Some("main could not be updated (offline)".to_owned()),
        }),
    });
    assert_eq!(
        app.notice.as_deref(),
        Some("Created /repo/fix-queue, from the local main: main could not be updated (offline).")
    );

    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/a"),
        worktree("/repo/b"),
        worktree("/repo/c"),
        worktree("/repo/fix-queue"),
    ])));
    app.apply(Update::Terminals(Ok(vec![
        terminal("TA", "/repo/a"),
        terminal("TC", "/repo/c"),
        terminal("TF", "/repo/fix-queue"),
    ])));

    assert_eq!(selected(&app), "fix-queue");
}

#[test]
fn test_enter_on_a_pull_request_without_a_worktree_creates_one_once() {
    let mut app = app();
    prs(&mut app, vec![pr(9, "feature/stacks")]);
    type_keys(&mut app, "G");

    assert_eq!(app.handle(key(KeyCode::Enter)), Action::CreateWorktree {
        number: 9,
        branch: "feature/stacks".to_owned(),
    });
    assert_eq!(app.notice.as_deref(), Some("Creating a worktree for #9…"));

    // A second press while the first is still running does not start another.
    assert_eq!(app.handle(key(KeyCode::Enter)), Action::None);
    assert_eq!(
        app.notice.as_deref(),
        Some("Already creating a worktree for #9.")
    );

    app.apply(Update::Created {
        pr: Some(9),
        outcome: Err("Checking out #9 failed: no such pull request".to_owned()),
    });
    assert_eq!(app.handle(key(KeyCode::Enter)), Action::CreateWorktree {
        number: 9,
        branch: "feature/stacks".to_owned(),
    });
}

#[test]
fn test_selection_follows_a_pull_request_into_its_new_worktree() {
    let mut app = app();
    prs(&mut app, vec![pr(9, "stacks")]);
    type_keys(&mut app, "G");
    assert_eq!(selected(&app), "#9");

    app.apply(Update::Worktrees(Ok(vec![
        worktree("/repo/a"),
        worktree("/repo/b"),
        worktree("/repo/c"),
        worktree("/repo/stacks"),
    ])));

    assert_eq!(lines(&app), ["a", "c", "stacks"]);
    assert_eq!(selected(&app), "stacks");
}

#[test]
fn test_fit_scroll_keeps_the_selection_on_screen() {
    let mut app = app();
    app.handle(key(KeyCode::Char('a')));

    // Three rows, room for two.
    app.fit_scroll(2);
    assert_eq!(app.scroll, 0);

    app.handle(key(KeyCode::Down));
    app.handle(key(KeyCode::Down));
    app.fit_scroll(2);
    assert_eq!(app.scroll, 1);

    // `a`'s tab, once expanded, pushes `c` further down the drawn lines.
    app.handle(key(KeyCode::Char('g')));
    app.handle(key(KeyCode::Char('g')));
    app.handle(key(KeyCode::Right));
    app.handle(key(KeyCode::Char('G')));
    app.fit_scroll(2);
    // 0 `a`, 1 TA, 2 `b`, 3 `c`.
    assert_eq!(app.scroll, 2);

    app.handle(key(KeyCode::Char('g')));
    app.handle(key(KeyCode::Char('g')));
    app.fit_scroll(2);
    assert_eq!(app.scroll, 0);
}

#[test]
fn test_a_failed_read_keeps_the_last_good_data() {
    let mut app = app();

    app.apply(Update::Terminals(Err("osascript failed".to_owned())));

    assert_eq!(lines(&app), ["a", "c"]);
    assert_eq!(app.terminals.error.as_deref(), Some("osascript failed"));
}

#[test]
fn test_quit_keys() {
    let mut app = app();

    assert_eq!(app.handle(key(KeyCode::Char('q'))), Action::Quit);
    assert_eq!(app.handle(key(KeyCode::Esc)), Action::Quit);
    assert_eq!(app.handle(ctrl('c')), Action::Quit);
}
