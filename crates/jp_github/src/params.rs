#[derive(Debug, Clone, Copy)]
pub enum State {
    Open,
    Closed,
    All,
}

impl State {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}
