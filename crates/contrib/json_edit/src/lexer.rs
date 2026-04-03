use crate::syntax::SyntaxKind;

/// Whether to parse strict JSON or the JSON5 superset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Json,
    Json5,
}

/// Tokenize `input` into a sequence of `(SyntaxKind, slice)` pairs.
///
/// Every byte of the input is covered by exactly one token, so
/// concatenating all slices reproduces the original input.
#[must_use]
pub fn lex(input: &str, dialect: Dialect) -> Vec<(SyntaxKind, &str)> {
    let mut tokens = Vec::new();
    let bytes = input.as_bytes();
    let mut pos = 0;

    while pos < bytes.len() {
        let start = pos;
        let kind = lex_token(bytes, &mut pos, dialect);
        tokens.push((kind, &input[start..pos]));
    }

    tokens
}

fn lex_token(bytes: &[u8], pos: &mut usize, dialect: Dialect) -> SyntaxKind {
    let b = bytes[*pos];
    match b {
        b'{' => {
            *pos += 1;
            SyntaxKind::LBrace
        }
        b'}' => {
            *pos += 1;
            SyntaxKind::RBrace
        }
        b'[' => {
            *pos += 1;
            SyntaxKind::LBracket
        }
        b']' => {
            *pos += 1;
            SyntaxKind::RBracket
        }
        b':' => {
            *pos += 1;
            SyntaxKind::Colon
        }
        b',' => {
            *pos += 1;
            SyntaxKind::Comma
        }
        b'"' => lex_string(bytes, pos, b'"'),
        b'\'' if dialect == Dialect::Json5 => lex_string(bytes, pos, b'\''),
        b'/' if dialect == Dialect::Json5 => lex_comment(bytes, pos),
        b if is_whitespace(b) => lex_whitespace(bytes, pos),
        b'-' | b'0'..=b'9' => lex_number(bytes, pos, dialect),
        b'+' if dialect == Dialect::Json5 => lex_number(bytes, pos, dialect),
        b'.' if dialect == Dialect::Json5 && peek_digit(bytes, *pos + 1) => {
            lex_number(bytes, pos, dialect)
        }
        b't' if starts_with(bytes, *pos, b"true") && !is_ident_continue_at(bytes, *pos + 4) => {
            *pos += 4;
            SyntaxKind::TrueKw
        }
        b'f' if starts_with(bytes, *pos, b"false") && !is_ident_continue_at(bytes, *pos + 5) => {
            *pos += 5;
            SyntaxKind::FalseKw
        }
        b'n' if starts_with(bytes, *pos, b"null") && !is_ident_continue_at(bytes, *pos + 4) => {
            *pos += 4;
            SyntaxKind::NullKw
        }
        b if dialect == Dialect::Json5 && is_ident_start(b) => lex_ident(bytes, pos),
        _ => {
            // Consume one byte as an error token. If it's a multi-byte UTF-8
            // char, consume the whole char so we don't split in the middle.
            let c_len = utf8_char_len(bytes, *pos);
            *pos += c_len;
            SyntaxKind::Error
        }
    }
}

fn lex_whitespace(bytes: &[u8], pos: &mut usize) -> SyntaxKind {
    while *pos < bytes.len() && is_whitespace(bytes[*pos]) {
        *pos += 1;
    }
    SyntaxKind::Whitespace
}

fn is_whitespace(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | 0x0B)
}

/// Lex a double- or single-quoted string. The opening quote has not been
/// consumed yet.
fn lex_string(bytes: &[u8], pos: &mut usize, quote: u8) -> SyntaxKind {
    *pos += 1; // opening quote
    loop {
        if *pos >= bytes.len() {
            return SyntaxKind::String; // unterminated
        }
        match bytes[*pos] {
            b'\\' => {
                *pos += 1; // backslash
                if *pos < bytes.len() {
                    // Skip escaped character. For \uXXXX, \xXX etc we just
                    // consume one byte here; the rest are consumed as normal
                    // characters in the next iterations.
                    let c_len = utf8_char_len(bytes, *pos);
                    *pos += c_len;
                }
            }
            b'\n' | b'\r' => {
                // In standard JSON, unescaped newlines in strings are invalid.
                // We still produce a String token (parser can report the error).
                return SyntaxKind::String;
            }
            b if b == quote => {
                *pos += 1; // closing quote
                return SyntaxKind::String;
            }
            _ => {
                let c_len = utf8_char_len(bytes, *pos);
                *pos += c_len;
            }
        }
    }
}

fn lex_number(bytes: &[u8], pos: &mut usize, dialect: Dialect) -> SyntaxKind {
    // Optional sign
    if *pos < bytes.len() && (bytes[*pos] == b'-' || bytes[*pos] == b'+') {
        *pos += 1;
    }

    // Hex literal (JSON5): 0x...
    if dialect == Dialect::Json5
        && *pos + 1 < bytes.len()
        && bytes[*pos] == b'0'
        && (bytes[*pos + 1] == b'x' || bytes[*pos + 1] == b'X')
    {
        *pos += 2;
        while *pos < bytes.len() && bytes[*pos].is_ascii_hexdigit() {
            *pos += 1;
        }
        return SyntaxKind::Number;
    }

    // Integer part (may be empty for JSON5 leading-dot numbers)
    while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
        *pos += 1;
    }

    // Fraction
    if *pos < bytes.len() && bytes[*pos] == b'.' {
        *pos += 1;
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }

    // Exponent
    if *pos < bytes.len() && (bytes[*pos] == b'e' || bytes[*pos] == b'E') {
        *pos += 1;
        if *pos < bytes.len() && (bytes[*pos] == b'+' || bytes[*pos] == b'-') {
            *pos += 1;
        }
        while *pos < bytes.len() && bytes[*pos].is_ascii_digit() {
            *pos += 1;
        }
    }

    SyntaxKind::Number
}

fn lex_comment(bytes: &[u8], pos: &mut usize) -> SyntaxKind {
    debug_assert_eq!(bytes[*pos], b'/');
    if *pos + 1 < bytes.len() && bytes[*pos + 1] == b'/' {
        // Line comment
        *pos += 2;
        while *pos < bytes.len() && bytes[*pos] != b'\n' {
            *pos += 1;
        }
        SyntaxKind::LineComment
    } else if *pos + 1 < bytes.len() && bytes[*pos + 1] == b'*' {
        // Block comment
        *pos += 2;
        loop {
            if *pos + 1 >= bytes.len() {
                *pos = bytes.len();
                break; // unterminated
            }
            if bytes[*pos] == b'*' && bytes[*pos + 1] == b'/' {
                *pos += 2;
                break;
            }
            *pos += 1;
        }
        SyntaxKind::BlockComment
    } else {
        *pos += 1;
        SyntaxKind::Error
    }
}

fn lex_ident(bytes: &[u8], pos: &mut usize) -> SyntaxKind {
    debug_assert!(is_ident_start(bytes[*pos]));
    *pos += 1;
    while *pos < bytes.len() && is_ident_continue(bytes[*pos]) {
        *pos += 1;
    }
    SyntaxKind::Ident
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

fn is_ident_continue_at(bytes: &[u8], pos: usize) -> bool {
    pos < bytes.len() && is_ident_continue(bytes[pos])
}

fn starts_with(bytes: &[u8], pos: usize, prefix: &[u8]) -> bool {
    bytes.get(pos..pos + prefix.len()) == Some(prefix)
}

fn peek_digit(bytes: &[u8], pos: usize) -> bool {
    pos < bytes.len() && bytes[pos].is_ascii_digit()
}

fn utf8_char_len(bytes: &[u8], pos: usize) -> usize {
    if pos >= bytes.len() {
        return 0;
    }
    let b = bytes[pos];
    if b < 0x80 {
        1
    } else if b < 0xE0 {
        2
    } else if b < 0xF0 {
        3
    } else {
        4
    }
    .min(bytes.len() - pos)
}

#[cfg(test)]
#[path = "lexer_tests.rs"]
mod tests;
