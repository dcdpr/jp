use super::strip_environment;

fn strip(xml: &str) -> String {
    String::from_utf8(strip_environment(xml.as_bytes().to_vec())).unwrap()
}

#[test]
fn removes_the_items_but_keeps_the_document() {
    let input = "<info><target><process pid=\"42\"/><environment>\n    <item \
                 key=\"ANTHROPIC_API_KEY\" value=\"sk-ant-secret\"/>\n    <item key=\"HOME\" \
                 value=\"/Users/jean\"/>\n  </environment></target></info>";

    assert_eq!(
        strip(input),
        "<info><target><process pid=\"42\"/><environment redacted=\"true\"/></target></info>"
    );
}

#[test]
fn removes_every_block() {
    let input = "<run><environment><item key=\"A\" \
                 value=\"1\"/></environment></run><run><environment><item key=\"B\" \
                 value=\"2\"/></environment></run>";

    assert_eq!(
        strip(input),
        "<run><environment redacted=\"true\"/></run><run><environment redacted=\"true\"/></run>"
    );
}

#[test]
fn leaves_a_self_closing_element_alone() {
    assert_eq!(
        strip("<target><environment/></target>"),
        "<target><environment/></target>"
    );
}

/// `<environment-info>` shares a prefix with the element being stripped, and
/// removing it would silently eat unrelated parts of the document.
#[test]
fn leaves_longer_element_names_alone() {
    let input = "<environment-info><item key=\"A\" value=\"1\"/></environment-info>";
    assert_eq!(strip(input), input);
}

/// Truncated XML must not become a way to smuggle the environment through.
/// The items after the unterminated tag are dropped, not passed along.
#[test]
fn truncates_an_unterminated_block() {
    let input = "<target><environment><item key=\"ANTHROPIC_API_KEY\" value=\"sk-ant-secret\"/>";

    let output = strip(input);
    assert_eq!(output, "<target><environment redacted=\"true\"/>");
    assert!(!output.contains("sk-ant-secret"));
}

#[test]
fn leaves_xml_without_an_environment_untouched() {
    let input = "<trace-toc><run number=\"1\"><table schema=\"time-sample\"/></run></trace-toc>";
    assert_eq!(strip(input), input);
}
