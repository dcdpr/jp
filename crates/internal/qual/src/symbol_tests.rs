use indoc::indoc;

use super::*;

// Line numbers in the tests below refer to this source.
const SOURCE: &str = indoc! {r#"
    //! Module docs.
    use std::fmt;

    /// Parses input.
    pub fn parse(input: &str) -> usize {
        let inner = || 42;
        inner()
    }

    pub struct Parser<T> {
        items: Vec<T>,
    }

    impl<T> Parser<T> {
        pub fn run(&self) -> usize {
            self.items.len()
        }
    }

    impl<T> fmt::Display for Parser<T> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("parser")
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn reads_input() {
            fn helper() {}
            helper();
        }
    }
"#};

fn symbol_at(line: usize) -> String {
    Symbols::parse(SOURCE).unwrap().at(line)
}

#[test]
fn a_line_outside_every_item_has_no_symbol() {
    assert_eq!(symbol_at(1), "");
    assert_eq!(symbol_at(2), "");
    assert_eq!(symbol_at(9), "");
}

#[test]
fn a_line_past_the_end_has_no_symbol() {
    assert_eq!(symbol_at(99), "");
}

#[test]
fn a_function_spans_its_doc_comment_and_body() {
    assert_eq!(symbol_at(4), "parse");
    assert_eq!(symbol_at(6), "parse");
}

#[test]
fn a_struct_is_named_without_its_generics() {
    assert_eq!(symbol_at(11), "Parser");
}

#[test]
fn an_inherent_impl_is_named_after_its_type() {
    assert_eq!(symbol_at(14), "Parser");
    assert_eq!(symbol_at(16), "Parser::run");
}

#[test]
fn a_trait_impl_names_its_trait() {
    assert_eq!(symbol_at(22), "<Parser as Display>::fmt");
}

#[test]
fn a_module_spans_its_attributes() {
    assert_eq!(symbol_at(26), "tests");
}

#[test]
fn nested_items_are_joined_into_a_path() {
    assert_eq!(symbol_at(29), "tests::reads_input");
    assert_eq!(symbol_at(30), "tests::reads_input::helper");
}

#[test]
fn contains_an_item_by_its_whole_path() {
    let symbols = Symbols::parse(SOURCE).unwrap();

    assert!(symbols.contains("parse"));
    assert!(symbols.contains("Parser::run"));
    assert!(symbols.contains("<Parser as Display>::fmt"));

    assert!(!symbols.contains("run"));
    assert!(!symbols.contains("Parser::parse"));
    assert!(!symbols.contains(""));
}

#[test]
fn rejects_source_that_does_not_parse() {
    assert!(Symbols::parse("fn (").is_err());
}
