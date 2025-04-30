pub mod error;
pub mod global;
pub mod parts;
pub mod serde;

use core::fmt;

pub use error::Error;
pub use parts::Parts;
use parts::{GlobalId, TargetId, Variant};

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

    /// The global ID for this ID type.
    ///
    /// This value has to be unique across all global IDs.
    fn global_id(&self) -> GlobalId;

    /// Returns `true` if the ID is valid.
    fn is_valid(&self) -> bool {
        Self::variant().is_valid() && self.target_id().is_valid() && self.global_id().is_valid()
    }

    /// Format the ID using the following format:
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
    /// jp-pdefault-456
    /// jp-mdefault-456
    /// ```
    fn format_id(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}-{}{}-{}",
            ID_PREFIX,
            Self::variant(),
            self.target_id(),
            self.global_id()
        )
    }
}
