pub mod error;
pub mod parts;
pub mod serde;

use core::fmt;

pub use error::Error;
pub use parts::Parts;
use parts::{TargetId, Variant};

const ID_PREFIX: &str = "jp";
pub const NANOSECONDS_PER_DECISECOND: u32 = 100_000_000;

pub fn parse<T: Id>(s: &str) -> Result<Parts, Error> {
    Parts::parse_with_variant(s, T::variant())
}

/// A trait for homogeneous ID types.
pub trait Id: fmt::Display {
    /// The variant character for this ID type.
    ///
    /// This value groups IDs of the same type together.
    fn variant() -> Variant;

    /// The target ID for this ID type.
    ///
    /// This value has to be unique for a given global ID and variant.
    fn target_id(&self) -> TargetId;

    /// Returns `true` if the ID is valid.
    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.target_id().is_valid()
    }

    /// Format the ID using the following format:
    ///
    /// ```text
    /// jp-{variant}{target_id}
    /// ```
    ///
    /// For example:
    ///
    /// ```text
    /// jp-c17457886043
    /// jp-pdefault
    /// ```
    fn format_id(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}{}", ID_PREFIX, Self::variant(), self.target_id())
    }

    /// Format the ID with a global ID suffix:
    ///
    /// ```text
    /// jp-{variant}{target_id}-{global_id}
    /// ```
    ///
    /// For example:
    ///
    /// ```text
    /// jp-c17457886043-otvo8
    /// jp-pdefault-123
    /// ```
    fn format_full(&self, global_id: &str) -> String {
        format!(
            "{}-{}{}-{}",
            ID_PREFIX,
            Self::variant(),
            self.target_id(),
            global_id
        )
    }
}
