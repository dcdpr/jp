use std::{collections::BTreeMap, io::Cursor};

use quick_xml::{
    Reader,
    events::{BytesStart, Event},
};
use serde::Serialize;

use crate::{
    error::{Error, Result},
    xml::XML_VERSION,
};

#[derive(Debug, Clone, Serialize)]
pub struct Toc {
    pub runs: Vec<Run>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Run {
    pub number: u32,
    pub info: Info,
    pub processes: Vec<TocProcess>,
    pub tables: Vec<Table>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Info {
    pub target: Option<Target>,
    pub summary: Option<Summary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Target {
    pub device: BTreeMap<String, String>,
    pub process: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub duration: Option<String>,
    pub end_reason: Option<String>,
    pub instruments_version: Option<String>,
    pub template_name: Option<String>,
    pub recording_mode: Option<String>,
    pub time_limit: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TocProcess {
    pub name: String,
    pub pid: i64,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Table {
    pub schema: String,
    pub documentation: Option<String>,
    pub attributes: BTreeMap<String, String>,
}

impl Toc {
    pub fn parse(xml: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(Cursor::new(xml));
        reader.config_mut().trim_text(false);
        let mut buf = Vec::new();
        let mut runs: Vec<Run> = Vec::new();
        loop {
            buf.clear();
            match reader.read_event_into(&mut buf)? {
                Event::Eof => break,
                Event::Start(s) if local_name(&s)? == "trace-toc" => continue,
                Event::Start(s) if local_name(&s)? == "run" => {
                    let number = u32_attr(&s, b"number")?.unwrap_or(0);
                    runs.push(read_run(&mut reader, number)?);
                }
                _ => {}
            }
        }
        Ok(Toc { runs })
    }

    pub fn run(&self, number: u32) -> Option<&Run> {
        self.runs.iter().find(|r| r.number == number)
    }

    pub fn first_run(&self) -> Option<&Run> {
        self.runs.first()
    }
}

impl Run {
    pub fn table(&self, schema: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.schema == schema)
    }

    pub fn tables_with(&self, schema: &str) -> impl Iterator<Item = &Table> {
        self.tables.iter().filter(move |t| t.schema == schema)
    }
}

fn read_run<R: std::io::BufRead>(reader: &mut Reader<R>, number: u32) -> Result<Run> {
    let mut buf = Vec::new();
    let mut info = Info::default();
    let mut processes: Vec<TocProcess> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Start(s) => match local_name(&s)?.as_str() {
                "info" => info = read_info(reader)?,
                "processes" => processes = read_processes(reader)?,
                "data" => tables = read_data(reader)?,
                "tracks" => skip_to_end(reader, "tracks")?,
                _ => skip_to_end(reader, &local_name(&s)?)?,
            },
            Event::Empty(_) => {}
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == "run" => break,
            Event::Eof => return Err(Error::Schema("EOF inside <run>".into())),
            _ => {}
        }
    }
    Ok(Run {
        number,
        info,
        processes,
        tables,
    })
}

fn read_info<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Info> {
    let mut buf = Vec::new();
    let mut info = Info::default();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Start(s) => match local_name(&s)?.as_str() {
                "target" => info.target = Some(read_target(reader)?),
                "summary" => info.summary = Some(read_summary(reader)?),
                other => skip_to_end(reader, other)?,
            },
            Event::Empty(_) => {}
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == "info" => break,
            Event::Eof => return Err(Error::Schema("EOF inside <info>".into())),
            _ => {}
        }
    }
    Ok(info)
}

fn read_target<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Target> {
    let mut buf = Vec::new();
    let mut device = BTreeMap::new();
    let mut process = BTreeMap::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Empty(s) => match local_name(&s)?.as_str() {
                "device" => device = collect_attrs(&s)?,
                "process" => process = collect_attrs(&s)?,
                _ => {}
            },
            Event::Start(s) => {
                let n = local_name(&s)?;
                let attrs = collect_attrs(&s)?;
                match n.as_str() {
                    "device" => device = attrs,
                    "process" => process = attrs,
                    _ => {}
                }
                skip_to_end(reader, &n)?;
            }
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == "target" => break,
            Event::Eof => return Err(Error::Schema("EOF inside <target>".into())),
            _ => {}
        }
    }
    Ok(Target { device, process })
}

fn read_summary<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Summary> {
    let mut buf = Vec::new();
    let mut summary = Summary::default();
    let mut current: Option<String> = None;
    let mut text_buf = String::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Start(s) => {
                let name = local_name(&s)?;
                if matches!(
                    name.as_str(),
                    "intruments-recording-settings" | "instruments-recording-settings"
                ) {
                    skip_to_end(reader, &name)?;
                    continue;
                }
                current = Some(name);
                text_buf.clear();
            }
            Event::Text(t) => {
                if current.is_some() {
                    text_buf.push_str(&t.xml_content(XML_VERSION)?);
                }
            }
            Event::End(e) => {
                let n = std::str::from_utf8(e.name().as_ref())?.to_string();
                if n == "summary" {
                    break;
                }
                if let Some(opening) = current.take()
                    && opening == n
                {
                    let v = std::mem::take(&mut text_buf).trim().to_string();
                    match n.as_str() {
                        "start-date" => summary.start_date = Some(v),
                        "end-date" => summary.end_date = Some(v),
                        "duration" => summary.duration = Some(v),
                        "end-reason" => summary.end_reason = Some(v),
                        "instruments-version" => summary.instruments_version = Some(v),
                        "template-name" => summary.template_name = Some(v),
                        "recording-mode" => summary.recording_mode = Some(v),
                        "time-limit" => summary.time_limit = Some(v),
                        _ => {}
                    }
                }
            }
            Event::Eof => return Err(Error::Schema("EOF inside <summary>".into())),
            _ => {}
        }
    }
    Ok(summary)
}

fn read_processes<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Vec<TocProcess>> {
    let mut buf = Vec::new();
    let mut out: Vec<TocProcess> = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            // Self-closing `<process/>` has no end tag to consume. Consuming one
            // anyway swallows whatever follows, which for adjacent processes is
            // the next one and for the last is `</processes>`.
            Event::Empty(s) if local_name(&s)? == "process" => {
                out.push(process(&s)?);
            }
            Event::Start(s) if local_name(&s)? == "process" => {
                out.push(process(&s)?);
                // Skip whatever the element wraps, so the loop resumes on the
                // next sibling rather than inside this one.
                skip_to_end(reader, "process")?;
            }
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == "processes" => break,
            Event::Eof => return Err(Error::Schema("EOF inside <processes>".into())),
            _ => {}
        }
    }
    Ok(out)
}

fn read_data<R: std::io::BufRead>(reader: &mut Reader<R>) -> Result<Vec<Table>> {
    let mut buf = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    loop {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Empty(s) if local_name(&s)? == "table" => {
                tables.push(table_from_attrs(&s)?);
            }
            Event::Start(s) if local_name(&s)? == "table" => {
                let t = table_from_attrs(&s)?;
                tables.push(t);
                skip_to_end(reader, "table")?;
            }
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == "data" => break,
            Event::Eof => return Err(Error::Schema("EOF inside <data>".into())),
            _ => {}
        }
    }
    Ok(tables)
}

fn table_from_attrs(s: &BytesStart<'_>) -> Result<Table> {
    let mut attrs = collect_attrs(s)?;
    let schema = attrs
        .remove("schema")
        .ok_or_else(|| Error::Schema("table missing schema attr".into()))?;
    let documentation = attrs.remove("documentation");
    Ok(Table {
        schema,
        documentation,
        attributes: attrs,
    })
}

/// One `<process>` element, from its attributes.
///
/// A process with no readable `pid` gets `-1`, which is what the rest of the
/// crate treats as "not a real process".
fn process(s: &BytesStart<'_>) -> Result<TocProcess> {
    let attrs = collect_attrs(s)?;
    Ok(TocProcess {
        name: attrs.get("name").cloned().unwrap_or_default(),
        pid: attrs.get("pid").and_then(|v| v.parse().ok()).unwrap_or(-1),
        path: attrs.get("path").cloned(),
    })
}

fn collect_attrs(s: &BytesStart<'_>) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for attr in s.attributes() {
        let attr = attr.map_err(quick_xml::Error::from)?;
        let key = std::str::from_utf8(attr.key.as_ref())?.to_string();
        let val = attr.normalized_value(XML_VERSION)?.into_owned();
        out.insert(key, val);
    }
    Ok(out)
}

fn local_name(s: &BytesStart<'_>) -> Result<String> {
    Ok(std::str::from_utf8(s.local_name().as_ref())?.to_string())
}

fn u32_attr(s: &BytesStart<'_>, key: &[u8]) -> Result<Option<u32>> {
    for attr in s.attributes() {
        let attr = attr.map_err(quick_xml::Error::from)?;
        if attr.key.as_ref() == key {
            let v = attr.normalized_value(XML_VERSION)?;
            return Ok(Some(v.parse()?));
        }
    }
    Ok(None)
}

fn skip_to_end<R: std::io::BufRead>(reader: &mut Reader<R>, name: &str) -> Result<()> {
    let mut buf = Vec::new();
    let mut depth: i32 = 1;
    while depth > 0 {
        buf.clear();
        match reader.read_event_into(&mut buf)? {
            Event::Start(s) if local_name(&s)? == name => depth += 1,
            Event::End(e) if std::str::from_utf8(e.name().as_ref())? == name => depth -= 1,
            Event::Eof => {
                return Err(Error::Schema(format!("EOF while skipping <{name}>")));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "toc_tests.rs"]
mod tests;
