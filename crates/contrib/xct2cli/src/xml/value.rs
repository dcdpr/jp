use std::rc::Rc;

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Cell {
    /// `<sentinel/>` - schema column present but no value for this row.
    Sentinel,
    Leaf(LeafCell),
    Composite(CompositeCell),
}

#[derive(Debug, Clone, Serialize)]
pub struct LeafCell {
    pub element: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fmt: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompositeCell {
    pub element: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fmt: Option<String>,
    pub children: Vec<Rc<Cell>>,
}

impl Cell {
    pub fn element(&self) -> Option<&str> {
        match self {
            Cell::Sentinel => None,
            Cell::Leaf(l) => Some(&l.element),
            Cell::Composite(c) => Some(&c.element),
        }
    }

    pub fn id(&self) -> Option<u64> {
        match self {
            Cell::Sentinel => None,
            Cell::Leaf(l) => l.id,
            Cell::Composite(c) => c.id,
        }
    }

    pub fn fmt(&self) -> Option<&str> {
        match self {
            Cell::Sentinel => None,
            Cell::Leaf(l) => l.fmt.as_deref(),
            Cell::Composite(c) => c.fmt.as_deref(),
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Cell::Leaf(l) => Some(&l.text),
            _ => None,
        }
    }

    pub fn children(&self) -> &[Rc<Cell>] {
        match self {
            Cell::Composite(c) => &c.children,
            _ => &[],
        }
    }

    /// First descendant whose element name equals `tag`, including self.
    pub fn find(&self, tag: &str) -> Option<&Cell> {
        if self.element() == Some(tag) {
            return Some(self);
        }
        for child in self.children() {
            if let Some(hit) = child.find(tag) {
                return Some(hit);
            }
        }
        None
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.text()?.trim().parse().ok()
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.text()?.trim().parse().ok()
    }
}
