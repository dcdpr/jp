//! Higher-level analyses built on `xml::RowReader`.

pub mod callgraph;
pub mod hotspots;
pub mod samples;

pub use callgraph::{CallgraphBuilder, CallgraphReport, FunctionStat};
pub use hotspots::{Hotspot, HotspotReport, HotspotsBuilder, SlideMode};
pub use samples::{Callstack, PcSample};
