pub mod macros;
pub mod mock;

pub type Result = std::result::Result<(), Box<dyn std::error::Error>>;
pub use test_log::test;
