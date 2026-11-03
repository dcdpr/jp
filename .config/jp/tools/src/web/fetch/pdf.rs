//! PDF fetch pipeline.
//!
//! Turns a downloaded PDF into HTML and runs it through the HTML pipeline's
//! section listing, section extraction, and markdown conversion.
//!
//! The text comes from poppler's `pdftotext`, which keeps the reading order of
//! multi-column layouts.
//! The bookmark outline comes from `pdftohtml -xml`; each bookmark becomes a
//! heading on the page it points at, so bookmarks are addressable as sections.
//! A PDF without bookmarks gets one heading per page instead.
//! Pages are addressable either way as `page-N` and `pages-N-M`.
//!
//! Both binaries ship with poppler and are looked up on `PATH`.

use std::{
    env,
    io::ErrorKind,
    ops::RangeInclusive,
    process::Stdio,
    slice,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use quick_xml::{
    Reader,
    escape::{escape, resolve_predefined_entity},
    events::Event,
};
use reqwest::Response;
use tokio::{fs, process::Command, time::timeout};
use url::Url;

use super::{SUMMARIZE_THRESHOLD, html, markdown::Slugger, truncate};
use crate::{
    Error,
    util::{ToolResult, error},
};

/// Ceiling on a single poppler invocation.
const CONVERT_TIMEOUT: Duration = Duration::from_mins(1);

/// How far into the file the `%PDF-` signature may start.
///
/// The PDF spec tolerates leading junk before the header, and readers look for
/// it within the first kilobyte.
const SIGNATURE_WINDOW: usize = 1024;

const INSTALL_HINT: &str = "Reading PDFs needs poppler's `pdftotext` and `pdftohtml` on PATH. \
                            Install poppler (`brew install poppler`, `apt install poppler-utils`, \
                            or `nixpkgs#poppler-utils`) and retry.";

/// Appended to a whole-document answer that had to be cut short.
const TOO_LARGE: &str = "This PDF is too large to return whole. Fetch the sections or pages you \
                         need with the `sections` argument:";

/// Distinguishes the temp files of concurrent conversions in one process.
static FILE_SEQ: AtomicU64 = AtomicU64::new(0);

/// True if `content_type` names a PDF.
pub(super) fn is_pdf_content_type(content_type: &str) -> bool {
    let ct = content_type.trim().to_ascii_lowercase();
    ct.starts_with("application/pdf") || ct.starts_with("application/x-pdf")
}

/// True if `url`'s path ends in `.pdf`, ignoring case.
pub(super) fn has_pdf_path(url: &Url) -> bool {
    url.path().to_ascii_lowercase().ends_with(".pdf")
}

/// True if `bytes` carries the PDF header signature near its start.
fn has_pdf_signature(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(SIGNATURE_WINDOW)]
        .windows(5)
        .any(|w| w == b"%PDF-")
}

/// Read a response labelled as generic binary data, if it is a PDF.
///
/// Returns `None` after reading only the first chunk when the body doesn't
/// start like a PDF, so a large non-PDF download is not pulled in whole.
pub(super) async fn read_if_pdf(mut response: Response) -> Result<Option<Vec<u8>>, Error> {
    let Some(first) = response.chunk().await? else {
        return Ok(None);
    };
    if !has_pdf_signature(&first) {
        return Ok(None);
    }

    let mut body = first.to_vec();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
    }

    Ok(Some(body))
}

/// Convert `bytes` and answer the request the same way the HTML pipeline
/// answers it for a web page.
pub(super) async fn render(
    url: &Url,
    bytes: &[u8],
    list_sections: bool,
    sections: Option<Vec<String>>,
) -> ToolResult {
    let (text, outline) = match extract(bytes).await {
        Ok(extracted) => extracted,
        Err(e) => return error(e),
    };

    answer(
        &Document::build(&text, &outline),
        url,
        list_sections,
        sections,
    )
}

/// Run poppler on `bytes`, returning the page text and the outline XML.
///
/// `pdftohtml` refuses to read stdin while writing to stdout, so the PDF goes
/// through a temp file for both tools.
async fn extract(bytes: &[u8]) -> Result<(String, String), Error> {
    let seq = FILE_SEQ.fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!("jp-web-fetch-{}-{seq}.pdf", std::process::id()));
    fs::write(&path, bytes).await?;

    let mut text = Command::new("pdftotext");
    text.args(["-enc", "UTF-8"]).arg(&path).arg("-");

    // The outline is written after the pages, whatever page range is asked
    // for, so converting only the first page keeps the XML small.
    let mut outline = Command::new("pdftohtml");
    outline
        .args(["-xml", "-i", "-stdout", "-f", "1", "-l", "1"])
        .arg(&path);

    let result = tokio::try_join!(run(text), run(outline));

    // Best-effort: a leftover file in the temp dir is harmless, and failing the
    // fetch over it would throw away a successful conversion.
    drop(fs::remove_file(&path).await);

    result
}

async fn run(mut command: Command) -> Result<String, Error> {
    let program = command
        .as_std()
        .get_program()
        .to_string_lossy()
        .into_owned();

    let child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("`{program}` was not found. {INSTALL_HINT}"),
            _ => format!("failed to start `{program}`: {e}"),
        })?;

    let output = timeout(CONVERT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("`{program}` timed out after {}s", CONVERT_TIMEOUT.as_secs()))??;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "`{program}` could not read this PDF ({}): {}",
            output.status,
            truncate(stderr.trim(), 2_000)
        )
        .into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// A PDF rendered as HTML, one fragment per page.
#[cfg_attr(test, derive(Debug))]
struct Document {
    /// HTML for each page.
    /// Page `N` is at index `N - 1`.
    pages: Vec<String>,

    /// Ids of the headings built from bookmarks, in document order.
    sections: Vec<String>,
}

impl Document {
    /// Build the document from `pdftotext` output and `pdftohtml -xml` output.
    fn build(text: &str, outline_xml: &str) -> Self {
        let pages = split_pages(text);
        let bookmarks = resolve_pages(parse_outline(outline_xml), pages.len());
        let has_outline = !bookmarks.is_empty();

        let mut slugger = Slugger::default();
        let mut sections = Vec::new();
        let mut by_page = vec![Vec::new(); pages.len()];
        for bookmark in bookmarks {
            let id = Some(slugger.slug(&bookmark.title)).filter(|id| !id.is_empty());
            sections.extend(id.clone());
            by_page[bookmark.page - 1].push(Heading {
                level: bookmark.level,
                id,
                text: bookmark.title,
            });
        }

        let pages = pages
            .iter()
            .zip(by_page)
            .enumerate()
            .map(|(index, (page, headings))| {
                let lines: Vec<&str> = page.lines().collect();
                let placements = if has_outline {
                    place(&lines, headings)
                } else {
                    vec![page_heading(index + 1)]
                };
                page_html(&lines, &placements)
            })
            .collect();

        Self { pages, sections }
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// HTML document holding the pages in `pages`, numbered from 1.
    fn html(&self, pages: &RangeInclusive<usize>) -> String {
        let body = self.pages[pages.start() - 1..*pages.end()].concat();
        format!("<html><body>\n{body}</body></html>")
    }

    fn full_html(&self) -> String {
        self.html(&(1..=self.page_count()))
    }
}

/// One piece of a `sections` request.
#[cfg_attr(test, derive(Debug, PartialEq))]
enum Part {
    Pages(RangeInclusive<usize>),
    Section(String),
}

fn answer(
    doc: &Document,
    url: &Url,
    list_sections: bool,
    sections: Option<Vec<String>>,
) -> ToolResult {
    if list_sections {
        return Ok(listing(doc).into());
    }

    let ids = match sections.filter(|ids| !ids.is_empty()) {
        Some(ids) => ids,
        None => match url.fragment().and_then(|f| fragment_id(doc, f)) {
            Some(id) => vec![id],
            None => return whole(doc),
        },
    };

    let parts = match parts(doc, &ids) {
        Ok(parts) => parts,
        Err(message) => return error(message),
    };

    let full = doc.full_html();
    let mut chunks = Vec::with_capacity(parts.len());
    for part in parts {
        let fragment = match part {
            Part::Pages(pages) => doc.html(&pages),
            Part::Section(id) => match html::extract_sections(&full, slice::from_ref(&id)) {
                Ok(fragment) => fragment,
                // `parts` only lets through ids the document's headings carry.
                Err(_) => return error(format!("No section `{id}` in this PDF.")),
            },
        };
        chunks.push(html::html_to_markdown(&fragment)?);
    }

    Ok(truncate(&chunks.join("\n\n"), SUMMARIZE_THRESHOLD).into())
}

/// The whole document, or as much as fits followed by the section listing.
fn whole(doc: &Document) -> ToolResult {
    let md = html::html_to_markdown(&doc.full_html())?;
    if md.len() <= SUMMARIZE_THRESHOLD {
        return Ok(md.into());
    }

    let listing = listing(doc);
    let budget = SUMMARIZE_THRESHOLD.saturating_sub(listing.len() + TOO_LARGE.len());
    Ok(format!("{}\n\n{TOO_LARGE}\n\n{listing}", truncate(&md, budget)).into())
}

fn listing(doc: &Document) -> String {
    format!(
        "{}\n\nPage count: {}. Request pages as `page-N` or `pages-N-M`.",
        html::format_section_listing(&doc.full_html()),
        doc.page_count()
    )
}

/// Resolve the requested ids, rejecting any that name nothing in `doc`.
fn parts(doc: &Document, ids: &[String]) -> Result<Vec<Part>, String> {
    let count = doc.page_count();
    let mut parts = Vec::with_capacity(ids.len());
    let mut unknown = Vec::new();

    for id in ids {
        if let Some(pages) = parse_page_id(id) {
            if *pages.end() > count {
                return Err(format!(
                    "`{id}` is past the end of this PDF, which has {count} pages."
                ));
            }
            parts.push(Part::Pages(pages));
        } else if doc.sections.contains(id) {
            parts.push(Part::Section(id.clone()));
        } else {
            unknown.push(format!("`{id}`"));
        }
    }

    if !unknown.is_empty() {
        return Err(format!(
            "No section {} in this PDF. Call with `list_sections` to see its section ids; pages \
             are addressable as `page-N` and `pages-N-M`.",
            unknown.join(", ")
        ));
    }

    Ok(parts)
}

/// Map a URL fragment to a section id.
///
/// Understands the `page=N` open parameter (`#page=3`, `#page=3&zoom=100`) and
/// bare section ids.
/// Anything else, such as `#nameddest=...`, is ignored.
fn fragment_id(doc: &Document, fragment: &str) -> Option<String> {
    if let Some(page) = fragment.split('&').find_map(|p| p.strip_prefix("page=")) {
        return Some(format!("page-{page}"));
    }

    doc.sections.iter().find(|id| *id == fragment).cloned()
}

/// Parse `page-N` or `pages-N-M` into a 1-based inclusive page range.
fn parse_page_id(id: &str) -> Option<RangeInclusive<usize>> {
    if let Some(page) = id.strip_prefix("page-") {
        let page: usize = page.parse().ok()?;
        return (page >= 1).then_some(page..=page);
    }

    let (start, end) = id.strip_prefix("pages-")?.split_once('-')?;
    let (start, end): (usize, usize) = (start.parse().ok()?, end.parse().ok()?);
    (1 <= start && start <= end).then_some(start..=end)
}

/// Split `pdftotext` output into pages.
///
/// Pages are separated by form feeds, and the output ends with one, which does
/// not start another page.
fn split_pages(text: &str) -> Vec<&str> {
    let mut pages: Vec<&str> = text.split('\u{c}').collect();
    if pages.len() > 1 && pages.last().is_some_and(|p| p.trim().is_empty()) {
        pages.pop();
    }
    pages
}

/// A bookmark as `pdftohtml -xml` reports it.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct OutlineItem {
    title: String,
    page: Option<usize>,
    depth: usize,
}

/// A bookmark resolved to a page in the document.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Bookmark {
    title: String,
    page: usize,
    level: u8,
}

/// Read the bookmarks out of `pdftohtml -xml` output, in document order.
///
/// Returns nothing for a PDF without bookmarks.
fn parse_outline(xml: &str) -> Vec<OutlineItem> {
    // Parse from the outline on: the page content before it is positioned
    // text that the outline doesn't need.
    let Some(start) = xml.find("<outline>") else {
        return Vec::new();
    };

    let mut reader = Reader::from_str(&xml[start..]);
    let mut items = Vec::new();
    let mut depth = 0_usize;
    let mut current: Option<OutlineItem> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == b"outline" => depth += 1,
            Ok(Event::End(e)) if e.name().as_ref() == b"outline" => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Start(e)) if e.name().as_ref() == b"item" => {
                let page = e
                    .attributes()
                    .flatten()
                    .find(|a| a.key.as_ref() == b"page")
                    .and_then(|a| str::from_utf8(&a.value).ok()?.parse().ok());
                current = Some(OutlineItem {
                    title: String::new(),
                    page,
                    depth,
                });
            }
            Ok(Event::Text(text)) => {
                if let Some(item) = current.as_mut()
                    && let Ok(text) = text.decode()
                {
                    item.title.push_str(&text);
                }
            }
            Ok(Event::GeneralRef(reference)) => {
                if let Some(item) = current.as_mut() {
                    if let Ok(Some(ch)) = reference.resolve_char_ref() {
                        item.title.push(ch);
                    } else if let Ok(name) = reference.decode()
                        && let Some(resolved) = resolve_predefined_entity(&name)
                    {
                        item.title.push_str(resolved);
                    }
                }
            }
            Ok(Event::End(e)) if e.name().as_ref() == b"item" => {
                if let Some(mut item) = current.take() {
                    item.title = normalize(&item.title);
                    if !item.title.is_empty() {
                        items.push(item);
                    }
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    items
}

/// Give every bookmark a page in `1..=page_count`.
///
/// A bookmark whose destination poppler couldn't resolve takes the page of the
/// next bookmark that has one, which is where its first child usually points.
/// Bookmarks left without a page in range are dropped.
fn resolve_pages(items: Vec<OutlineItem>, page_count: usize) -> Vec<Bookmark> {
    let mut next_page = None;
    let mut resolved: Vec<Bookmark> = items
        .into_iter()
        .rev()
        .filter_map(|item| {
            let page = match item.page {
                Some(page) => {
                    next_page = Some(page);
                    page
                }
                None => next_page?,
            };
            if !(1..=page_count).contains(&page) {
                return None;
            }

            Some(Bookmark {
                title: item.title,
                page,
                level: u8::try_from(item.depth.clamp(1, 6)).unwrap_or(6),
            })
        })
        .collect();

    resolved.reverse();
    resolved
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Heading {
    level: u8,
    id: Option<String>,
    text: String,
}

/// Where a heading goes on its page.
#[cfg_attr(test, derive(Debug, PartialEq))]
struct Placement {
    /// Index of the line the heading goes before.
    line: usize,

    /// How many lines from `line` on the heading replaces: the title it was
    /// matched against, or none if it wasn't found on the page.
    consumed: usize,

    heading: Heading,
}

fn page_heading(page: usize) -> Placement {
    Placement {
        line: 0,
        consumed: 0,
        heading: Heading {
            level: 1,
            id: Some(format!("page-{page}")),
            text: format!("Page {page}"),
        },
    }
}

/// Find where each of a page's headings goes, keeping them in order.
///
/// Each heading replaces the line holding its title.
/// One that isn't found goes right after the previous heading, or at the top of
/// the page.
fn place(lines: &[&str], headings: Vec<Heading>) -> Vec<Placement> {
    let mut cursor = 0;
    let mut placements = Vec::with_capacity(headings.len());

    for mut heading in headings {
        let placement = match find_title(lines, cursor, &heading.text) {
            Some(found) => {
                if let Some(number) = found.number {
                    heading.text = format!("{number} {}", heading.text);
                }
                Placement {
                    line: found.line,
                    consumed: found.consumed,
                    heading,
                }
            }
            None => Placement {
                line: cursor,
                consumed: 0,
                heading,
            },
        };

        cursor = placement.line + placement.consumed;
        placements.push(placement);
    }

    placements
}

struct TitleMatch {
    line: usize,
    consumed: usize,

    /// Section number printed on its own line above the title.
    number: Option<String>,
}

/// Find `title` on or after line `from`.
///
/// A title preceded by a section number on its own line (`3.2.1`, then a blank
/// line, then the title) is preferred over a bare one: the same words often
/// appear earlier as a figure label, and the numbered form is the heading.
fn find_title(lines: &[&str], from: usize, title: &str) -> Option<TitleMatch> {
    let want = fold(title);
    let range = from.min(lines.len())..lines.len();

    let numbered = range.clone().find_map(|i| {
        let number = normalize(lines[i]);
        if !is_section_number(&number) {
            return None;
        }

        let next = (i + 1..lines.len()).find(|&j| !lines[j].trim().is_empty())?;
        (fold(lines[next]) == want).then(|| TitleMatch {
            line: i,
            consumed: next + 1 - i,
            number: Some(number),
        })
    });

    numbered.or_else(|| {
        range
            .into_iter()
            .find(|&i| fold(lines[i]) == want)
            .map(|i| TitleMatch {
                line: i,
                consumed: 1,
                number: None,
            })
    })
}

/// True for section numbers such as `3`, `3.2.1`, `5.7.`, and `A.1`.
fn is_section_number(s: &str) -> bool {
    let digits = s
        .strip_prefix(|c: char| c.is_ascii_uppercase())
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or(s);

    digits.chars().any(|c| c.is_ascii_digit())
        && digits.chars().all(|c| c.is_ascii_digit() || c == '.')
}

/// Render a page's lines as paragraphs, with its headings in place.
///
/// Blank lines separate paragraphs.
fn page_html(lines: &[&str], placements: &[Placement]) -> String {
    let mut out = String::new();
    let mut paragraph = Vec::new();
    let mut placements = placements.iter().peekable();
    let mut skip_until = 0;

    for (i, line) in lines.iter().enumerate() {
        while let Some(placement) = placements.next_if(|p| p.line == i) {
            flush_paragraph(&mut out, &mut paragraph);
            push_heading(&mut out, &placement.heading);
            skip_until = i + placement.consumed;
        }

        if i < skip_until {
            continue;
        }

        if line.trim().is_empty() {
            flush_paragraph(&mut out, &mut paragraph);
        } else {
            paragraph.push(*line);
        }
    }

    flush_paragraph(&mut out, &mut paragraph);
    for placement in placements {
        push_heading(&mut out, &placement.heading);
    }

    out
}

fn flush_paragraph(out: &mut String, paragraph: &mut Vec<&str>) {
    if paragraph.is_empty() {
        return;
    }

    out.push_str("<p>");
    out.push_str(&escape(paragraph.join("\n")));
    out.push_str("</p>\n");
    paragraph.clear();
}

fn push_heading(out: &mut String, heading: &Heading) {
    let level = heading.level;
    let text = escape(heading.text.as_str());
    let line = match &heading.id {
        Some(id) => format!(
            "<h{level} id=\"{}\">{text}</h{level}>\n",
            escape(id.as_str())
        ),
        None => format!("<h{level}>{text}</h{level}>\n"),
    };
    out.push_str(&line);
}

/// Collapse whitespace runs (including no-break spaces) to single spaces.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn fold(s: &str) -> String {
    normalize(s).to_lowercase()
}

#[cfg(test)]
#[path = "pdf_tests.rs"]
mod tests;
