//! Mach-O + DWARF symbolication for instruction-level drilldowns.

pub mod macho;
pub mod swift;

pub use macho::{
    BinaryInfo, ImageLoad, InlinedFrame, SlideCandidate, SymbolicatedFrame, Symbolicator,
    SymbolicatorOptions,
};
