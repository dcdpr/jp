use super::Toc;

/// Every fixture here is compact, with no whitespace between elements.
///
/// That is deliberate rather than a style choice: the parser reads with
/// `trim_text(false)`, so an indented document puts a `Text` event between
/// every pair of elements.
/// A parser that consumes one event too many after a process would eat that
/// whitespace and look correct, and only a document without it shows the next
/// process being lost instead.
const THREE_SELF_CLOSING: &str = concat!(
    r#"<trace-toc><run number="1"><processes>"#,
    r#"<process name="JP" pid="501" path="/Applications/JP.app"/>"#,
    r#"<process name="loginwindow" pid="502"/>"#,
    r#"<process name="WindowServer" pid="503"/>"#,
    r#"</processes></run></trace-toc>"#,
);

/// `xctrace` writes `<process/>` self-closing when a process carries no child
/// elements, which is the ordinary case.
#[test]
fn every_self_closing_process_is_read() {
    let toc = Toc::parse(THREE_SELF_CLOSING.as_bytes()).unwrap();
    let run = toc.first_run().unwrap();
    let names: Vec<&str> = run.processes.iter().map(|p| p.name.as_str()).collect();

    assert_eq!(names, ["JP", "loginwindow", "WindowServer"]);
    assert_eq!(run.processes[0].pid, 501);
    assert_eq!(
        run.processes[0].path.as_deref(),
        Some("/Applications/JP.app")
    );
    // Absent rather than empty, so a caller can tell "no path recorded" from a
    // process whose path is the empty string.
    assert_eq!(run.processes[1].path, None);
}

/// The expanded form has an end tag, and the process after it must still be
/// read: skipping to the close has to stop at the matching one.
#[test]
fn an_expanded_process_does_not_consume_its_sibling() {
    let xml = concat!(
        r#"<trace-toc><run number="1"><processes>"#,
        r#"<process name="JP" pid="501"><detail/></process>"#,
        r#"<process name="loginwindow" pid="502"/>"#,
        r#"</processes></run></trace-toc>"#,
    );

    let toc = Toc::parse(xml.as_bytes()).unwrap();
    let names: Vec<&str> = toc
        .first_run()
        .unwrap()
        .processes
        .iter()
        .map(|p| p.name.as_str())
        .collect();

    assert_eq!(names, ["JP", "loginwindow"]);
}

/// A `pid` that is missing or unparseable reads as `-1` rather than failing the
/// whole table: one unreadable row should not cost the rest of the trace.
#[test]
fn an_unreadable_pid_reads_as_minus_one() {
    let xml = concat!(
        r#"<trace-toc><run number="1"><processes>"#,
        r#"<process name="no-pid"/>"#,
        r#"<process name="bad-pid" pid="not-a-number"/>"#,
        r#"</processes></run></trace-toc>"#,
    );

    let toc = Toc::parse(xml.as_bytes()).unwrap();
    let pids: Vec<i64> = toc
        .first_run()
        .unwrap()
        .processes
        .iter()
        .map(|p| p.pid)
        .collect();

    assert_eq!(pids, [-1, -1]);
}
