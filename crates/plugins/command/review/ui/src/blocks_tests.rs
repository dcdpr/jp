use pretty_assertions::assert_eq;

use super::*;

#[test]
fn each_top_level_element_is_a_block() {
    let html = concat!(
        "<h1><span data-s=\"2\" data-e=\"7\">Title</span></h1>\n",
        "<p><span data-s=\"9\" data-e=\"14\">First</span> <em><span data-s=\"16\" \
         data-e=\"20\">line</span></em></p>\n",
        "<hr />\n",
        "<ul>\n<li><span data-s=\"30\" data-e=\"34\">item</span></li>\n</ul>\n",
    );

    assert_eq!(blocks(html), [
        Block {
            html: "<h1><span data-s=\"2\" data-e=\"7\">Title</span></h1>".to_owned(),
            bytes: Some([2, 7]),
            line: false,
        },
        Block {
            html: "<p><span data-s=\"9\" data-e=\"14\">First</span> <em><span data-s=\"16\" \
                   data-e=\"20\">line</span></em></p>"
                .to_owned(),
            bytes: Some([9, 20]),
            line: false,
        },
        Block {
            html: "<hr />".to_owned(),
            bytes: None,
            line: false,
        },
        Block {
            html: "<ul>\n<li><span data-s=\"30\" data-e=\"34\">item</span></li>\n</ul>".to_owned(),
            bytes: Some([30, 34]),
            line: false,
        },
    ]);
}

/// A task list writes a void `<input>` without a closing slash in some
/// renderers; either way it doesn't open an element.
#[test]
fn a_void_element_nests_nothing() {
    let html = "<ul>\n<li><input type=\"checkbox\" disabled=\"\"> <span data-s=\"6\" \
                data-e=\"10\">done</span><br></li>\n</ul>\n<p><span data-s=\"12\" \
                data-e=\"13\">x</span></p>\n";

    let found = blocks(html);

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].bytes, Some([6, 10]));
    assert_eq!(found[1].bytes, Some([12, 13]));
}

#[test]
fn a_quoted_attribute_can_hold_an_angle_bracket() {
    let html = "<p><a href=\"x?a>b\" title='c>d'><span data-s=\"1\" \
                data-e=\"2\">a</span></a></p>\n<p><span data-s=\"5\" data-e=\"6\">b</span></p>\n";

    let found = blocks(html);

    assert_eq!(found.len(), 2);
    assert_eq!(found[1].bytes, Some([5, 6]));
}

/// A code block is one block: its lines aren't split the way a text file's are.
#[test]
fn a_code_block_stays_whole() {
    let html = "<pre><code class=\"language-rust\"><span data-s=\"8\" data-e=\"20\">fn main() \
                {}</span>\n\n<span data-s=\"22\" data-e=\"32\">let x = 1;</span>\n</code></pre>\n";

    let found = blocks(html);

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].bytes, Some([8, 32]));
    assert!(!found[0].line);
}

/// Each line of a plain-text file is a block of its own, its empty lines
/// included.
#[test]
fn each_line_of_a_text_file_is_a_block() {
    let html = "<pre><span data-s=\"0\" data-e=\"5\">plain</span>\n<span data-s=\"6\" \
                data-e=\"6\"></span>\n<span data-s=\"7\" \
                data-e=\"15\">&lt;text&gt;</span>\n</pre>\n";

    assert_eq!(blocks(html), [
        Block {
            html: "<span data-s=\"0\" data-e=\"5\">plain</span>".to_owned(),
            bytes: Some([0, 5]),
            line: true,
        },
        Block {
            html: "<span data-s=\"6\" data-e=\"6\"></span>".to_owned(),
            bytes: Some([6, 6]),
            line: true,
        },
        Block {
            html: "<span data-s=\"7\" data-e=\"15\">&lt;text&gt;</span>".to_owned(),
            bytes: Some([7, 15]),
            line: true,
        },
    ]);
}

#[test]
fn an_empty_file_has_no_blocks() {
    assert_eq!(blocks(""), []);
    assert_eq!(blocks("<pre></pre>\n").len(), 1);
}

#[test]
fn text_outside_any_element_is_a_block_of_its_own() {
    let found = blocks("stray\n<p><span data-s=\"0\" data-e=\"1\">a</span></p>");

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].html, "stray");
    assert_eq!(found[0].bytes, None);
}
