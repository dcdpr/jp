use std::{
    collections::{HashMap, VecDeque},
    io::BufRead,
    rc::Rc,
};

use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};

use crate::{
    error::{Error, Result},
    xml::{
        XML_VERSION,
        schema::{Column, EngineeringType, Schema},
        value::{Cell, CompositeCell, LeafCell},
    },
};

#[derive(Debug)]
pub struct Node {
    pub xpath: Option<String>,
    pub schema: Option<Schema>,
}

#[derive(Debug)]
pub enum RowReaderEvent {
    NodeStart(Node),
    Row(Vec<Rc<Cell>>),
    NodeEnd,
}

/// Result of `read_node_prelude`: optional schema, plus the first row if we had
/// to consume one to know the prelude was over.
type NodePrelude = (Option<Schema>, Option<Vec<Rc<Cell>>>);

pub struct RowReader<R: BufRead> {
    reader: Reader<R>,
    buf: Vec<u8>,
    ids: HashMap<u64, Rc<Cell>>,
    pending: VecDeque<RowReaderEvent>,
    seen_root: bool,
    in_node: bool,
}

impl<R: BufRead> RowReader<R> {
    pub fn new(reader: R) -> Self {
        let mut r = Reader::from_reader(reader);
        r.config_mut().trim_text(false);
        Self {
            reader: r,
            buf: Vec::with_capacity(4096),
            ids: HashMap::new(),
            pending: VecDeque::new(),
            seen_root: false,
            in_node: false,
        }
    }

    pub fn next_event(&mut self) -> Result<Option<RowReaderEvent>> {
        if let Some(ev) = self.pending.pop_front() {
            if matches!(ev, RowReaderEvent::NodeEnd) {
                self.in_node = false;
            }
            return Ok(Some(ev));
        }
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Eof => return Ok(None),
                Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => continue,
                Event::Text(_) | Event::CData(_) => continue,
                Event::Start(start) => {
                    let owned = start.into_owned();
                    let name = local_name(&owned)?;
                    if !self.seen_root {
                        if name != "trace-query-result" {
                            return Err(Error::Schema(format!(
                                "expected <trace-query-result> root, got <{name}>"
                            )));
                        }
                        self.seen_root = true;
                        continue;
                    }
                    if !self.in_node && name == "node" {
                        let xpath = attr_string(&owned, b"xpath")?;
                        let (schema, first_row) = self.read_node_prelude()?;
                        self.in_node = true;
                        if let Some(row) = first_row {
                            self.pending.push_back(RowReaderEvent::Row(row));
                        }
                        return Ok(Some(RowReaderEvent::NodeStart(Node { xpath, schema })));
                    }
                    if self.in_node && name == "row" {
                        let cells = self.read_row(&name)?;
                        return Ok(Some(RowReaderEvent::Row(cells)));
                    }
                    return Err(Error::Schema(format!(
                        "unexpected <{name}> at top level (in_node={})",
                        self.in_node
                    )));
                }
                Event::End(end) => {
                    let qn = end.name();
                    let name = std::str::from_utf8(qn.as_ref())?;
                    if name == "node" && self.in_node {
                        self.in_node = false;
                        return Ok(Some(RowReaderEvent::NodeEnd));
                    }
                    if name == "trace-query-result" {
                        return Ok(None);
                    }
                }
                Event::Empty(start) => {
                    let owned = start.into_owned();
                    let name = local_name(&owned)?;
                    if self.in_node && name == "row" {
                        return Ok(Some(RowReaderEvent::Row(Vec::new())));
                    }
                }
                _ => {}
            }
        }
    }

    fn read_node_prelude(&mut self) -> Result<NodePrelude> {
        let mut schema: Option<Schema> = None;
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Text(_) | Event::CData(_) | Event::Comment(_) | Event::PI(_) => continue,
                Event::Start(start) => {
                    let owned = start.into_owned();
                    let name = local_name(&owned)?;
                    if name == "schema" {
                        schema = Some(self.read_schema_body(&owned)?);
                        continue;
                    }
                    if name == "row" {
                        let row = self.read_row(&name)?;
                        return Ok((schema, Some(row)));
                    }
                    return Err(Error::Schema(format!(
                        "expected <schema> or <row> inside <node>, got <{name}>"
                    )));
                }
                Event::Empty(start) => {
                    let owned = start.into_owned();
                    let name = local_name(&owned)?;
                    if name == "schema" {
                        schema = Some(Schema {
                            name: attr_string(&owned, b"name")?
                                .ok_or_else(|| Error::Schema("schema missing name".into()))?,
                            documentation: attr_string(&owned, b"documentation")?,
                            columns: Vec::new(),
                        });
                        continue;
                    }
                    if name == "row" {
                        return Ok((schema, Some(Vec::new())));
                    }
                }
                Event::End(end) => {
                    let qn = end.name();
                    let n = std::str::from_utf8(qn.as_ref())?;
                    if n == "node" {
                        self.pending.push_back(RowReaderEvent::NodeEnd);
                    }
                    return Ok((schema, None));
                }
                Event::Eof => return Ok((schema, None)),
                _ => {}
            }
        }
    }

    fn read_schema_body(&mut self, start: &BytesStart<'_>) -> Result<Schema> {
        let name = attr_string(start, b"name")?
            .ok_or_else(|| Error::Schema("schema missing name".into()))?;
        let documentation = attr_string(start, b"documentation")?;
        let mut columns: Vec<Column> = Vec::new();
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Start(s) => {
                    let owned = s.into_owned();
                    if local_name(&owned)? == "col" {
                        columns.push(self.read_column()?);
                    }
                }
                Event::End(e) => {
                    let qn = e.name();
                    if std::str::from_utf8(qn.as_ref())? == "schema" {
                        break;
                    }
                }
                Event::Eof => return Err(Error::Schema("EOF inside <schema>".into())),
                _ => {}
            }
        }
        Ok(Schema {
            name,
            documentation,
            columns,
        })
    }

    fn read_column(&mut self) -> Result<Column> {
        let mut mnemonic = String::new();
        let mut name = String::new();
        let mut engineering_type = String::new();
        let mut current: Option<&'static str> = None;
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Start(s) => {
                    let owned = s.into_owned();
                    let n = local_name(&owned)?;
                    current = match n.as_str() {
                        "mnemonic" => Some("mnemonic"),
                        "name" => Some("name"),
                        "engineering-type" => Some("engineering-type"),
                        _ => None,
                    };
                }
                Event::Text(t) => {
                    let txt = t.xml_content(XML_VERSION)?;
                    match current {
                        Some("mnemonic") => mnemonic.push_str(&txt),
                        Some("name") => name.push_str(&txt),
                        Some("engineering-type") => engineering_type.push_str(&txt),
                        _ => {}
                    }
                }
                Event::End(e) => {
                    let qn = e.name();
                    let n = std::str::from_utf8(qn.as_ref())?;
                    if n == "col" {
                        break;
                    }
                    current = None;
                }
                Event::Eof => return Err(Error::Schema("EOF inside <col>".into())),
                _ => {}
            }
        }
        Ok(Column {
            mnemonic,
            name,
            engineering_type: EngineeringType::from(engineering_type),
        })
    }

    fn read_row(&mut self, row_tag: &str) -> Result<Vec<Rc<Cell>>> {
        let mut cells: Vec<Rc<Cell>> = Vec::new();
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Start(s) => {
                    let owned = s.into_owned();
                    cells.push(self.parse_cell_start(&owned)?);
                }
                Event::Empty(s) => {
                    let owned = s.into_owned();
                    cells.push(self.parse_cell_empty(&owned)?);
                }
                Event::End(e) => {
                    let qn = e.name();
                    let n = std::str::from_utf8(qn.as_ref())?;
                    if n == row_tag {
                        break;
                    }
                }
                Event::Eof => return Err(Error::Schema("EOF inside <row>".into())),
                _ => {}
            }
        }
        Ok(cells)
    }

    fn parse_cell_start(&mut self, start: &BytesStart<'_>) -> Result<Rc<Cell>> {
        let name = local_name(start)?;
        let id = attr_u64(start, b"id")?;
        let fmt = attr_string(start, b"fmt")?;
        if let Some(r) = attr_u64(start, b"ref")? {
            self.consume_until_end(&name)?;
            return self.lookup_ref(r);
        }
        let mut text = String::new();
        let mut children: Vec<Rc<Cell>> = Vec::new();
        loop {
            self.buf.clear();
            let ev = self.reader.read_event_into(&mut self.buf)?;
            match ev {
                Event::Start(s) => {
                    let owned = s.into_owned();
                    children.push(self.parse_cell_start(&owned)?);
                }
                Event::Empty(s) => {
                    let owned = s.into_owned();
                    children.push(self.parse_cell_empty(&owned)?);
                }
                Event::Text(t) => text.push_str(&t.xml_content(XML_VERSION)?),
                Event::CData(t) => text.push_str(std::str::from_utf8(&t)?),
                Event::End(e) => {
                    let qn = e.name();
                    let n = std::str::from_utf8(qn.as_ref())?;
                    if n == name {
                        break;
                    }
                    return Err(Error::Schema(format!(
                        "mismatched close </{n}> while reading <{name}>"
                    )));
                }
                Event::Eof => return Err(Error::Schema(format!("EOF inside <{name}>"))),
                _ => {}
            }
        }
        let cell = if children.is_empty() {
            Cell::Leaf(LeafCell {
                element: name,
                id,
                fmt,
                text,
            })
        } else {
            Cell::Composite(CompositeCell {
                element: name,
                id,
                fmt,
                children,
            })
        };
        let rc = Rc::new(cell);
        if let Some(i) = id {
            self.ids.insert(i, rc.clone());
        }
        Ok(rc)
    }

    fn parse_cell_empty(&mut self, start: &BytesStart<'_>) -> Result<Rc<Cell>> {
        let name = local_name(start)?;
        if name == "sentinel" {
            return Ok(Rc::new(Cell::Sentinel));
        }
        let id = attr_u64(start, b"id")?;
        let fmt = attr_string(start, b"fmt")?;
        if let Some(r) = attr_u64(start, b"ref")? {
            return self.lookup_ref(r);
        }
        let cell = Cell::Leaf(LeafCell {
            element: name,
            id,
            fmt,
            text: String::new(),
        });
        let rc = Rc::new(cell);
        if let Some(i) = id {
            self.ids.insert(i, rc.clone());
        }
        Ok(rc)
    }

    fn lookup_ref(&self, id: u64) -> Result<Rc<Cell>> {
        self.ids.get(&id).cloned().ok_or(Error::UnresolvedRef(id))
    }

    fn consume_until_end(&mut self, _name: &str) -> Result<()> {
        Ok(())
    }
}

fn local_name(start: &BytesStart<'_>) -> Result<String> {
    let ln = start.local_name();
    Ok(std::str::from_utf8(ln.as_ref())?.to_string())
}

fn attr_string(start: &BytesStart<'_>, key: &[u8]) -> Result<Option<String>> {
    for attr in start.attributes() {
        let attr = attr.map_err(quick_xml::Error::from)?;
        if attr.key.as_ref() == key {
            let v = attr.normalized_value(XML_VERSION).map_err(Error::Xml)?;
            return Ok(Some(v.into_owned()));
        }
    }
    Ok(None)
}

fn attr_u64(start: &BytesStart<'_>, key: &[u8]) -> Result<Option<u64>> {
    Ok(match attr_string(start, key)? {
        Some(s) => Some(s.parse()?),
        None => None,
    })
}
