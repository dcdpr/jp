//! Internal merge strategies.

mod map;
mod string;
mod vec;

pub use map::map_with_strategy;
pub use string::string_with_strategy;
pub use vec::{ordered_vec_with_strategy, vec_with_strategy};
