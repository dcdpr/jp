//! Internal merge strategies.

mod map;
mod plain_vec;
mod string;
mod vec;

pub use map::map_with_strategy;
pub use plain_vec::append_vec_dedup;
pub use string::string_with_strategy;
pub use vec::vec_with_strategy;
