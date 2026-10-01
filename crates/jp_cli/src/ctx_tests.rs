use camino_tempfile::tempdir;
use jp_config::style::Sanitization;
use jp_printer::{OutputFormat, SharedBuffer};

use super::*;

/// A context under `config`, printing pretty output to the returned buffer.
fn setup(config: AppConfig) -> (Ctx, SharedBuffer) {
    let tmp = tempdir().unwrap();
    let workspace = Workspace::in_memory(tmp.path());
    let (printer, out, _err) = Printer::memory(OutputFormat::TextPretty);
    let ctx = Ctx::new(
        ExecutionContext::for_workspace(&workspace),
        workspace,
        None,
        Runtime::new().unwrap(),
        Globals::default(),
        config,
        None,
        printer,
    );

    (ctx, out)
}

/// The test config with `style.sanitize` set to `sanitize`.
fn config_with(sanitize: Sanitization) -> AppConfig {
    let mut config = AppConfig::new_test();
    config.style.sanitize = sanitize;
    config
}

#[test]
fn the_printer_filters_the_way_style_sanitize_asks() {
    // Every command prints through this printer, including the ones that never
    // filter their own output.
    let (ctx, out) = setup(config_with(Sanitization::Visualize));

    ctx.printer.print("a\x1b[2Jb");
    ctx.printer.flush();

    assert_eq!(*out.lock(), "a\u{241b}b");
}

#[test]
fn a_swapped_config_changes_how_the_printer_filters() {
    let (mut ctx, out) = setup(config_with(Sanitization::Strip));

    ctx.swap_config(Arc::new(config_with(Sanitization::Off)));
    ctx.printer.print("a\x1b[2Jb");
    ctx.printer.flush();

    assert_eq!(*out.lock(), "a\x1b[2Jb");
}
