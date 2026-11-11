//! Internal merge strategies.

mod map;
mod string;
mod vec;

pub use map::map_with_strategy;
pub use string::string_with_strategy;
pub use vec::{
    CanonicalItem, canonical_vec_with_strategy, ordered_vec_with_strategy, vec_with_strategy,
};
