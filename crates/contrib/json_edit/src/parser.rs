use rowan::GreenNode;

use crate::{
    error::ParseError,
    lexer::{self, Dialect},
    syntax::SyntaxKind,
};

/// The result of parsing a JSON or JSON5 document.
pub struct Parse {
    pub green_node: GreenNode,
    pub errors: Vec<ParseError>,
}

/// Parse a JSON string into a lossless syntax tree.
#[must_use]
pub fn parse(input: &str, dialect: Dialect) -> Parse {
    let tokens = lexer::lex(input, dialect);
    Parser::new(tokens, dialect).parse()
}

struct Parser<'a> {
    tokens: Vec<(SyntaxKind, &'a str)>,
    pos: usize,
    builder: rowan::GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
    /// Byte offset in the original source (for error reporting).
    offset: usize,
    dialect: Dialect,
}

impl<'a> Parser<'a> {
    fn new(tokens: Vec<(SyntaxKind, &'a str)>, dialect: Dialect) -> Self {
        Self {
            tokens,
            pos: 0,
            builder: rowan::GreenNodeBuilder::new(),
            errors: Vec::new(),
            offset: 0,
            dialect,
        }
    }

    fn parse(mut self) -> Parse {
        self.builder.start_node(SyntaxKind::Root.into());
        self.skip_trivia();
        if self.current().is_some() {
            self.parse_value();
        }
        self.skip_trivia();
        // Any remaining tokens are errors
        while self.current().is_some() {
            self.error("unexpected token after value");
            self.bump();
        }
        self.builder.finish_node();

        Parse {
            green_node: self.builder.finish(),
            errors: self.errors,
        }
    }

    fn parse_value(&mut self) {
        match self.current() {
            Some(SyntaxKind::LBrace) => self.parse_object(),
            Some(SyntaxKind::LBracket) => self.parse_array(),
            Some(
                SyntaxKind::String
                | SyntaxKind::Number
                | SyntaxKind::TrueKw
                | SyntaxKind::FalseKw
                | SyntaxKind::NullKw,
            ) => self.bump(),
            // JSON5: unquoted identifiers as values (Infinity, NaN, etc.)
            Some(SyntaxKind::Ident) if self.dialect == Dialect::Json5 => self.bump(),
            _ => {
                self.error("expected a value");
            }
        }
    }

    fn parse_object(&mut self) {
        debug_assert_eq!(self.current(), Some(SyntaxKind::LBrace));
        self.builder.start_node(SyntaxKind::Object.into());
        self.bump(); // {

        self.skip_trivia();

        // Members
        if self.current_is_member_key() {
            self.parse_member();

            loop {
                self.skip_trivia();
                if self.current() != Some(SyntaxKind::Comma) {
                    break;
                }
                self.bump(); // ,
                self.skip_trivia();

                // Trailing comma (JSON5) or missing member
                if self.current() == Some(SyntaxKind::RBrace) || self.current().is_none() {
                    break;
                }

                self.parse_member();
            }
        }

        self.skip_trivia();

        if self.current() == Some(SyntaxKind::RBrace) {
            self.bump(); // }
        } else {
            self.error("expected '}'");
        }

        self.builder.finish_node();
    }

    fn parse_member(&mut self) {
        self.builder.start_node(SyntaxKind::Member.into());

        // Key
        if self.current_is_member_key() {
            self.bump();
        } else {
            self.error("expected object key");
        }

        self.skip_trivia();

        // Colon
        if self.current() == Some(SyntaxKind::Colon) {
            self.bump();
        } else {
            self.error("expected ':'");
        }

        self.skip_trivia();

        // Value
        self.parse_value();

        self.builder.finish_node();
    }

    fn parse_array(&mut self) {
        debug_assert_eq!(self.current(), Some(SyntaxKind::LBracket));
        self.builder.start_node(SyntaxKind::Array.into());
        self.bump(); // [

        self.skip_trivia();

        // First element
        if self.current() != Some(SyntaxKind::RBracket) && self.current().is_some() {
            self.parse_value();

            loop {
                self.skip_trivia();
                if self.current() != Some(SyntaxKind::Comma) {
                    break;
                }
                self.bump(); // ,
                self.skip_trivia();

                // Trailing comma (JSON5) or end
                if self.current() == Some(SyntaxKind::RBracket) || self.current().is_none() {
                    break;
                }

                self.parse_value();
            }
        }

        self.skip_trivia();

        if self.current() == Some(SyntaxKind::RBracket) {
            self.bump(); // ]
        } else {
            self.error("expected ']'");
        }

        self.builder.finish_node();
    }

    // -- helpers --

    fn current(&self) -> Option<SyntaxKind> {
        self.tokens.get(self.pos).map(|(k, _)| *k)
    }

    fn current_is_member_key(&self) -> bool {
        matches!(self.current(), Some(SyntaxKind::String | SyntaxKind::Ident))
    }

    fn bump(&mut self) {
        if let Some(&(kind, text)) = self.tokens.get(self.pos) {
            self.builder.token(kind.into(), text);
            self.offset += text.len();
            self.pos += 1;
        }
    }

    fn skip_trivia(&mut self) {
        while let Some(kind) = self.current() {
            if !kind.is_trivia() {
                break;
            }
            self.bump();
        }
    }

    fn error(&mut self, msg: &str) {
        self.errors.push(ParseError {
            message: msg.to_owned(),
            offset: self.offset,
        });
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod tests;
