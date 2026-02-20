//! The current state of the buffer.

/// The current state of the buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// We are at a block boundary, looking for the start of a new block.
    #[default]
    AtBoundary,

    /// We are buffering a "paragraph-like" block (paragraph, list, blockquote)
    /// which can be terminated by a blank line, a new block starter, or a
    /// Setext header underline.
    BufferingParagraph,

    /// We are inside an indented code block.
    InIndentedCode,

    /// We are inside a fenced code block and looking for the closing fence.
    InFencedCode {
        /// The type of fence character used.
        fence_type: FenceType,

        /// The length of the fence character.
        fence_length: usize,

        /// The indentation of the fence character.
        indent: usize,
    },

    /// We are inside an HTML block.
    InHtmlBlock {
        /// The rule for how this specific HTML block terminates.
        block_type: HtmlBlockRule,
    },
}

/// Represents the type of fence character used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceType {
    /// \`
    Backtick,

    /// ~
    Tilde,
}

impl FenceType {
    /// Returns the character corresponding to this fence type.
    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            Self::Backtick => '`',
            Self::Tilde => '~',
        }
    }
}

/// Represents the 7 types of HTML blocks defined by the CommonMark spec.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlBlockRule {
    /// Type 1: `<script>`, `<pre>`, `<style>`, `<textarea>`
    /// Ends with matching closing tag.
    Type1(HtmlType1Tag),
    /// Type 2: `<!-- ... -->`
    Type2,
    /// Type 3: `<? ... ?>`
    Type3,
    /// Type 4: `<!...>`
    Type4,
    /// Type 5: `<![CDATA[ ... ]]>`
    Type5,
    /// Type 6: `<div...` etc.
    /// Ends with a blank line.
    Type6(HtmlType6Tag),
    /// Type 7: `<foo...`
    /// Ends with a blank line, cannot interrupt a paragraph.
    Type7,
}

impl HtmlBlockRule {
    /// Get the end tag for this tag.
    pub const fn end_tag(self) -> &'static str {
        match self {
            Self::Type1(tag) => tag.end_tag(),
            Self::Type2 => "-->",
            Self::Type3 => "?>",
            Self::Type4 => ">",
            Self::Type5 => "]]>",
            Self::Type6(_) | Self::Type7 => "\n\n",
        }
    }
}

/// Represents the specific tag for an HTML Block Type 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HtmlType1Tag {
    /// `<script>`
    Script,

    /// `<pre>`
    Pre,

    /// `<style>`
    Style,

    /// `<textarea>`
    Textarea,
}

impl HtmlType1Tag {
    /// Parses a tag name (must be lowercase) into a Type 1 tag.
    pub fn from_str(tag: &str) -> Option<Self> {
        match tag {
            "script" => Some(Self::Script),
            "pre" => Some(Self::Pre),
            "style" => Some(Self::Style),
            "textarea" => Some(Self::Textarea),
            _ => None,
        }
    }

    /// Get the end tag for this tag.
    pub const fn end_tag(self) -> &'static str {
        match self {
            Self::Script => "</script>",
            Self::Pre => "</pre>",
            Self::Style => "</style>",
            Self::Textarea => "</textarea>",
        }
    }
}

/// Represents the specific tag for an HTML Block Type 6.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::allow_attributes, clippy::missing_docs_in_private_items)]
pub enum HtmlType6Tag {
    Address,
    Article,
    Aside,
    Base,
    Basefont,
    Blockquote,
    Body,
    Caption,
    Center,
    Col,
    Colgroup,
    Dd,
    Details,
    Dialog,
    Dir,
    Div,
    Dl,
    Dt,
    Fieldset,
    Figcaption,
    Figure,
    Footer,
    Form,
    Frame,
    Frameset,
    H1,
    H2,
    H3,
    H4,
    H5,
    H6,
    Head,
    Header,
    Hr,
    Html,
    Iframe,
    Legend,
    Li,
    Link,
    Main,
    Menu,
    Menuitem,
    Nav,
    Noframes,
    Ol,
    Optgroup,
    Option,
    P,
    Param,
    Search,
    Section,
    Summary,
    Table,
    Tbody,
    Td,
    Tfoot,
    Th,
    Thead,
    Title,
    Tr,
    Track,
    Ul,
}

impl HtmlType6Tag {
    /// Parses a tag name (must be lowercase) into a Type 6 tag.
    /// This is a large, but efficient, match statement.
    pub fn from_str(tag: &str) -> Option<Self> {
        match tag {
            "address" => Some(Self::Address),
            "article" => Some(Self::Article),
            "aside" => Some(Self::Aside),
            "base" => Some(Self::Base),
            "basefont" => Some(Self::Basefont),
            "blockquote" => Some(Self::Blockquote),
            "body" => Some(Self::Body),
            "caption" => Some(Self::Caption),
            "center" => Some(Self::Center),
            "col" => Some(Self::Col),
            "colgroup" => Some(Self::Colgroup),
            "dd" => Some(Self::Dd),
            "details" => Some(Self::Details),
            "dialog" => Some(Self::Dialog),
            "dir" => Some(Self::Dir),
            "div" => Some(Self::Div),
            "dl" => Some(Self::Dl),
            "dt" => Some(Self::Dt),
            "fieldset" => Some(Self::Fieldset),
            "figcaption" => Some(Self::Figcaption),
            "figure" => Some(Self::Figure),
            "footer" => Some(Self::Footer),
            "form" => Some(Self::Form),
            "frame" => Some(Self::Frame),
            "frameset" => Some(Self::Frameset),
            "h1" => Some(Self::H1),
            "h2" => Some(Self::H2),
            "h3" => Some(Self::H3),
            "h4" => Some(Self::H4),
            "h5" => Some(Self::H5),
            "h6" => Some(Self::H6),
            "head" => Some(Self::Head),
            "header" => Some(Self::Header),
            "hr" => Some(Self::Hr),
            "html" => Some(Self::Html),
            "iframe" => Some(Self::Iframe),
            "legend" => Some(Self::Legend),
            "li" => Some(Self::Li),
            "link" => Some(Self::Link),
            "main" => Some(Self::Main),
            "menu" => Some(Self::Menu),
            "menuitem" => Some(Self::Menuitem),
            "nav" => Some(Self::Nav),
            "noframes" => Some(Self::Noframes),
            "ol" => Some(Self::Ol),
            "optgroup" => Some(Self::Optgroup),
            "option" => Some(Self::Option),
            "p" => Some(Self::P),
            "param" => Some(Self::Param),
            "search" => Some(Self::Search),
            "section" => Some(Self::Section),
            "summary" => Some(Self::Summary),
            "table" => Some(Self::Table),
            "tbody" => Some(Self::Tbody),
            "td" => Some(Self::Td),
            "tfoot" => Some(Self::Tfoot),
            "th" => Some(Self::Th),
            "thead" => Some(Self::Thead),
            "title" => Some(Self::Title),
            "tr" => Some(Self::Tr),
            "track" => Some(Self::Track),
            "ul" => Some(Self::Ul),
            _ => None,
        }
    }
}
