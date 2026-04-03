use std::fmt;

use rowan::GreenNodeBuilder;

use crate::{
    error::ParseError,
    lexer::Dialect,
    parser,
    syntax::{SyntaxElement, SyntaxKind, SyntaxNode, element_kind},
};

/// A parsed JSON/JSON5 document that supports format-preserving edits.
pub struct Document {
    root: SyntaxNode,
    dialect: Dialect,
}

impl Document {
    /// Parse a JSON string into a document.
    ///
    /// Returns `Err` if the input contains syntax errors.
    pub fn parse(input: &str) -> Result<Self, Vec<ParseError>> {
        Self::parse_with_dialect(input, Dialect::Json)
    }

    /// Parse a JSON5 string into a document.
    pub fn parse_json5(input: &str) -> Result<Self, Vec<ParseError>> {
        Self::parse_with_dialect(input, Dialect::Json5)
    }

    fn parse_with_dialect(input: &str, dialect: Dialect) -> Result<Self, Vec<ParseError>> {
        let result = parser::parse(input, dialect);
        if !result.errors.is_empty() {
            return Err(result.errors);
        }
        let root = SyntaxNode::new_root_mut(result.green_node);
        Ok(Self { root, dialect })
    }

    /// Get the root value as an [`Object`], if it is one.
    #[must_use]
    pub fn as_object(&self) -> Option<Object> {
        self.root
            .children()
            .find(|c| c.kind() == SyntaxKind::Object)
            .map(|node| Object {
                node,
                dialect: self.dialect,
            })
    }

    /// Get the root value as an [`Array`], if it is one.
    #[must_use]
    pub fn as_array(&self) -> Option<Array> {
        self.root
            .children()
            .find(|c| c.kind() == SyntaxKind::Array)
            .map(|node| Array { node })
    }
}

impl fmt::Display for Document {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.root, f)
    }
}

// ---------------------------------------------------------------------------
// Object
// ---------------------------------------------------------------------------

/// A view into a JSON object node, supporting format-preserving edits.
pub struct Object {
    node: SyntaxNode,
    dialect: Dialect,
}

impl Object {
    /// Iterate over all members in this object.
    pub fn members(&self) -> impl Iterator<Item = Member> + '_ {
        self.node
            .children()
            .filter(|c| c.kind() == SyntaxKind::Member)
            .map(|node| Member { node })
    }

    /// Find the value for `key`, if present.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<Value> {
        self.find_member(key).and_then(|m| m.value())
    }

    /// Find the value for `key` as a nested [`Object`], if present.
    #[must_use]
    pub fn get_object(&self, key: &str) -> Option<Object> {
        self.find_member(key).and_then(|m| {
            let v = m.value_element()?;
            match v {
                SyntaxElement::Node(n) if n.kind() == SyntaxKind::Object => Some(Object {
                    node: n,
                    dialect: self.dialect,
                }),
                _ => None,
            }
        })
    }

    /// Set `key` to `raw_value`. If the key already exists, its value is
    /// replaced in-place. Otherwise a new member is appended.
    pub fn set(&self, key: &str, raw_value: &str) {
        if let Some(member) = self.find_member(key) {
            self.replace_member_value(&member, raw_value);
        } else {
            self.insert_member(key, raw_value);
        }
    }

    /// Remove the member with `key`. Returns `true` if it was found.
    ///
    /// # Panics
    ///
    /// Panics if internal tree indices become inconsistent (should never
    /// happen for well-formed trees).
    #[must_use]
    pub fn remove(&self, key: &str) -> bool {
        let Some(member_node) = self.find_member(key).map(|m| m.node) else {
            return false;
        };

        let kids: Vec<SyntaxElement> = self.node.children_with_tokens().collect();
        let count = kids.len();
        let Some(idx) = kids
            .iter()
            .position(|c| matches!(c, SyntaxElement::Node(n) if *n == member_node))
        else {
            return false;
        };

        let mut to_remove: Vec<usize> = vec![idx];

        // Check for trailing comma
        let mut after = idx + 1;
        while after < count && element_kind(&kids[after]).is_trivia() {
            after += 1;
        }
        if after < count && element_kind(&kids[after]) == SyntaxKind::Comma {
            to_remove.clear();
            for i in idx..=after {
                to_remove.push(i);
            }
            let mut trail = after + 1;
            while trail < count && element_kind(&kids[trail]).is_trivia() {
                to_remove.push(trail);
                trail += 1;
            }
        } else {
            // Check for leading comma
            let mut before = idx;
            while before > 0 && element_kind(&kids[before - 1]).is_trivia() {
                before -= 1;
            }
            if before > 0 && element_kind(&kids[before - 1]) == SyntaxKind::Comma {
                to_remove.clear();
                for i in before - 1..=idx {
                    to_remove.push(i);
                }
            }
        }

        // Detach in reverse order
        to_remove.sort_unstable();
        for &i in to_remove.iter().rev() {
            let child = self.node.children_with_tokens().nth(i).unwrap();
            detach_element(child);
        }
        true
    }

    fn find_member(&self, key: &str) -> Option<Member> {
        self.members().find(|m| m.key_matches(key))
    }

    fn replace_member_value(&self, member: &Member, raw_value: &str) {
        let children: Vec<SyntaxElement> = member.node.children_with_tokens().collect();

        let val_idx = children
            .iter()
            .rposition(|c| !element_kind(c).is_trivia() && element_kind(c) != SyntaxKind::Colon)
            .unwrap();

        let key_idx = children
            .iter()
            .position(|c| matches!(element_kind(c), SyntaxKind::String | SyntaxKind::Ident))
            .unwrap();

        if val_idx <= key_idx {
            let new_val = self.parse_value_fragment(raw_value);
            member
                .node
                .splice_children(children.len()..children.len(), new_val);
            return;
        }

        // Detach old value, then insert new one at same position
        let old = member.node.children_with_tokens().nth(val_idx).unwrap();
        detach_element(old);
        let new_val = self.parse_value_fragment(raw_value);
        member.node.splice_children(val_idx..val_idx, new_val);
    }

    fn insert_member(&self, key: &str, raw_value: &str) {
        let kids: Vec<SyntaxElement> = self.node.children_with_tokens().collect();
        let count = kids.len();

        // Check if the object is multiline by looking at its full text
        let obj_text = self.node.text().to_string();
        let is_multiline = obj_text.contains('\n');

        // Find the position of the last MEMBER
        let last_member_idx = kids
            .iter()
            .rposition(|c| matches!(c, SyntaxElement::Node(n) if n.kind() == SyntaxKind::Member));

        let new_member = build_member(key, raw_value, self.dialect);

        if let Some(lm_idx) = last_member_idx {
            // Insert right after the last member
            let insert_at = lm_idx + 1;
            let mut to_insert: Vec<SyntaxElement> = Vec::new();
            to_insert.push(make_token(SyntaxKind::Comma, ","));

            if is_multiline {
                let indent = infer_member_indent(&kids);
                to_insert.push(make_token(SyntaxKind::Whitespace, &indent));
            }
            to_insert.push(SyntaxElement::Node(new_member));

            self.node.splice_children(insert_at..insert_at, to_insert);
        } else {
            // Empty object: insert before }
            let rbrace_idx = kids
                .iter()
                .rposition(|c| element_kind(c) == SyntaxKind::RBrace)
                .unwrap_or(count);

            self.node
                .splice_children(rbrace_idx..rbrace_idx, vec![SyntaxElement::Node(
                    new_member,
                )]);
        }
    }

    fn parse_value_fragment(&self, raw_value: &str) -> Vec<SyntaxElement> {
        let result = parser::parse(raw_value, self.dialect);
        let root = SyntaxNode::new_root_mut(result.green_node);
        root.children_with_tokens().collect()
    }
}

fn detach_element(elem: SyntaxElement) {
    match elem {
        SyntaxElement::Node(n) => n.detach(),
        SyntaxElement::Token(t) => t.detach(),
    }
}

fn infer_member_indent(children: &[SyntaxElement]) -> String {
    for (i, child) in children.iter().enumerate() {
        if matches!(child, SyntaxElement::Node(n) if n.kind() == SyntaxKind::Member)
            && i > 0
            && let SyntaxElement::Token(t) = &children[i - 1]
            && t.kind() == SyntaxKind::Whitespace
        {
            return t.text().to_string();
        }
    }
    "\n  ".to_string()
}

// ---------------------------------------------------------------------------
// Member
// ---------------------------------------------------------------------------

/// A view into a single key-value pair within a JSON object.
pub struct Member {
    node: SyntaxNode,
}

impl Member {
    /// The raw key text, including quotes.
    #[must_use]
    pub fn raw_key(&self) -> Option<String> {
        self.key_token().map(|t| t.text().to_string())
    }

    /// The key as a plain string (quotes stripped).
    #[must_use]
    pub fn key(&self) -> Option<String> {
        self.key_token().map(|t| strip_quotes(t.text()))
    }

    /// The value element, if present.
    pub fn value(&self) -> Option<Value> {
        self.value_element().map(Value)
    }

    fn key_token(&self) -> Option<crate::syntax::SyntaxToken> {
        self.node.children_with_tokens().find_map(|c| {
            c.into_token()
                .filter(|t| matches!(t.kind(), SyntaxKind::String | SyntaxKind::Ident))
        })
    }

    fn value_element(&self) -> Option<SyntaxElement> {
        let mut found_colon = false;
        for child in self.node.children_with_tokens() {
            if !found_colon {
                if element_kind(&child) == SyntaxKind::Colon {
                    found_colon = true;
                }
                continue;
            }
            if !element_kind(&child).is_trivia() {
                return Some(child);
            }
        }
        None
    }

    fn key_matches(&self, key: &str) -> bool {
        self.key().is_some_and(|k| k == key)
    }
}

// ---------------------------------------------------------------------------
// Value
// ---------------------------------------------------------------------------

/// A reference to a JSON value within the syntax tree.
pub struct Value(SyntaxElement);

impl Value {
    /// The syntax kind of this value.
    #[must_use]
    pub fn kind(&self) -> SyntaxKind {
        element_kind(&self.0)
    }

    /// The raw text of this value.
    #[must_use]
    pub fn text(&self) -> String {
        match &self.0 {
            SyntaxElement::Node(n) => n.text().to_string(),
            SyntaxElement::Token(t) => t.text().to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Array
// ---------------------------------------------------------------------------

/// A view into a JSON array node.
pub struct Array {
    node: SyntaxNode,
}

impl Array {
    /// The number of elements in this array.
    #[must_use]
    pub fn count(&self) -> usize {
        self.elements().count()
    }

    /// Iterate over the value elements in this array.
    pub fn elements(&self) -> impl Iterator<Item = Value> + '_ {
        self.node.children_with_tokens().filter_map(|c| {
            let k = element_kind(&c);
            if k.is_trivia()
                || matches!(
                    k,
                    SyntaxKind::LBracket | SyntaxKind::RBracket | SyntaxKind::Comma
                )
            {
                None
            } else {
                Some(Value(c))
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn strip_quotes(text: &str) -> String {
    if (text.starts_with('"') && text.ends_with('"'))
        || (text.starts_with('\'') && text.ends_with('\''))
    {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

fn escape_json_key(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let n = c as u32;
                out.push_str(&format!("\\u{n:04x}"));
            }
            c => out.push(c),
        }
    }
    out
}

fn build_member(key: &str, raw_value: &str, dialect: Dialect) -> SyntaxNode {
    let escaped = escape_json_key(key);
    let src = format!("{{\"{escaped}\":{raw_value}}}");
    let result = parser::parse(&src, dialect);
    let root = SyntaxNode::new_root_mut(result.green_node);

    let object = root.first_child().expect("parsed object");
    let member = object
        .children()
        .find(|c| c.kind() == SyntaxKind::Member)
        .expect("parsed member");

    let kids: Vec<SyntaxElement> = member.children_with_tokens().collect();
    if let Some(ci) = kids
        .iter()
        .position(|c| element_kind(c) == SyntaxKind::Colon)
    {
        let has_ws = kids
            .get(ci + 1)
            .is_some_and(|c| element_kind(c) == SyntaxKind::Whitespace);
        if !has_ws {
            member.splice_children(ci + 1..ci + 1, vec![make_token(
                SyntaxKind::Whitespace,
                " ",
            )]);
        }
    }

    member
}

fn make_token(kind: SyntaxKind, text: &str) -> SyntaxElement {
    let mut b = GreenNodeBuilder::new();
    b.start_node(SyntaxKind::Root.into());
    b.token(kind.into(), text);
    b.finish_node();
    let root = SyntaxNode::new_root_mut(b.finish());
    root.first_child_or_token().unwrap()
}

#[cfg(test)]
#[path = "ast_tests.rs"]
mod tests;
