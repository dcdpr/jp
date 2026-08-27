use super::*;
use crate::printer::OutputFormat;

/// A stack holding one immediately-visible region, plus its id.
fn visible_stack(columns: Option<u16>) -> (RegionStack, RegionId, Vec<u8>) {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, detail| {
        detail.unwrap_or("waiting").to_owned()
    });

    stack.claim(1, style, columns, &mut out);
    (stack, 1, out)
}

#[test]
fn regions_need_a_pretty_format() {
    let capability = TerminalCapability::interactive(Some(80));

    assert!(capability.permits_regions(OutputFormat::TextPretty));
    assert!(!capability.permits_regions(OutputFormat::Text));
    assert!(!capability.permits_regions(OutputFormat::Json));
    assert!(!capability.permits_regions(OutputFormat::JsonPretty));
}

#[test]
fn regions_need_an_interactive_stderr() {
    assert!(!TerminalCapability::default().permits_regions(OutputFormat::TextPretty));
}

#[test]
fn regions_are_off_while_logs_go_to_stderr() {
    let capability = TerminalCapability::interactive(Some(80)).with_stderr_logging(true);

    assert!(!capability.permits_regions(OutputFormat::TextPretty));
}

#[test]
fn claiming_a_due_region_paints_it_immediately() {
    let (_stack, _id, out) = visible_stack(None);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kwaiting");
}

#[test]
fn a_delayed_region_paints_nothing_until_its_delay_passes() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::from_mins(1), Duration::from_millis(50), |_, _| {
        "waiting".to_owned()
    });

    stack.claim(1, style, None, &mut out);
    stack.redraw(&mut out);

    assert!(out.is_empty());
}

#[test]
fn releasing_the_top_region_erases_its_row() {
    let (mut stack, id, mut out) = visible_stack(None);
    out.clear();

    stack.release(id, &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[K");
}

#[test]
fn setting_a_detail_repaints_the_row() {
    let (mut stack, id, mut out) = visible_stack(None);
    out.clear();

    stack.set_detail(id, "starting bookworm".to_owned(), &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kstarting bookworm");
}

#[test]
fn a_background_wraps_the_row_and_its_erase() {
    let (mut stack, id, mut out) = visible_stack(None);
    out.clear();

    stack.set_background(id, Some("\x1b[48;5;236m".to_owned()), &mut out);
    // The erase asserts the background too: `\x1b[K` fills with whatever is
    // active, so a region inside a shaded block must not punch a hole in it.
    stack.erase(&mut out);

    assert_eq!(
        String::from_utf8(out).unwrap(),
        "\r\x1b[48;5;236m\x1b[Kwaiting\x1b[49m\r\x1b[48;5;236m\x1b[K\x1b[49m"
    );
}

#[test]
fn the_newest_claim_renders_and_release_re_exposes_the_one_below() {
    let (mut stack, first, mut out) = visible_stack(None);

    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "running tool".to_owned()
    });
    stack.claim(2, style, None, &mut out);
    out.clear();

    stack.release(2, &mut out);

    assert_eq!(
        String::from_utf8(out).unwrap(),
        // Erase the tool row, then repaint the region underneath it.
        "\r\x1b[K\r\x1b[Kwaiting"
    );
    assert_eq!(first, 1);
}

#[test]
fn releasing_a_buried_claim_leaves_the_screen_alone() {
    let (mut stack, buried, mut out) = visible_stack(None);

    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "running tool".to_owned()
    });
    stack.claim(2, style, None, &mut out);
    out.clear();

    stack.release(buried, &mut out);

    assert!(out.is_empty());
}

#[test]
fn suspension_erases_the_row_and_blocks_redraws() {
    let (mut stack, id, mut out) = visible_stack(None);
    out.clear();

    stack.suspend(&mut out);
    assert_eq!(String::from_utf8(out.clone()).unwrap(), "\r\x1b[K");
    assert!(
        stack.tick_after().is_none(),
        "a suspended region must not tick"
    );

    // Nothing lands while suspended, including a detail update.
    out.clear();
    stack.redraw(&mut out);
    stack.set_detail(id, "still here".to_owned(), &mut out);
    assert!(out.is_empty());

    stack.resume(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kstill here");
}

#[test]
fn nested_suspensions_resume_only_on_the_last_release() {
    let (mut stack, _id, mut out) = visible_stack(None);

    stack.suspend(&mut out);
    stack.suspend(&mut out);
    out.clear();

    stack.resume(&mut out);
    assert!(out.is_empty(), "one suspension is still held");

    stack.resume(&mut out);
    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[Kwaiting");
}

#[test]
fn an_empty_stack_never_ticks() {
    assert!(RegionStack::new().tick_after().is_none());
}

#[test]
fn a_pending_region_ticks_at_its_remaining_delay() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::from_mins(1), Duration::from_millis(50), |_, _| {
        String::new()
    });

    stack.claim(1, style, None, &mut out);

    let tick = stack.tick_after().expect("a claimed region ticks");
    assert!(
        tick > Duration::from_secs(59) && tick <= Duration::from_mins(1),
        "a region waiting out its delay wakes when the delay ends, not on its interval; got \
         {tick:?}"
    );
}

#[test]
fn a_visible_region_ticks_at_its_interval() {
    let (stack, _id, _out) = visible_stack(None);

    assert_eq!(stack.tick_after(), Some(Duration::from_millis(50)));
}

#[test]
fn intervals_below_the_floor_are_raised() {
    let style = RegionStyle::new(Duration::ZERO, Duration::ZERO, |_, _| String::new());

    assert_eq!(style.interval, MIN_INTERVAL);
}

#[test]
fn rows_are_bounded_to_the_captured_column_count() {
    let mut stack = RegionStack::new();
    let mut out = Vec::new();
    let style = RegionStyle::new(Duration::ZERO, Duration::from_millis(50), |_, _| {
        "0123456789".to_owned()
    });

    stack.claim(1, style, Some(6), &mut out);

    assert_eq!(String::from_utf8(out).unwrap(), "\r\x1b[K012345");
}

#[test]
fn truncate_row_leaves_a_fitting_row_alone() {
    assert_eq!(truncate_row("⏱ Waiting… 4.2s", 40), "⏱ Waiting… 4.2s");
}

#[test]
fn truncate_row_measures_visible_columns() {
    // The escapes cost no columns, so all ten digits survive a budget of ten.
    assert_eq!(
        truncate_row("\x1b[2m0123456789\x1b[22m", 10),
        "\x1b[2m0123456789\x1b[22m"
    );
}

#[test]
fn truncate_row_keeps_escapes_whole_and_drops_text() {
    // The cut falls inside the styled run: the opening and closing sequences
    // are copied verbatim so the terminal is never left mid-sequence, and the
    // reset still lands.
    assert_eq!(
        truncate_row("\x1b[2m0123456789\x1b[22m", 4),
        "\x1b[2m0123\x1b[22m"
    );
}

#[test]
fn truncate_row_cuts_on_grapheme_boundaries() {
    // A budget that lands mid-character drops the character rather than
    // splitting its bytes.
    assert_eq!(truncate_row("aé日", 2), "aé");
}

#[test]
fn escape_end_spans_a_csi_sequence() {
    assert_eq!(escape_end("\x1b[48;5;236mrest"), 11);
    assert_eq!(escape_end("\x1b[Krest"), 3);
}

#[test]
fn escape_end_spans_an_osc_string() {
    assert_eq!(escape_end("\x1b]8;;https://example.com\x07rest"), 25);
    assert_eq!(escape_end("\x1b]0;title\x1b\\rest"), 11);
}

#[test]
fn escape_end_runs_to_the_end_of_an_unterminated_sequence() {
    assert_eq!(escape_end("\x1b[48;5"), 6);
    assert_eq!(escape_end("\x1b"), 1);
}
