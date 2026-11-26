pub(crate) mod container;
pub(crate) mod field;
pub(crate) mod field_value;
pub(crate) mod macros;
pub(crate) mod variant;

pub use container::*;
pub use field::*;
pub use field_value::*;
pub use macros::*;
pub use variant::*;

#[derive(darling::FromMeta, Default)]
#[darling(default)]
pub struct SerdeMeta {
    pub content: Option<String>,
    pub expecting: Option<String>,
    pub tag: Option<String>,
    pub untagged: bool,
}
