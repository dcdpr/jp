//! `dash`: a live terminal dashboard of the repository's worktrees.
//!
//! Displays open terminals, running `jp` processes, pull requests, CI status,
//! and review threads, and runs a merge queue for pull requests carrying the
//! queue label (see `queue` and `source::github::merge`).
//!
//! Run it from any worktree of the repository with `just dash`.
//! The sources are read on background threads (see `source`); the main thread
//! only draws and handles keys.

mod app;
mod create;
mod model;
mod queue;
mod source;
mod ui;

use std::{
    env,
    io::{self, Write},
    panic,
    process::{Command, ExitCode},
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::Utc;
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute, queue,
    style::{Attribute, Print, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen, SetTitle},
};

use crate::{
    app::{Action, App, Update},
    create::Created,
    source::{
        Refresher, ghostty,
        github::{self, QueueControl},
        jp,
    },
    ui::{Line, Span},
};

/// Poll timeout before redrawing with any pending source updates.
const TICK: Duration = Duration::from_millis(250);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let _unreported = writeln!(io::stderr(), "dash: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> io::Result<()> {
    let dir = Utf8PathBuf::try_from(env::current_dir()?).map_err(io::Error::other)?;

    let (updates, received) = mpsc::channel();
    let control = Arc::new(QueueControl::default());
    let refresher = source::spawn(&dir, &updates, Arc::clone(&control));
    let labels = {
        let (updates, refresher) = (updates.clone(), refresher.clone());
        github::label_editor(dir.clone(), move |number, add, outcome| {
            let outcome = outcome.map_err(|error| {
                let verb = if add { "Adding" } else { "Removing" };
                format!("{verb} the queue label on #{number} failed: {error}")
            });
            let _gone = updates.send(Update::QueueEdited {
                number,
                add,
                outcome,
            });
            refresher.refresh_pull_requests();
        })
    };

    // Restore the terminal before printing panic output so the message is not
    // lost when leaving the alternate screen.
    let report = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        report(info);
    }));

    let _screen = Screen::enter()?;
    event_loop(&dir, &received, &updates, &refresher, &control, &labels)
}

/// RAII guard that keeps the terminal in raw mode on the alternate screen.
struct Screen;

impl Screen {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Construct the guard immediately so Drop disables raw mode if a later
        // setup step fails.
        let screen = Self;
        // The title marks the dashboard's own window so new tabs open in a
        // different window.
        //
        // Without mouse capture, the terminal translates scroll-wheel events
        // into arrow keys on the alternate screen, which wrap around the table.
        // Capturing mouse events lets scroll events clamp at the table bounds
        // and routes clicks through the app's hit-testing.
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            Hide,
            SetTitle(ghostty::TITLE)
        )?;
        Ok(screen)
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let _unrestored = execute!(
        io::stdout(),
        Show,
        DisableMouseCapture,
        LeaveAlternateScreen
    );
    let _still_raw = terminal::disable_raw_mode();
}

fn event_loop(
    dir: &Utf8Path,
    received: &Receiver<Update>,
    updates: &Sender<Update>,
    refresher: &Refresher,
    control: &QueueControl,
    labels: &Sender<(u64, bool)>,
) -> io::Result<()> {
    let mut out = io::stdout();
    let mut app = App::default();
    let mut painted: Vec<Line> = vec![];

    loop {
        while let Ok(update) = received.try_recv() {
            app.apply(update);
        }

        let (width, height) = terminal::size()?;
        app.now = Utc::now();
        app.fit_scroll(ui::body_rows(height));
        let lines = ui::screen(&app, width, height);
        if lines != painted {
            paint(&mut out, &lines)?;
            painted = lines;
        }

        if !event::poll(TICK)? {
            continue;
        }

        let event = event::read()?;
        if let Event::Resize(..) = event {
            // Clear cells outside the new dimensions that the next frame will
            // not overwrite.
            execute!(out, Clear(ClearType::All))?;
            painted.clear();
            continue;
        }

        match react(&mut app, &event, width, height) {
            Action::Quit => return Ok(()),
            action => carry_out(action, dir, updates, refresher, control, labels),
        }
    }
}

/// Dispatch a key press or mouse event to `app` and return the resulting
/// [`Action`].
fn react(app: &mut App, event: &Event, width: u16, height: u16) -> Action {
    match event {
        Event::Key(key) if key.kind == KeyEventKind::Press => app.handle(*key),
        Event::Mouse(mouse) => match mouse.kind {
            MouseEventKind::ScrollDown => {
                app.wheel(true);
                Action::None
            }
            MouseEventKind::ScrollUp => {
                app.wheel(false);
                Action::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                match ui::hit(app, width, height, mouse.column, mouse.row) {
                    Some((line, target)) => app.click(line, target),
                    None => Action::None,
                }
            }
            _ => Action::None,
        },
        _ => Action::None,
    }
}

/// Execute `action`, spawning a background thread for subprocess calls.
fn carry_out(
    action: Action,
    dir: &Utf8Path,
    updates: &Sender<Update>,
    refresher: &Refresher,
    control: &QueueControl,
    labels: &Sender<(u64, bool)>,
) {
    match action {
        Action::None | Action::Quit => {}
        Action::Refresh => refresher.refresh(),
        Action::Focus(id) => in_background(updates, move || {
            ghostty::focus(&id).map_err(|error| format!("Focus failed: {error}"))
        }),
        Action::OpenTab(path) => {
            let refresher = refresher.clone();
            in_background(updates, move || {
                let opened = ghostty::open_tab(&path, None)
                    .map_err(|error| format!("Opening a tab in {path} failed: {error}"));
                refresher.refresh();
                opened
            });
        }
        Action::Open(url) => in_background(updates, move || open_url(&url)),
        Action::CreateWorktree { number, branch } => {
            create_worktree(dir, Some(number), updates, refresher, move |dir| {
                create::for_pull_request(dir, number, &branch)
            });
        }
        Action::NewWorktree { branch } => {
            create_worktree(dir, None, updates, refresher, move |dir| {
                create::for_branch(dir, &branch)
            });
        }
        Action::RunInTab {
            terminal,
            shell,
            command,
        } => {
            let refresher = refresher.clone();
            in_background(updates, move || {
                run_in_tab(&terminal, shell, &command)?;
                refresher.refresh();
                Ok(())
            });
        }
        Action::RunInNewTab { dir, command } => {
            let refresher = refresher.clone();
            in_background(updates, move || {
                ghostty::open_tab(&dir, Some(&command))
                    .map_err(|error| format!("Opening a tab for `{command}` failed: {error}"))?;
                refresher.refresh();
                Ok(())
            });
        }
        Action::PauseQueue(paused) => pause_queue(paused, control, refresher),
        Action::Queue { number, add } => edit_queue_label(number, add, control, labels),
    }
}

/// Pause the merge queue in this process, or resume it.
fn pause_queue(paused: bool, control: &QueueControl, refresher: &Refresher) {
    control.set_paused(paused);
    // A pause takes effect at the next poll; a resume should act now.
    if !paused {
        refresher.refresh();
    }
}

/// Hand adding or removing the queue label on pull request `number` to the
/// label editor.
fn edit_queue_label(number: u64, add: bool, control: &QueueControl, labels: &Sender<(u64, bool)>) {
    // Re-enqueuing a pull request clears any hold placed on its entry.
    if add {
        control.clear(number);
    }
    // The editor applies label changes sequentially and outlives the event
    // loop.
    let _gone = labels.send((number, add));
}

fn open_url(url: &str) -> Result<(), String> {
    let status = Command::new("open")
        .arg(url)
        .status()
        .map_err(|error| format!("Opening {url} failed: {error}"))?;

    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Opening {url} failed: {status}"))
}

/// Create a worktree on a background thread, open a tab in it, and trigger a
/// source refresh.
fn create_worktree(
    dir: &Utf8Path,
    pr: Option<u64>,
    updates: &Sender<Update>,
    refresher: &Refresher,
    create: impl FnOnce(&Utf8Path) -> Result<Created, String> + Send + 'static,
) {
    let (dir, updates, refresher) = (dir.to_owned(), updates.clone(), refresher.clone());

    thread::spawn(move || {
        let outcome = create(&dir).and_then(|created| {
            ghostty::open_tab(&created.path, None)
                .map(|()| created.clone())
                .map_err(|error| {
                    format!(
                        "Created {}, but opening a tab failed: {error}",
                        created.path
                    )
                })
        });

        refresher.refresh();
        let _gone = updates.send(Update::Created { pr, outcome });
    });
}

/// Send `command` to `terminal` only if `shell` is the foreground process.
///
/// If another process is in the foreground, keystrokes sent to the terminal
/// would go to that process instead of the shell.
fn run_in_tab(terminal: &str, shell: u32, command: &str) -> Result<(), String> {
    match jp::shell_is_idle(shell) {
        Ok(true) => {}
        Ok(false) => {
            return Err(format!(
                "Not running `{command}`: something is running in that tab. Finish it, then try \
                 again."
            ));
        }
        Err(error) => return Err(format!("Not running `{command}`: {error}")),
    }

    ghostty::run_in(terminal, command)
        .map_err(|error| format!("Running `{command}` failed: {error}"))
}

/// Draw `lines` from the top of the screen, one per terminal row.
///
/// Every line is padded to the full terminal width, so rows overwrite previous
/// content without clearing first.
fn paint(out: &mut impl Write, lines: &[Line]) -> io::Result<()> {
    for (row, line) in (0..).zip(lines) {
        queue!(out, MoveTo(0, row))?;

        for span in &line.spans {
            queue_span(out, span)?;
        }
    }

    queue!(out, SetAttribute(Attribute::Reset))?;
    out.flush()
}

/// Queue `span` with its terminal attributes, resetting any attributes from the
/// previous span first.
fn queue_span(out: &mut impl Write, span: &Span) -> io::Result<()> {
    queue!(out, SetAttribute(Attribute::Reset))?;
    if span.style.bold {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }
    if span.style.dim {
        queue!(out, SetAttribute(Attribute::Dim))?;
    }
    if span.style.reversed {
        queue!(out, SetAttribute(Attribute::Reverse))?;
    }
    if let Some(color) = span.style.fg {
        queue!(out, SetForegroundColor(color))?;
    }

    // OSC 8 hyperlink escapes occupy zero terminal columns, preserving column
    // widths.
    let Some(url) = &span.link else {
        return queue!(out, Print(&span.text));
    };
    queue!(
        out,
        Print(format!("\x1b]8;;{url}\x1b\\{}\x1b]8;;\x1b\\", span.text))
    )
}

/// Run `task` on a background thread and report any error as a status notice.
///
/// Subprocess calls such as `osascript` are slow enough to block key handling
/// if run on the UI thread.
fn in_background<F>(updates: &Sender<Update>, task: F)
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    let updates = updates.clone();
    thread::spawn(move || {
        if let Err(error) = task() {
            let _gone = updates.send(Update::Notice(error));
        }
    });
}
