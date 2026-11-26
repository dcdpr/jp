//! Shared source fixture and recorded `rust-analyzer` document symbols for unit
//! tests.

use serde_json::json;

use crate::code::protocol::DocumentSymbol;

pub const STREAM: &str = "\
use crate::Render;

/// A stream of numbers.
///
/// Second paragraph.
#[derive(Debug, Default)]
pub struct Stream {
    /// The items.
    pub items: Vec<u32>,
    pub config: String,
}

impl Stream {
    /// Iterate over the items.
    pub fn iter(&self) -> impl Iterator<Item = &u32> {
        self.items.iter()
    }
}

impl Render for Stream {
    fn render(&self) -> String {
        let total: u32 = self.iter().sum();
        format!(\"{total} {}\", self.config)
    }
}

pub fn total(stream: &Stream) -> u32 {
    stream.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        let stream = Stream::default();
        assert_eq!(stream.iter().count(), 0);
    }
}

#[cfg(test)]
#[path = \"stream_tests.rs\"]
mod more_tests;
";

pub fn stream_lines() -> Vec<String> {
    STREAM.lines().map(str::to_owned).collect()
}

/// Recorded `textDocument/documentSymbol` response for [`STREAM`].
pub fn stream_symbols() -> Vec<DocumentSymbol> {
    serde_json::from_value(json!([
        {"name":"Stream","kind":23,"range":{"start":{"line":2,"character":0},"end":{"line":10,"character":1}},"selectionRange":{"start":{"line":6,"character":11},"end":{"line":6,"character":17}},"children":[
            {"name":"items","detail":"Vec<u32>","kind":8,"range":{"start":{"line":7,"character":4},"end":{"line":8,"character":23}},"selectionRange":{"start":{"line":8,"character":8},"end":{"line":8,"character":13}}},
            {"name":"config","detail":"String","kind":8,"range":{"start":{"line":9,"character":4},"end":{"line":9,"character":22}},"selectionRange":{"start":{"line":9,"character":8},"end":{"line":9,"character":14}}}]},
        {"name":"impl Stream","kind":19,"range":{"start":{"line":12,"character":0},"end":{"line":17,"character":1}},"selectionRange":{"start":{"line":12,"character":5},"end":{"line":12,"character":11}},"children":[
            {"name":"iter","detail":"fn(&self) -> impl Iterator<Item = &u32>","kind":6,"range":{"start":{"line":13,"character":4},"end":{"line":16,"character":5}},"selectionRange":{"start":{"line":14,"character":11},"end":{"line":14,"character":15}}}]},
        {"name":"impl Render for Stream","kind":19,"range":{"start":{"line":19,"character":0},"end":{"line":24,"character":1}},"selectionRange":{"start":{"line":19,"character":16},"end":{"line":19,"character":22}},"children":[
            {"name":"render","detail":"fn(&self) -> String","kind":6,"range":{"start":{"line":20,"character":4},"end":{"line":23,"character":5}},"selectionRange":{"start":{"line":20,"character":7},"end":{"line":20,"character":13}}}]},
        {"name":"total","detail":"fn(stream: &Stream) -> u32","kind":12,"range":{"start":{"line":26,"character":0},"end":{"line":28,"character":1}},"selectionRange":{"start":{"line":26,"character":7},"end":{"line":26,"character":12}}},
        {"name":"tests","kind":2,"range":{"start":{"line":30,"character":0},"end":{"line":39,"character":1}},"selectionRange":{"start":{"line":31,"character":4},"end":{"line":31,"character":9}},"children":[
            {"name":"counts","detail":"fn()","kind":12,"range":{"start":{"line":34,"character":4},"end":{"line":38,"character":5}},"selectionRange":{"start":{"line":35,"character":7},"end":{"line":35,"character":13}}}]},
        {"name":"more_tests","kind":2,"range":{"start":{"line":41,"character":0},"end":{"line":43,"character":15}},"selectionRange":{"start":{"line":43,"character":4},"end":{"line":43,"character":14}}}
    ]))
    .unwrap()
}
