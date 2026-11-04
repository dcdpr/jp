//! `dash`: a live table of the repository's worktrees and what is going on in
//! each one — open terminals, running `jp` processes, pull requests, CI, and
//! review threads.
//!
//! Run it from any worktree of the repository with `just dash`.
//! The sources are read on background threads (see [`source`]); the main thread
//! only draws and handles keys.

mod app;
mod create;
mod model;
mod source;
mod ui;

use std::{
    env,
    io::{self, Write},
    panic,
    process::{Command, ExitCode},
    sync::mpsc::{self, Receiver, Sender},
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
    source::{Refresher, ghostty, jp},
    ui::Line,
};

/// How long to wait for a key before redrawing with whatever the sources sent.
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
    let refresher = source::spawn(&dir, &updates);

    // A panic message printed on the alternate screen vanishes with it, so the
    // terminal is put back before the message is printed.
    let report = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        report(info);
    }));

    let _screen = Screen::enter()?;
    event_loop(&dir, &received, &updates, &refresher)
}

/// The terminal in raw mode on the alternate screen, until dropped.
struct Screen;

impl Screen {
    fn enter() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        // Created before anything else can fail, so raw mode is undone on
        // every path out.
        let screen = Self;
        // The title marks the dashboard's own window, which new tabs avoid.
        //
        // Without mouse capture, the terminal turns the scroll wheel into
        // arrow keys on the alternate screen, which wrap around the table.
        // Captured, a wheel turn arrives as its own event and stops at the
        // ends. It also turns clicks into events, so the PR number's link is
        // opened here rather than by the terminal.
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

        let action = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.handle(key),
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
                    match ui::hit(&app, width, height, mouse.column, mouse.row) {
                        Some((line, target)) => app.click(line, target),
                        None => Action::None,
                    }
                }
                _ => continue,
            },
            Event::Resize(..) => {
                // Text the old size left outside the new one is not
                // overwritten by the next frame.
                execute!(out, Clear(ClearType::All))?;
                painted.clear();
                continue;
            }
            _ => continue,
        };

        if action == Action::Quit {
            return Ok(());
        }
        carry_out(action, dir, updates, refresher);
    }
}

/// Do what a key press or click asked for, off the UI thread when it takes a
/// process or two.
fn carry_out(action: Action, dir: &Utf8Path, updates: &Sender<Update>, refresher: &Refresher) {
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
                // The new tab shows up at once rather than at the next
                // poll.
                refresher.refresh();
                opened
            });
        }
        Action::Open(url) => in_background(updates, move || {
            Command::new("open")
                .arg(&url)
                .status()
                .map_err(|error| format!("Opening {url} failed: {error}"))
                .and_then(|status| {
                    status
                        .success()
                        .then_some(())
                        .ok_or_else(|| format!("Opening {url} failed: {status}"))
                })
        }),
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
    }
}

/// Create a worktree with `create` and open a tab in it, off the UI thread,
/// then have the sources read again so the new worktree and its tab show up.
/// `pr` is the pull request it is for, if any.
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
        // The UI has quit when nobody is listening; nothing to report to.
        let _gone = updates.send(Update::Created { pr, outcome });
    });
}

/// Run `command` at the prompt of `shell` in the terminal `terminal`, but only
/// if the shell sits idle there: typed while something else runs in front of
/// it, the command would go to that program instead.
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
/// Every line spans the full width, so nothing needs clearing first.
fn paint(out: &mut impl Write, lines: &[Line]) -> io::Result<()> {
    for (row, line) in (0..).zip(lines) {
        queue!(out, MoveTo(0, row))?;

        for span in &line.spans {
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

            // OSC 8 escapes take no columns, so the layout's widths still hold.
            match &span.link {
                Some(url) => queue!(
                    out,
                    Print(format!("\x1b]8;;{url}\x1b\\{}\x1b]8;;\x1b\\", span.text))
                )?,
                None => queue!(out, Print(&span.text))?,
            }
        }
    }

    queue!(out, SetAttribute(Attribute::Reset))?;
    out.flush()
}

/// Run `task` off the UI thread, reporting its failure on the status line.
///
/// Focusing a terminal goes through `osascript`, which takes long enough to
/// make the keyboard feel stuck if it ran inline.
fn in_background<F>(updates: &Sender<Update>, task: F)
where
    F: FnOnce() -> Result<(), String> + Send + 'static,
{
    let updates = updates.clone();
    thread::spawn(move || {
        if let Err(error) = task() {
            // The UI has quit when nobody is listening; nothing to report to.
            let _gone = updates.send(Update::Notice(error));
        }
    });
}
