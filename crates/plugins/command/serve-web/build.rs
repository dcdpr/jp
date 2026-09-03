//! Fails the build when an embedded script cannot run.
//!
//! The scripts are served verbatim inside `<script>` tags and are otherwise
//! opaque to the toolchain: a syntax error or a duplicate declaration compiles
//! perfectly well and only shows up as a blank page in a browser console.

use std::{fs, process};

use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;

/// The scripts each page loads, in the order its `<script>` tags appear.
///
/// Grouped per page rather than checked one file at a time because classic
/// scripts share a single global lexical scope.
/// A `const` in one collides with the same name in the next, and neither file
/// is wrong on its own.
const PAGES: &[&[&str]] = &[
    &["src/views/configs.js", "src/views/new.js"],
    &["src/views/configs.js", "src/views/detail.js"],
    &["src/views/list.js", "src/views/filter.js"],
];

fn main() {
    let mut failed = false;

    for page in PAGES {
        let mut source = String::new();
        for path in *page {
            println!("cargo::rerun-if-changed={path}");

            let text = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
            source.push_str(&text);
            source.push('\n');
        }

        for problem in check(&source) {
            // One line per directive, so a multi-line diagnostic is flattened.
            let problem = problem.replace('\n', " ");
            println!("cargo::error={}: {problem}", page.join(" + "));
            failed = true;
        }
    }

    if failed {
        process::exit(1);
    }
}

/// Parse as a classic script and apply the spec's early-error rules.
fn check(source: &str) -> Vec<String> {
    let allocator = Allocator::default();

    // Script rather than module, which is what a `<script>` tag runs. Module
    // mode implies strict mode and would reject things a page may legitimately
    // do.
    let parsed = Parser::new(&allocator, source, SourceType::cjs()).parse();

    if !parsed.diagnostics.is_empty() {
        return parsed.diagnostics.iter().map(ToString::to_string).collect();
    }

    // The parser deliberately leaves redeclarations and the rest of the early
    // errors to the semantic pass, so grammar alone is not enough.
    SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&parsed.program)
        .diagnostics
        .iter()
        .map(ToString::to_string)
        .collect()
}
