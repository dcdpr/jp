//! High-level facade over a `.trace` bundle.

pub mod toc;

use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use serde::Serialize;
pub use toc::{Table, Toc};

use crate::{
    error::{Error, Result},
    xctrace::Xctrace,
    xml::{
        Cell, Schema,
        stream::{RowReader, RowReaderEvent},
    },
};

/// On-disk Instruments `.trace` bundle.
/// Cheap to construct; no I/O is performed until you call a method.
#[derive(Debug, Clone)]
pub struct TraceBundle {
    path: PathBuf,
    xctrace: Xctrace,
}

impl TraceBundle {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if !path.exists() {
            return Err(Error::BundleMissing(path));
        }
        Ok(Self {
            path,
            xctrace: Xctrace::discover(),
        })
    }

    pub fn with_xctrace(mut self, xctrace: Xctrace) -> Self {
        self.xctrace = xctrace;
        self
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn xctrace(&self) -> &Xctrace {
        &self.xctrace
    }

    pub fn toc(&self) -> Result<Toc> {
        let xml = self.xctrace.export_toc(&self.path)?;
        Toc::parse(&xml)
    }

    /// Run an XPath query and return parsed rows for every `<node>`.
    pub fn query(&self, xpath: &str) -> Result<QueryResult> {
        let xml = self.xctrace.export_xpath(&self.path, xpath)?;
        QueryResult::parse(xml)
    }
}

/// In-memory snapshot of a single XPath query.
#[derive(Debug, Default, Serialize)]
pub struct QueryResult {
    pub nodes: Vec<NodeData>,
}

#[derive(Debug, Serialize)]
pub struct NodeData {
    pub xpath: Option<String>,
    pub schema: Option<Schema>,
    pub rows: Vec<Vec<Rc<Cell>>>,
}

impl QueryResult {
    pub fn parse(xml: Vec<u8>) -> Result<Self> {
        let cursor = std::io::Cursor::new(xml);
        let mut reader = RowReader::new(cursor);
        let mut result = QueryResult::default();
        let mut current: Option<NodeData> = None;
        while let Some(ev) = reader.next_event()? {
            match ev {
                RowReaderEvent::NodeStart(node) => {
                    if let Some(prev) = current.take() {
                        result.nodes.push(prev);
                    }
                    current = Some(NodeData {
                        xpath: node.xpath,
                        schema: node.schema,
                        rows: Vec::new(),
                    });
                }
                RowReaderEvent::Row(cells) => {
                    if let Some(n) = current.as_mut() {
                        n.rows.push(cells);
                    }
                }
                RowReaderEvent::NodeEnd => {
                    if let Some(n) = current.take() {
                        result.nodes.push(n);
                    }
                }
            }
        }
        if let Some(n) = current.take() {
            result.nodes.push(n);
        }
        Ok(result)
    }
}
