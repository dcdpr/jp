//! Concise schema DSL parser.
//!
//! Converts a compact field definition string into a JSON Schema object.
//! Accepts either the concise DSL syntax or raw JSON Schema passthrough.
//!
//! See [RFD 030](https://jp.computer/rfd/030-schema-dsl) for the full syntax
//! reference.
//!
//! # DSL Syntax
//!
//! Fields are separated by commas or newlines. Each field has the form:
//!
//! ```text
//! [?] name [type] [: description]
//! ```
//!
//! - `?` marks the field as optional (not in `required`).
//! - `name` is required — alphanumeric, hyphens, or underscores.
//! - `type` is optional; defaults to `string`.
//! - `description` is optional text after a colon.
//!
//! ## Types
//!
//! | DSL                 | JSON Schema                              |
//! |---------------------|------------------------------------------|
//! | `str` / `string`    | `{"type": "string"}`                     |
//! | `int` / `integer`   | `{"type": "integer"}`                    |
//! | `float` / `number`  | `{"type": "number"}`                     |
//! | `bool` / `boolean`  | `{"type": "boolean"}`                    |
//! | `any`               | `{}` (no constraint)                     |
//! | `[string]`          | `{"type": "array", "items": {"type":`    |
//! |                     | `"string"}}`                             |
//! | `[string\|int]`     | array with `anyOf` items                 |
//! | `[string]\|int`     | field-level union (`anyOf`)              |
//! | `{ name, age int }` | nested object                            |
//!
//! ## Descriptions
//!
//! - Inline: `summary: a brief summary` (ends at `,` or newline)
//! - Quoted: `bar: "hello, universe"` (allows commas)
//! - Heredoc: `baz: """\na longer description\n"""`
//!
//! ## Line Continuation
//!
//! A backslash at the end of a line joins it with the next:
//!
//! ```text
//! ?age \
//!       int
//! ```
//!
//! # Examples
//!
//! ```ignore
//! # use jp_cli::schema::parse_schema_dsl;
//! // Simple fields
//! let s = parse_schema_dsl("summary").unwrap();
//! let s = parse_schema_dsl("name, age int, active bool").unwrap();
//!
//! // Optional fields
//! let s = parse_schema_dsl("name, ?nickname").unwrap();
//!
//! // Arrays and nesting
//! let s = parse_schema_dsl("tags [string], address { city, zip }").unwrap();
//!
//! // JSON passthrough
//! let s = parse_schema_dsl(r#"{"type":"object","properties":{"x":{"type":"string"}}}"#).unwrap();
//! ```

use serde_json::json;

/// Parse a concise schema DSL string into a JSON Schema value.
///
/// If the input is valid JSON (starts with `{`), it is returned as-is.
pub fn parse_schema_dsl(input: &str) -> Result<serde_json::Value, ParseError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ParseError::Empty);
    }

    // JSON passthrough: try parsing as JSON first if it looks like it.
    if input.starts_with('{') {
        return serde_json::from_str::<serde_json::Value>(input).map_err(ParseError::Json);
    }

    let mut parser = Parser::new(input);
    let fields = parser.parse_field_list(None)?;
    parser.skip_separators();

    if !parser.is_eof() {
        return Err(ParseError::Unexpected {
            expected: "end of input",
            found: parser.peek(),
            pos: parser.pos,
        });
    }

    if fields.is_empty() {
        return Err(ParseError::Empty);
    }

    Ok(fields_to_json(&fields))
}

#[derive(Debug, Clone, PartialEq)]
struct SchemaField {
    name: String,
    required: bool,
    field_type: SchemaType,
    description: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum SchemaType {
    String,
    Integer,
    Number,
    Boolean,
    Any,
    /// A literal/constant value (string, number, bool, or null).
    Literal(serde_json::Value),
    Array(Box<SchemaType>),
    Object(Vec<SchemaField>),
    Union(Vec<SchemaType>),
}

impl SchemaType {
    fn to_json(&self) -> serde_json::Value {
        match self {
            Self::String => json!({"type": "string"}),
            Self::Integer => json!({"type": "integer"}),
            Self::Number => json!({"type": "number"}),
            Self::Boolean => json!({"type": "boolean"}),
            Self::Any => json!({}),
            Self::Literal(val) => json!({"const": val}),
            Self::Array(items) => json!({
                "type": "array",
                "items": items.to_json(),
            }),
            Self::Object(fields) => fields_to_json(fields),
            Self::Union(types) => union_to_json(types),
        }
    }

    fn is_literal(&self) -> bool {
        matches!(self, Self::Literal(_))
    }
}

struct Parser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> &'a str {
        self.input.get(self.pos..).unwrap_or("")
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.input.len()
    }

    fn peek(&self) -> Option<char> {
        self.remaining().chars().next()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += c.len_utf8();
        Some(c)
    }

    /// Skip spaces, tabs, and `\<newline>` line continuations. Does NOT skip
    /// bare newlines (those are field separators).
    fn skip_ws(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t') => {
                    self.pos += 1;
                }
                Some('\\') => match self.continuation_len() {
                    Some(skip) => self.pos += skip,
                    None => break,
                },
                _ => break,
            }
        }
    }

    /// Skip field separators: spaces, tabs, commas, newlines, and `\<newline>`
    /// continuations.
    fn skip_separators(&mut self) {
        loop {
            match self.peek() {
                Some(' ' | '\t' | ',' | '\n') => {
                    self.pos += 1;
                }
                Some('\\') => match self.continuation_len() {
                    Some(skip) => self.pos += skip,
                    None => break,
                },
                _ => break,
            }
        }
    }

    /// If the current position starts a line continuation (`\` then optional
    /// horizontal whitespace then `\n`), return the byte count to skip.
    /// Otherwise `None`.
    fn continuation_len(&self) -> Option<usize> {
        let rest = self.remaining();
        if !rest.starts_with('\\') {
            return None;
        }
        let after = &rest[1..];
        let ws = after
            .bytes()
            .take_while(|b| *b == b' ' || *b == b'\t')
            .count();
        if after.get(ws..).is_some_and(|s| s.starts_with('\n')) {
            Some(1 + ws + 1) // \ + spaces/tabs + \n
        } else {
            None
        }
    }

    /// Read an unquoted field name. Accepts any character that isn't reserved
    /// by the grammar.
    fn read_name(&mut self) -> Option<&'a str> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if is_name_char(c) {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if self.pos > start {
            Some(&self.input[start..self.pos])
        } else {
            None
        }
    }

    /// Read a field name, which can be unquoted or quoted.
    fn read_field_name(&mut self) -> Result<Option<String>, ParseError> {
        if self.peek() == Some('"') {
            self.parse_quoted_string().map(Some)
        } else {
            Ok(self.read_name().map(str::to_owned))
        }
    }

    /// Read a keyword-style word: `[a-zA-Z0-9_-]+`. Used for type keywords.
    fn read_keyword(&mut self) -> Option<&'a str> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                self.pos += 1;
            } else {
                break;
            }
        }
        if self.pos > start {
            Some(&self.input[start..self.pos])
        } else {
            None
        }
    }

    /// Peek at the next keyword-style word without consuming it.
    fn peek_keyword(&self) -> &'a str {
        let r = self.remaining();
        let end = r
            .bytes()
            .position(|b| !b.is_ascii_alphanumeric() && b != b'_' && b != b'-')
            .unwrap_or(r.len());
        &r[..end]
    }

    fn parse_field_list(
        &mut self,
        terminator: Option<char>,
    ) -> Result<Vec<SchemaField>, ParseError> {
        let mut fields = Vec::new();

        loop {
            self.skip_separators();

            if self.is_eof() {
                break;
            }
            if terminator.is_some_and(|t| self.peek() == Some(t)) {
                break;
            }

            fields.push(self.parse_field(terminator)?);
        }

        Ok(fields)
    }

    fn parse_field(&mut self, terminator: Option<char>) -> Result<SchemaField, ParseError> {
        self.skip_ws();

        let required = if self.peek() == Some('?') {
            self.advance();
            false
        } else {
            true
        };

        self.skip_ws();

        let name_pos = self.pos;
        let name = self
            .read_field_name()?
            .ok_or(ParseError::ExpectedFieldName { pos: name_pos })?;

        if name.is_empty() {
            return Err(ParseError::ExpectedFieldName { pos: name_pos });
        }

        self.skip_ws();

        let field_type = self.try_parse_type_expr()?.unwrap_or(SchemaType::String);

        self.skip_ws();

        let description = if self.peek() == Some(':') {
            self.advance();
            self.skip_ws();
            let desc = self.parse_description(terminator)?;
            if desc.is_empty() { None } else { Some(desc) }
        } else {
            None
        };

        Ok(SchemaField {
            name,
            required,
            field_type,
            description,
        })
    }

    /// Try to parse a type expression. Returns `None` if the next token doesn't
    /// look like a type (e.g. it's `:`, `,`, `\n`, or EOF).
    ///
    /// If the next token is a word that is NOT a type keyword, returns an error
    /// - within a field definition, a word after the name must be a valid type.
    fn try_parse_type_expr(&mut self) -> Result<Option<SchemaType>, ParseError> {
        match self.peek() {
            Some('[' | '{' | '"') => Ok(Some(self.parse_type_expr()?)),
            // Number literal: `field 42` or `field -1`
            Some(c) if c.is_ascii_digit() || c == '-' => Ok(Some(self.parse_type_expr()?)),
            Some(c) if c.is_ascii_alphabetic() => {
                let word = self.peek_keyword();
                if is_type_keyword(word) || is_literal_keyword(word) {
                    Ok(Some(self.parse_type_expr()?))
                } else if !word.is_empty() {
                    Err(ParseError::UnknownType {
                        given: word.to_owned(),
                        pos: self.pos,
                    })
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    /// Parse a type expression, including union (`|`).
    fn parse_type_expr(&mut self) -> Result<SchemaType, ParseError> {
        let first = self.parse_base_type()?;

        self.skip_ws();
        if self.peek() != Some('|') {
            return Ok(first);
        }

        let mut types = vec![first];
        while self.peek() == Some('|') {
            self.advance();
            self.skip_ws();
            types.push(self.parse_base_type()?);
            self.skip_ws();
        }

        Ok(SchemaType::Union(types))
    }

    fn parse_base_type(&mut self) -> Result<SchemaType, ParseError> {
        self.skip_ws();
        match self.peek() {
            Some('[') => self.parse_array_type(),
            Some('{') => self.parse_object_type(),
            Some('"') => self.parse_string_literal(),
            Some(c) if c.is_ascii_digit() || c == '-' => self.parse_number_literal(),
            _ => self.parse_primitive_or_literal_keyword(),
        }
    }

    fn parse_array_type(&mut self) -> Result<SchemaType, ParseError> {
        let open_pos = self.pos;
        self.advance(); // skip [

        self.skip_ws();
        if self.peek() == Some(']') {
            // bare [] is sugar for [any]
            self.advance();
            return Ok(SchemaType::Array(Box::new(SchemaType::Any)));
        }

        let items = self.parse_type_expr()?;
        self.skip_ws();

        if self.peek() != Some(']') {
            return Err(ParseError::Unterminated {
                kind: "array",
                pos: open_pos,
            });
        }
        self.advance();

        Ok(SchemaType::Array(Box::new(items)))
    }

    fn parse_object_type(&mut self) -> Result<SchemaType, ParseError> {
        let open_pos = self.pos;
        self.advance(); // skip {

        let fields = self.parse_field_list(Some('}'))?;
        self.skip_ws();

        if self.peek() != Some('}') {
            return Err(ParseError::Unterminated {
                kind: "object",
                pos: open_pos,
            });
        }
        self.advance();

        if fields.is_empty() {
            return Err(ParseError::EmptyObject { pos: open_pos });
        }

        Ok(SchemaType::Object(fields))
    }

    fn parse_primitive_or_literal_keyword(&mut self) -> Result<SchemaType, ParseError> {
        let pos = self.pos;
        let word = self
            .read_keyword()
            .ok_or(ParseError::ExpectedType { pos })?;

        match word {
            "str" | "string" => Ok(SchemaType::String),
            "int" | "integer" => Ok(SchemaType::Integer),
            "float" | "number" => Ok(SchemaType::Number),
            "bool" | "boolean" => Ok(SchemaType::Boolean),
            "any" => Ok(SchemaType::Any),
            "true" => Ok(SchemaType::Literal(serde_json::Value::Bool(true))),
            "false" => Ok(SchemaType::Literal(serde_json::Value::Bool(false))),
            "null" => Ok(SchemaType::Literal(serde_json::Value::Null)),
            _ => Err(ParseError::UnknownType {
                given: word.to_owned(),
                pos,
            }),
        }
    }

    fn parse_string_literal(&mut self) -> Result<SchemaType, ParseError> {
        let s = self.parse_quoted_string()?;
        Ok(SchemaType::Literal(serde_json::Value::String(s)))
    }

    fn parse_number_literal(&mut self) -> Result<SchemaType, ParseError> {
        let pos = self.pos;
        let start = self.pos;

        // Optional leading minus.
        if self.peek() == Some('-') {
            self.advance();
        }

        // Integer part (at least one digit required).
        if !self.peek().is_some_and(|c| c.is_ascii_digit()) {
            return Err(ParseError::ExpectedType { pos });
        }
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.advance();
        }

        // Optional fractional part.
        let is_float = if self.peek() == Some('.')
            && self
                .input
                .get(self.pos + 1..)
                .and_then(|s| s.chars().next())
                .is_some_and(|c| c.is_ascii_digit())
        {
            self.advance(); // skip '.'
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.advance();
            }
            true
        } else {
            false
        };

        let raw = &self.input[start..self.pos];
        let val = if is_float {
            raw.parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(serde_json::Value::Number)
        } else {
            raw.parse::<i64>()
                .ok()
                .map(serde_json::Number::from)
                .map(serde_json::Value::Number)
        };

        match val {
            Some(v) => Ok(SchemaType::Literal(v)),
            None => Err(ParseError::InvalidLiteral {
                raw: raw.to_owned(),
                pos,
            }),
        }
    }

    fn parse_description(&mut self, terminator: Option<char>) -> Result<String, ParseError> {
        if self.remaining().starts_with(r#"""""#) {
            return self.parse_heredoc();
        }
        if self.peek() == Some('"') {
            return self.parse_quoted_string();
        }
        Ok(self.parse_inline_description(terminator))
    }

    fn parse_heredoc(&mut self) -> Result<String, ParseError> {
        let open_pos = self.pos;
        self.pos += 3; // skip opening """

        // Skip a single newline after the opening delimiter.
        if self.peek() == Some('\n') {
            self.pos += 1;
        }

        let start = self.pos;
        loop {
            if self.is_eof() {
                return Err(ParseError::Unterminated {
                    kind: "heredoc",
                    pos: open_pos,
                });
            }
            if self.remaining().starts_with(r#"""""#) {
                let content = self.input[start..self.pos].trim_end_matches('\n');
                self.pos += 3;
                return Ok(content.to_owned());
            }
            self.advance();
        }
    }

    fn parse_quoted_string(&mut self) -> Result<String, ParseError> {
        let open_pos = self.pos;
        self.advance(); // skip opening "
        let start = self.pos;

        loop {
            match self.advance() {
                None => {
                    return Err(ParseError::Unterminated {
                        kind: "quoted string",
                        pos: open_pos,
                    });
                }
                Some('"') => {
                    return Ok(self.input[start..self.pos - 1].to_owned());
                }
                Some('\\') => {
                    self.advance(); // skip escaped char
                }
                Some(_) => {}
            }
        }
    }

    fn parse_inline_description(&mut self, terminator: Option<char>) -> String {
        let mut desc = std::string::String::new();

        loop {
            match self.peek() {
                None | Some('\n' | ',') => break,
                Some(c) if terminator == Some(c) => break,
                Some('\\') => {
                    // Line continuation inside description text.
                    if let Some(skip) = self.continuation_len() {
                        self.pos += skip;
                        // Trim trailing whitespace before the \, then join with
                        // a single space.
                        let trimmed = desc.trim_end().len();
                        desc.truncate(trimmed);
                        desc.push(' ');
                        // Skip leading whitespace on the continued line.
                        while matches!(self.peek(), Some(' ' | '\t')) {
                            self.pos += 1;
                        }
                    } else {
                        desc.push('\\');
                        self.advance();
                    }
                }
                Some(c) => {
                    desc.push(c);
                    self.advance();
                }
            }
        }

        desc.trim().to_owned()
    }
}

fn fields_to_json(fields: &[SchemaField]) -> serde_json::Value {
    let mut properties = serde_json::Map::new();
    let mut required: Vec<serde_json::Value> = Vec::new();

    for field in fields {
        let mut prop = field.field_type.to_json();
        if let Some(desc) = &field.description
            && let serde_json::Value::Object(ref mut map) = prop
        {
            map.insert("description".into(), json!(desc));
        }

        if field.required {
            required.push(json!(field.name));
        }

        properties.insert(field.name.clone(), prop);
    }

    let mut schema = serde_json::Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), serde_json::Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }

    serde_json::Value::Object(schema)
}

/// Convert a union to JSON Schema, optimizing all-literal unions to `enum`.
fn union_to_json(types: &[SchemaType]) -> serde_json::Value {
    if types.iter().all(SchemaType::is_literal) {
        // All literals: use the more widely supported `enum` form.
        let values: Vec<&serde_json::Value> = types
            .iter()
            .map(|t| match t {
                SchemaType::Literal(v) => v,
                _ => unreachable!(),
            })
            .collect();
        json!({ "enum": values })
    } else {
        // Mixed types and literals: use `anyOf`.
        json!({
            "anyOf": types.iter().map(SchemaType::to_json).collect::<Vec<_>>(),
        })
    }
}

/// Characters reserved by the grammar that cannot appear in unquoted field
/// names: whitespace, and `, : [ ] { } | ? \ "`.
fn is_name_char(c: char) -> bool {
    !matches!(
        c,
        ' ' | '\t' | '\n' | ',' | ':' | '[' | ']' | '{' | '}' | '|' | '?' | '\\' | '"'
    )
}

fn is_type_keyword(word: &str) -> bool {
    matches!(
        word,
        "str" | "string" | "int" | "integer" | "float" | "number" | "bool" | "boolean" | "any"
    )
}

fn is_literal_keyword(word: &str) -> bool {
    matches!(word, "true" | "false" | "null")
}

#[derive(Debug)]
pub enum ParseError {
    /// Input was empty or contained no fields.
    Empty,

    /// Input looked like JSON but failed to parse.
    Json(serde_json::Error),

    /// Expected a specific token but found something else.
    Unexpected {
        expected: &'static str,
        found: Option<char>,
        pos: usize,
    },

    /// Expected a field name.
    ExpectedFieldName { pos: usize },

    /// Expected a type expression.
    ExpectedType { pos: usize },

    /// An unrecognized type keyword.
    UnknownType { given: String, pos: usize },

    /// A number literal could not be parsed.
    InvalidLiteral { raw: String, pos: usize },

    /// An object type with no fields (not supported by LLM strict mode).
    EmptyObject { pos: usize },

    /// Unterminated delimiter.
    Unterminated { kind: &'static str, pos: usize },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "schema definition is empty"),
            Self::Json(e) => write!(f, "invalid JSON schema: {e}"),
            Self::Unexpected {
                expected,
                found: Some(c),
                pos,
            } => write!(f, "expected {expected}, found '{c}' at position {pos}"),
            Self::Unexpected {
                expected,
                found: None,
                pos,
            } => write!(
                f,
                "expected {expected}, found end of input at position {pos}"
            ),
            Self::ExpectedFieldName { pos } => {
                write!(f, "expected field name at position {pos}")
            }
            Self::ExpectedType { pos } => write!(f, "expected type at position {pos}"),
            Self::UnknownType { given, pos } => write!(
                f,
                "unknown type '{given}' as position {pos} (expected: str, int, float, bool, any, \
                 or a literal value)"
            ),
            Self::InvalidLiteral { raw, pos } => {
                write!(f, "invalid number literal '{raw}' at position {pos}")
            }
            Self::EmptyObject { pos } => write!(
                f,
                "empty object types at position {pos} are not supported (LLM strict mode requires \
                 explicit fields)"
            ),
            Self::Unterminated { kind, pos } => {
                write!(f, "unterminated {kind} starting at position {pos}")
            }
        }
    }
}

impl std::error::Error for ParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(e) => Some(e),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "schema_tests.rs"]
mod tests;
