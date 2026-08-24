use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
};

use addr2line::Loader;
use object::{Object, ObjectSection, ObjectSegment, ObjectSymbol, SymbolKind};
use serde::Serialize;

use crate::{
    address::{FilePc, RuntimePc, Slide},
    analysis::PcSample,
    error::{Error, Result},
    symbol::swift,
    trace::TraceBundle,
    xml::{
        Cell,
        stream::{RowReader, RowReaderEvent},
    },
};

#[derive(Debug, Clone, Default)]
pub struct SymbolicatorOptions {
    pub binary: Option<PathBuf>,
    pub dsym: Option<PathBuf>,
    /// Subtracted from runtime PCs before lookup.
    /// Use when the binary was loaded with an ASLR offset.
    pub slide: Slide,
}

pub struct Symbolicator {
    loader: Option<Loader>,
    slide: Slide,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolicatedFrame {
    pub address: RuntimePc,
    /// The deepest-inlined function this PC actually belongs to - i.e. the
    /// source the compiler labels the instruction as coming from.
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    /// Outer call sites that inlined this PC, closest-out first.
    /// Empty when not inlined; the last entry is the concrete binary function.
    pub inlined_into: Vec<InlinedFrame>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InlinedFrame {
    pub function: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
}

impl Symbolicator {
    pub fn new(opts: SymbolicatorOptions) -> Result<Self> {
        let pick = opts.dsym.as_ref().or(opts.binary.as_ref()).cloned();
        let loader = match pick {
            Some(p) => Some(load(&p)?),
            None => None,
        };
        Ok(Self {
            loader,
            slide: opts.slide,
        })
    }

    /// The binary's symbol-table name for this PC
    /// (`addr2line::Loader::find_symbol_info`).
    /// Unlike `resolve`, this returns the *concrete* binary function containing
    /// the PC - not the deepest-inlined source function.
    /// Use for stack-frame analysis where you want the real call frame.
    pub fn symbol_at(&self, runtime_pc: RuntimePc) -> Option<String> {
        let loader = self.loader.as_ref()?;
        let probe = runtime_pc.to_file(self.slide)?;
        loader
            .find_symbol_info(probe.raw())
            .map(|s| demangle(s.name()))
    }

    pub fn resolve(&self, runtime_pc: RuntimePc) -> Result<SymbolicatedFrame> {
        let mut frame = SymbolicatedFrame {
            address: runtime_pc,
            function: None,
            file: None,
            line: None,
            column: None,
            inlined_into: Vec::new(),
        };
        let Some(loader) = &self.loader else {
            return Ok(frame);
        };
        let Some(probe) = runtime_pc.to_file(self.slide) else {
            return Ok(frame);
        };
        let probe_raw = probe.raw();

        // addr2line yields frames innermost-first: the very first frame
        // is the deepest inlined function (what the instruction actually
        // belongs to), and each subsequent frame is the call site that
        // inlined the previous one. The last frame is the outermost
        // concrete binary function.
        let mut iter = loader
            .find_frames(probe_raw)
            .map_err(|e| Error::Addr2Line(e.to_string()))?;
        let mut frames: Vec<addr2line::Frame<'_, _>> = Vec::new();
        while let Some(f) = iter.next().map_err(|e| Error::Addr2Line(e.to_string()))? {
            frames.push(f);
        }

        if let Some(innermost) = frames.first() {
            if let Some(fun) = innermost.function.as_ref() {
                frame.function = demangled_function(fun);
            }
            if let Some(loc) = innermost.location.as_ref() {
                frame.file = loc.file.map(str::to_string);
                frame.line = loc.line;
                frame.column = loc.column;
            }
        }

        if frame.function.is_none()
            && let Some(sym) = loader.find_symbol_info(probe_raw)
        {
            frame.function = Some(demangle(sym.name()));
        }

        for outer in frames.iter().skip(1) {
            frame.inlined_into.push(InlinedFrame {
                function: outer.function.as_ref().and_then(demangled_function),
                file: outer
                    .location
                    .as_ref()
                    .and_then(|l| l.file.map(str::to_string)),
                line: outer.location.as_ref().and_then(|l| l.line),
            });
        }

        Ok(frame)
    }
}

fn load(path: &Path) -> Result<Loader> {
    let resolved = resolve_dsym(path);
    Loader::new(&resolved).map_err(|e| Error::Addr2Line(e.to_string()))
}

#[derive(Debug, Clone)]
pub struct BinaryInfo {
    /// Preferred VM address of the `__text` section (executable code).
    pub text_start: FilePc,
    pub text_end: FilePc,
    /// Preferred VM address of the `__TEXT` segment (Mach-O header).
    /// Used to convert kdebug-reported load addresses into a slide.
    pub segment_text_start: FilePc,
    /// Sorted ascending.
    /// File addresses of function symbols in `__text`.
    pub function_addrs: Vec<FilePc>,
    /// The binary's `LC_UUID`.
    pub uuid: Option<[u8; 16]>,
}

impl BinaryInfo {
    /// Parse a Mach-O binary for the data needed to detect ASLR slide.
    pub fn open(binary: &Path) -> Result<Self> {
        let data = std::fs::read(binary)?;
        let file = object::File::parse(&*data)?;
        let mut segment_text_start: Option<u64> = None;
        let mut segment_text_end: u64 = 0;
        for seg in file.segments() {
            if let Some(name) = seg.name()?
                && name == "__TEXT"
            {
                segment_text_start = Some(seg.address());
                segment_text_end = seg.address() + seg.size();
            }
        }
        let (text_start_raw, text_end_raw) = match file.section_by_name("__text") {
            Some(sec) => (sec.address(), sec.address() + sec.size()),
            None => (segment_text_start.unwrap_or(0), segment_text_end),
        };
        if text_end_raw <= text_start_raw {
            return Err(Error::Schema("binary has no __TEXT segment".into()));
        }
        let mut function_addrs: Vec<FilePc> = file
            .symbols()
            .filter(|s| s.kind() == SymbolKind::Text)
            .map(|s| s.address())
            .filter(|a| *a >= text_start_raw && *a < text_end_raw)
            .map(FilePc::new)
            .collect();
        function_addrs.sort();
        function_addrs.dedup();
        let uuid = file.mach_uuid()?;
        let segment_text_start = FilePc::new(segment_text_start.unwrap_or(text_start_raw));
        Ok(BinaryInfo {
            text_start: FilePc::new(text_start_raw),
            text_end: FilePc::new(text_end_raw),
            segment_text_start,
            function_addrs,
            uuid,
        })
    }

    /// Look up the slide by matching this binary's `LC_UUID` against the
    /// trace's recorded image loads.
    /// Returns `None` if the UUID isn't in `loads` or the binary has no UUID.
    pub fn slide_from(&self, loads: &[ImageLoad]) -> Option<Slide> {
        let uuid = self.uuid?;
        let load = loads.iter().find(|l| l.uuid == uuid)?;
        load.load_address
            .raw()
            .checked_sub(self.segment_text_start.raw())
            .map(Slide::new)
    }

    /// Heuristic enumeration of plausible page-aligned ASLR slides.
    /// Provided as a fallback for traces with no kdebug DBG_DYLD events.
    /// Ranking is heuristic; multiple slides will look equally valid for short
    /// traces or stripped binaries.
    pub fn enumerate_slides(
        &self,
        pcs_with_weight: &[PcSample],
        dwarf_path: &Path,
    ) -> Vec<SlideCandidate> {
        const PAGE: u64 = 0x4000;
        let resolved = resolve_dsym(dwarf_path);
        let loader = Loader::new(&resolved).ok();
        let text_start = self.text_start.raw();
        let text_end = self.text_end.raw();

        // Ordered, because the ranking below can leave two candidates tied and
        // the order they were enumerated in then decides which one a caller
        // acts on. A hashed set would make that choice differently per process,
        // and a wrong slide does not fail — it names the wrong functions.
        let mut candidates: BTreeSet<u64> = BTreeSet::new();
        for s in pcs_with_weight {
            let pc = s.pc.raw();
            if pc < text_start {
                continue;
            }
            let max_slide = pc.saturating_sub(text_start);
            let min_slide = pc.saturating_sub(text_end.saturating_sub(1));
            let max_aligned = max_slide & !(PAGE - 1);
            let min_aligned = (min_slide + PAGE - 1) & !(PAGE - 1);
            let mut s = min_aligned;
            while s <= max_aligned {
                candidates.insert(s);
                s = match s.checked_add(PAGE) {
                    Some(v) => v,
                    None => break,
                };
            }
        }

        let func_addrs_raw: Vec<u64> = self.function_addrs.iter().map(|f| f.raw()).collect();
        let mut out: Vec<SlideCandidate> = Vec::new();
        for slide in candidates {
            let mut per_func: HashMap<u64, u64> = HashMap::new();
            let mut covered: u64 = 0;
            for s in pcs_with_weight {
                let Some(probe) = s.pc.raw().checked_sub(slide) else {
                    continue;
                };
                let Some(func_start) = function_containing(&func_addrs_raw, probe, text_end) else {
                    continue;
                };
                *per_func.entry(func_start).or_insert(0) += s.samples;
                covered += s.samples;
            }
            if covered == 0 {
                continue;
            }
            let mut entries: Vec<(u64, u64)> = per_func.into_iter().collect();
            // Address ascending on a tie: these come out of a `HashMap`, so
            // without it two functions with the same sample count would be
            // reported interchangeably between runs.
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let (top_addr, top_share) = entries[0];
            let func_len = function_length(&func_addrs_raw, top_addr, text_end);
            let top_function_name = loader.as_ref().and_then(|l| top_function_at(l, top_addr));
            out.push(SlideCandidate {
                slide: Slide::new(slide),
                covered_samples: covered,
                top_function_samples: top_share,
                top_function_address: FilePc::new(top_addr),
                top_function_size: func_len,
                top_function_name,
            });
        }
        out.sort_by(rank_candidates);
        out
    }
}

/// Rank two slide candidates: most concentrated first, then tightest function,
/// then lowest slide.
///
/// The slide is the last key, and it is what makes the ranking a total order.
/// Two candidates tied on both heuristics is the normal case for a short trace
/// or a stripped binary — which [`BinaryInfo::enumerate_slides`] says itself
/// — and the candidates are accumulated in a set, so without it the one a
/// caller takes first would differ between runs.
/// A wrong slide does not fail: it names the wrong functions.
fn rank_candidates(a: &SlideCandidate, b: &SlideCandidate) -> Ordering {
    b.top_function_samples
        .cmp(&a.top_function_samples)
        .then_with(|| a.top_function_size.cmp(&b.top_function_size))
        .then_with(|| a.slide.raw().cmp(&b.slide.raw()))
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ImageLoad {
    pub uuid: [u8; 16],
    pub load_address: RuntimePc,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlideCandidate {
    pub slide: Slide,
    pub covered_samples: u64,
    pub top_function_samples: u64,
    pub top_function_address: FilePc,
    pub top_function_size: u64,
    pub top_function_name: Option<String>,
}

impl TraceBundle {
    /// Read every `DBG_DYLD_UUID_MAP_A` event from the trace and decode the
    /// (UUID, runtime load address) pairs.
    /// These events are kernel ground truth - when dyld maps an image, the
    /// kernel records the runtime base address it chose under ASLR.
    pub fn image_loads(&self) -> Result<Vec<ImageLoad>> {
        let xml = self.xctrace().export_xpath(self.path(), DBG_DYLD_XPATH)?;
        let mut reader = RowReader::new(std::io::Cursor::new(xml));
        let mut out: Vec<ImageLoad> = Vec::new();
        while let Some(ev) = reader.next_event()? {
            let RowReaderEvent::Row(cells) = ev else {
                continue;
            };
            let Some(decoded) = decode_dyld_map_a(&cells) else {
                continue;
            };
            out.push(decoded);
        }
        Ok(out)
    }
}

const DBG_DYLD_XPATH: &str = "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"kdebug\"]";

fn decode_dyld_map_a(cells: &[std::rc::Rc<Cell>]) -> Option<ImageLoad> {
    let mut class: Option<u64> = None;
    let mut subclass: Option<u64> = None;
    let mut code: Option<u64> = None;
    let mut args: Vec<u64> = Vec::new();
    for cell in cells {
        let Some(name) = cell.element() else { continue };
        match name {
            "kdebug-class" => class = cell.as_u64(),
            "kdebug-subclass" => subclass = cell.as_u64(),
            "kdebug-code" => code = cell.as_u64(),
            "kdebug-arg" => {
                if let Some(v) = cell.as_u64() {
                    args.push(v);
                }
            }
            _ => {}
        }
    }
    if class? != 31 || subclass? != 5 || code? != 0 {
        return None;
    }
    if args.len() < 3 {
        return None;
    }
    let mut uuid = [0u8; 16];
    uuid[0..8].copy_from_slice(&args[0].to_le_bytes());
    uuid[8..16].copy_from_slice(&args[1].to_le_bytes());
    Some(ImageLoad {
        uuid,
        load_address: RuntimePc::new(args[2]),
    })
}

fn top_function_at(loader: &Loader, probe: u64) -> Option<String> {
    let mut iter = loader.find_frames(probe).ok()?;
    let mut last: Option<String> = None;
    while let Ok(Some(f)) = iter.next() {
        if let Some(name) = f.function.as_ref().and_then(|n| n.raw_name().ok()) {
            last = Some(demangle(&name));
        }
    }
    last
}

fn function_containing(starts: &[u64], probe: u64, text_end: u64) -> Option<u64> {
    let i = starts.partition_point(|&s| s <= probe);
    if i == 0 {
        return None;
    }
    let func_start = starts[i - 1];
    let func_end = starts.get(i).copied().unwrap_or(text_end);
    if probe >= func_start && probe < func_end {
        Some(func_start)
    } else {
        None
    }
}

fn function_length(starts: &[u64], func_start: u64, text_end: u64) -> u64 {
    let i = starts.partition_point(|&s| s <= func_start);
    let func_end = starts.get(i).copied().unwrap_or(text_end);
    func_end.saturating_sub(func_start)
}

fn resolve_dsym(path: &Path) -> PathBuf {
    if path.extension().and_then(|s| s.to_str()) != Some("dSYM") {
        return path.to_path_buf();
    }
    let dwarf_dir = path.join("Contents").join("Resources").join("DWARF");
    let Ok(entries) = std::fs::read_dir(&dwarf_dir) else {
        return path.to_path_buf();
    };
    let mut candidates: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    if candidates.is_empty() {
        return path.to_path_buf();
    }
    candidates.sort_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
    candidates.pop().unwrap()
}

fn demangle(s: &str) -> String {
    if let Some(sym) = swift::demangle(s) {
        return sym;
    }
    if let Ok(sym) = rustc_demangle_try(s) {
        return sym;
    }
    if let Ok(sym) = cpp_demangle_try(s) {
        return sym;
    }
    s.to_string()
}

fn demangled_function<R>(f: &addr2line::FunctionName<R>) -> Option<String>
where
    R: gimli::Reader,
{
    let raw = f.raw_name().ok()?;
    Some(demangle(&raw))
}

fn rustc_demangle_try(s: &str) -> std::result::Result<String, ()> {
    Ok(format!(
        "{:#}",
        rustc_demangle::try_demangle(s).map_err(|_| ())?
    ))
}

fn cpp_demangle_try(s: &str) -> std::result::Result<String, ()> {
    let sym = cpp_demangle::Symbol::new(s).map_err(|_| ())?;
    sym.demangle().map_err(|_| ())
}

#[cfg(test)]
#[path = "macho_tests.rs"]
mod tests;
