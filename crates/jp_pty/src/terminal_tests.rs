use std::io::Write as _;

use super::*;

/// Write `text` into a model-backed terminal and return the resulting screen.
///
/// A model-backed write lands synchronously, so there is nothing to wait for.
fn draw(terminal: &Terminal, text: &str) -> Screen {
    assert_eq!(terminal.backend(), Backend::Modelled);

    let mut writer = terminal.writer().expect("a writable terminal");
    write!(writer, "{text}").expect("the write to reach the terminal");
    writer.flush().expect("the write to reach the terminal");

    terminal.screen()
}

#[test]
fn onlcr_leaves_a_line_without_breaks_alone() {
    assert!(matches!(onlcr(b"plain"), Cow::Borrowed(_)));
    assert_eq!(onlcr(b"plain").as_ref(), b"plain");
}

#[test]
fn onlcr_returns_the_cursor_on_every_break() {
    // Unconditional, as the terminal driver is: the doubled carriage return in
    // the second case is what a real tty produces too, and renders the same.
    assert_eq!(onlcr(b"a\nb").as_ref(), b"a\r\nb");
    assert_eq!(onlcr(b"a\r\nb").as_ref(), b"a\r\r\nb");
}

#[test]
fn a_modelled_terminal_starts_blank_at_its_declared_size() {
    let terminal = Terminal::modelled(Size::new(4, 12));

    assert_eq!(terminal.backend(), Backend::Modelled);
    assert_eq!(terminal.screen().size(), Size::new(4, 12));
    assert_eq!(terminal.screen().cursor(), (0, 0));
}

#[test]
fn a_line_break_returns_the_cursor_to_the_first_column() {
    // JP writes a bare `\n` between the rows of a block and relies on the
    // terminal to treat it as a newline. A screen model fed directly would move
    // the cursor down and leave the column where it was.
    let terminal = Terminal::modelled(Size::new(4, 12));

    assert_eq!(draw(&terminal, "one\ntwo").cursor(), (1, 3));
}

#[test]
fn writing_past_the_last_row_scrolls_rather_than_overwrites() {
    let terminal = Terminal::modelled(Size::new(3, 12));
    let screen = draw(&terminal, "one\ntwo\nthree\nfour");

    assert_eq!(screen.rows(), ["two", "three", "four"]);
    assert_eq!(screen.cursor(), (2, 4));
}

#[test]
fn resizing_changes_what_the_screen_reports() {
    let terminal = Terminal::modelled(Size::new(4, 12));
    terminal
        .resize(Size::new(2, 6))
        .expect("the resize to apply");

    assert_eq!(terminal.screen().size(), Size::new(2, 6));
}

#[test]
fn a_modelled_terminal_has_nobody_to_type_at() {
    let terminal = Terminal::modelled(Size::new(4, 12));

    assert!(matches!(terminal.send("x"), Err(Error::NotAPty(_))));
    assert!(matches!(
        terminal.spawn(CommandBuilder::new("true")),
        Err(Error::NotAPty(_))
    ));
}

#[test]
fn an_unsatisfiable_wait_on_a_modelled_terminal_gives_up_at_once() {
    // Nothing but a write moves a model-backed screen, so waiting out the
    // timeout would only make a failing assertion slower to report.
    let terminal = Terminal::modelled(Size::new(2, 8)).with_timeout(Duration::from_mins(10));
    let started = Instant::now();

    let error = terminal
        .wait_for("a row that was never written", |screen| {
            screen.contains("nope")
        })
        .expect_err("nothing can satisfy this");

    assert!(matches!(error, Error::Stalled { .. }), "{error}");
    assert!(started.elapsed() < Duration::from_secs(1), "{error}");
}

#[cfg(unix)]
#[test]
fn open_takes_the_pty_where_there_is_one() {
    // The fallback to the screen model is for platforms without a pty a parent
    // can write into. On unix, silently taking it would cost every ported test
    // its line-discipline fidelity without saying so.
    let terminal = Terminal::open(Size::new(4, 12));

    assert_eq!(terminal.backend(), Backend::Pty);
}

#[cfg(unix)]
#[test]
fn a_pty_renders_what_the_model_renders() {
    // The model-backed terminal stands in for a pty on platforms that have
    // none, so what it shows has to be what a real one shows. These are the
    // sequences the status region emits: plain lines, rows reserved with line
    // breaks, a walk back up, then each row filled from its first column.
    //
    // `written\nnext` is the part that tells the two apart: a terminal returns
    // the cursor to the first column on a line break and a screen model fed raw
    // bytes does not, so `next` starts under the end of `written` unless the
    // model imitates the line discipline.
    let text = "written\nnext\n\n\n\x1b[2A\r\x1b[Kone\n\r\x1b[Ktwo\n\r\x1b[K* status";
    let size = Size::new(6, 20);

    let model = Terminal::modelled(size);
    let expected = draw(&model, text);

    let pty = Terminal::pty(size).expect("a pty on unix");
    let mut writer = pty.writer().expect("a writable subsidiary end");
    write!(writer, "{text}").expect("the write to reach the pty");
    writer.flush().expect("the write to reach the pty");

    if let Err(error) = pty.wait_for("the pty to render what the model did", |screen| {
        *screen == expected
    }) {
        panic!("{error}\n\nthe model rendered:\n{expected}");
    }
}

#[cfg(unix)]
#[test]
fn resizing_a_pty_reaches_the_kernel() {
    let terminal = Terminal::pty(Size::new(24, 80)).expect("a pty on unix");
    terminal
        .resize(Size::new(10, 40))
        .expect("the resize to apply");

    assert_eq!(terminal.screen().size(), Size::new(10, 40));
}
