use pretty_assertions::assert_eq;

use super::*;

#[test]
fn each_line_is_a_span_with_its_line_ending_between() {
    assert_eq!(
        render_text("fn main() {}\n// done\n").html,
        "<pre><span data-s=\"0\" data-e=\"12\">fn main() {}</span>\n<span data-s=\"13\" \
         data-e=\"20\">// done</span>\n</pre>\n"
    );
}

/// A browser places a selection by the span it starts or ends in, so an empty
/// line has one too.
#[test]
fn an_empty_line_is_an_empty_span() {
    assert_eq!(
        render_text("a\n\nb").html,
        "<pre><span data-s=\"0\" data-e=\"1\">a</span>\n<span data-s=\"2\" \
         data-e=\"2\"></span>\n<span data-s=\"3\" data-e=\"4\">b</span></pre>\n"
    );
}

#[test]
fn every_line_ending_is_left_out_of_the_spans() {
    assert_eq!(
        render_text("a\r\nb\rc\n").html,
        "<pre><span data-s=\"0\" data-e=\"1\">a</span>\n<span data-s=\"3\" \
         data-e=\"4\">b</span>\n<span data-s=\"5\" data-e=\"6\">c</span>\n</pre>\n"
    );
}

#[test]
fn markup_in_the_text_is_escaped_and_offsets_count_bytes() {
    assert_eq!(
        render_text("<é> & x\n").html,
        "<pre><span data-s=\"0\" data-e=\"8\">&lt;é&gt; &amp; x</span>\n</pre>\n"
    );
}

#[test]
fn an_empty_text_has_no_lines() {
    assert_eq!(render_text("").html, "<pre></pre>\n");
}
