use super::*;

#[test]
fn test_inline_option_new() {
    let opt = InlineOption::new('y', "yes - proceed");
    assert_eq!(opt.key, 'y');
    assert_eq!(opt.description, "yes - proceed");
}

#[test]
fn test_inline_select_build_help_text() {
    let options = vec![
        InlineOption::new('y', "stage this hunk"),
        InlineOption::new('n', "do not stage this hunk"),
        InlineOption::new('q', "quit"),
    ];
    let select = InlineSelect::new("Stage this hunk", options);
    let help = select.build_help_text().unwrap();

    assert!(help.contains("y - stage this hunk"));
    assert!(help.contains("n - do not stage this hunk"));
    assert!(help.contains("q - quit"));
    assert!(help.contains("? - print help"));
}
