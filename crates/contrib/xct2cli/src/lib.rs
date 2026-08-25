//! Library + CLI for transforming Xcode Instruments traces.

pub mod address;
pub mod analysis;
pub mod error;
pub mod redact;
pub mod symbol;
pub mod trace;
pub mod xctrace;
pub mod xml;

pub use address::{CoreId, FilePc, Pid, RuntimePc, SampleTime, Slide};
pub use error::{Error, Result};
pub use trace::TraceBundle;
