//! Streaming parser for `xctrace export --xpath` XML results.

use quick_xml::XmlVersion;

pub mod schema;
pub mod stream;
pub mod value;

pub use schema::{Column, EngineeringType, Schema};
pub use stream::{Node, RowReader, RowReaderEvent};
pub use value::Cell;

/// Instruments' exported XML opens with `<?xml version="1.0"?>`. quick-xml
/// needs the declared version to pick its normalization rules.
pub(crate) const XML_VERSION: XmlVersion = XmlVersion::Explicit1_0;
