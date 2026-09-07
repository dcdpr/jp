use jp_printer::{Chrome, OutputFormat, Printer};

use super::*;

#[test]
fn reports_each_outdated_plugin() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);

    report_updates(&printer, &["serve".to_owned(), "ticket".to_owned()]);
    printer.flush();

    assert_eq!(
        *err.lock(),
        "  \u{2192} serve: update available\n  \u{2192} ticket: update available\n"
    );
}

#[test]
fn reports_that_everything_is_current() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);

    report_updates(&printer, &[]);
    printer.flush();

    assert_eq!(
        *err.lock(),
        "  \u{2192} All installed plugins are up to date.\n"
    );
}

// Registry status is chrome: a quiet run gets none of it, whether or not
// anything is outdated.
#[test]
fn a_quiet_run_reports_nothing() {
    let (printer, _out, err) = Printer::memory(OutputFormat::Text);
    let printer = printer.with_chrome(Chrome::Silenced);

    report_updates(&printer, &["serve".to_owned()]);
    report_updates(&printer, &[]);
    printer.flush();

    assert_eq!(*err.lock(), "");
}
