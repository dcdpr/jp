/// All token and node kinds in a JSON/JSON5 syntax tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u16)]
pub enum SyntaxKind {
    // Punctuation
    LBrace = 0,
    RBrace,
    LBracket,
    RBracket,
    Colon,
    Comma,

    // Literals
    String,
    Number,
    TrueKw,
    FalseKw,
    NullKw,

    // JSON5 extensions
    Ident,
    LineComment,
    BlockComment,

    // Trivia
    Whitespace,

    // Error recovery
    Error,

    // Composite nodes
    Root,
    Object,
    Array,
    Member,
}

impl SyntaxKind {
    /// Whether this kind is trivia (whitespace or comments) that the parser
    /// skips over between syntactically meaningful tokens.
    #[must_use]
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            Self::Whitespace | Self::LineComment | Self::BlockComment
        )
    }
}

impl From<SyntaxKind> for rowan::SyntaxKind {
    fn from(kind: SyntaxKind) -> Self {
        Self(kind as u16)
    }
}

/// Marker type for the rowan `Language` trait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum JsonLang {}

impl rowan::Language for JsonLang {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        assert!(raw.0 <= SyntaxKind::Member as u16);
        // Safety: SyntaxKind is repr(u16) and all values in range are valid variants.
        unsafe { std::mem::transmute::<u16, SyntaxKind>(raw.0) }
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        kind.into()
    }
}

pub type SyntaxNode = rowan::SyntaxNode<JsonLang>;
pub type SyntaxToken = rowan::SyntaxToken<JsonLang>;
pub type SyntaxElement = rowan::NodeOrToken<SyntaxNode, SyntaxToken>;

/// Extract the [`SyntaxKind`] from a [`SyntaxElement`].
#[must_use]
pub fn element_kind(element: &SyntaxElement) -> SyntaxKind {
    match element {
        SyntaxElement::Node(n) => n.kind(),
        SyntaxElement::Token(t) => t.kind(),
    }
}

#[cfg(test)]
#[path = "syntax_tests.rs"]
mod tests;
