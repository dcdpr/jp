//! Per-CPU hotspot aggregation from the `time-sample` table.

use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    rc::Rc,
};

use serde::Serialize;

use crate::{
    address::{CoreId, Pid, RuntimePc, SampleTime, Slide},
    error::Result,
    symbol::{BinaryInfo, Symbolicator, SymbolicatorOptions},
    trace::TraceBundle,
    xml::{
        Cell,
        stream::{RowReader, RowReaderEvent},
    },
};

#[derive(Clone, Serialize)]
pub struct HotspotReport {
    pub total_samples: u64,
    pub per_cpu: BTreeMap<CoreId, CpuStats>,
    pub timeline_buckets_ns: u64,
    pub timeline: Vec<TimelineBucket>,
    pub top_pcs: Vec<Hotspot>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CpuStats {
    pub samples: u64,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TimelineBucket {
    pub start_ns: u64,
    pub end_ns: u64,
    pub samples_per_cpu: BTreeMap<CoreId, u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hotspot {
    pub pc: RuntimePc,
    pub samples: u64,
    pub fmt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Where `HotspotsBuilder` should get its slide from.
#[derive(Debug, Clone, Default)]
pub enum SlideMode {
    /// Recover from kdebug DBG\_DYLD events when a binary is provided.
    #[default]
    Auto,
    /// Use the explicit slide.
    Manual(Slide),
}

pub struct HotspotsBuilder<'a> {
    bundle: &'a TraceBundle,
    pid: Option<Pid>,
    bucket_ns: u64,
    top_n: usize,
    time_window_ns: Option<(u64, u64)>,
    binary: Option<PathBuf>,
    dsym: Option<PathBuf>,
    slide: SlideMode,
    filter: Option<String>,
}

impl<'a> HotspotsBuilder<'a> {
    pub fn new(bundle: &'a TraceBundle) -> Self {
        Self {
            bundle,
            pid: None,
            bucket_ns: 10_000_000,
            top_n: 25,
            time_window_ns: None,
            binary: None,
            dsym: None,
            slide: SlideMode::default(),
            filter: None,
        }
    }

    /// Substring filter on the resolved function name.
    /// Applied after symbolication, before truncating to `top`.
    /// Useful for cutting out stdlib/system noise.
    pub fn filter(mut self, substring: Option<String>) -> Self {
        self.filter = substring;
        self
    }

    pub fn binary(mut self, path: Option<PathBuf>) -> Self {
        self.binary = path;
        self
    }

    pub fn dsym(mut self, path: Option<PathBuf>) -> Self {
        self.dsym = path;
        self
    }

    pub fn slide(mut self, mode: SlideMode) -> Self {
        self.slide = mode;
        self
    }

    pub fn pid(mut self, pid: Pid) -> Self {
        self.pid = Some(pid);
        self
    }

    pub fn bucket_ns(mut self, ns: u64) -> Self {
        self.bucket_ns = ns.max(1);
        self
    }

    pub fn top(mut self, n: usize) -> Self {
        self.top_n = n;
        self
    }

    pub fn time_window_ns(mut self, start: u64, end: u64) -> Self {
        self.time_window_ns = Some((start, end));
        self
    }

    pub fn run(self) -> Result<HotspotReport> {
        let xml = self
            .bundle
            .xctrace()
            .export_xpath(self.bundle.path(), TIME_SAMPLE_XPATH)?;
        let mut reader = RowReader::new(std::io::Cursor::new(xml));

        let mut total_samples: u64 = 0;
        let mut per_cpu: BTreeMap<CoreId, CpuStats> = BTreeMap::new();
        let mut pc_counts: HashMap<RuntimePc, PcAccumulator> = HashMap::new();
        let mut bucket_map: BTreeMap<u64, BTreeMap<CoreId, u64>> = BTreeMap::new();
        let mut origin_ns: Option<u64> = None;

        while let Some(ev) = reader.next_event()? {
            let RowReaderEvent::Row(cells) = ev else {
                continue;
            };
            let Some(sample) = parse_time_sample(&cells) else {
                continue;
            };
            if let Some(want_pid) = self.pid
                && sample.pid != want_pid
            {
                continue;
            }
            if let Some((lo, hi)) = self.time_window_ns
                && (sample.time.ns() < lo || sample.time.ns() >= hi)
            {
                continue;
            }
            let Some(core) = sample.core else { continue };

            total_samples += 1;
            let entry = per_cpu.entry(core).or_default();
            entry.samples += 1;
            if entry.label.is_none() {
                entry.label = sample.core_label;
            }

            let origin = *origin_ns.get_or_insert(sample.time.ns());
            let bucket_key = (sample.time.ns().saturating_sub(origin)) / self.bucket_ns;
            *bucket_map
                .entry(bucket_key)
                .or_default()
                .entry(core)
                .or_default() += 1;

            if let Some(pc) = sample.pc {
                let e = pc_counts.entry(pc).or_insert_with(|| PcAccumulator {
                    samples: 0,
                    fmt: sample.pc_fmt.clone(),
                });
                e.samples += 1;
            }
        }

        let mut timeline: Vec<TimelineBucket> = bucket_map
            .into_iter()
            .map(|(k, samples_per_cpu)| {
                let start_ns = k * self.bucket_ns;
                TimelineBucket {
                    start_ns,
                    end_ns: start_ns + self.bucket_ns,
                    samples_per_cpu,
                }
            })
            .collect();
        timeline.sort_by_key(|b| b.start_ns);

        let mut top_pcs: Vec<Hotspot> = pc_counts
            .into_iter()
            .map(|(pc, acc)| Hotspot {
                pc,
                samples: acc.samples,
                fmt: acc.fmt,
                function: None,
                file: None,
                line: None,
            })
            .collect();
        top_pcs.sort_by(by_samples);
        // If no filter, truncate immediately to avoid symbolicating PCs
        // we won't keep. With a filter we have to symbolicate everything
        // before we know what survives.
        if self.filter.is_none() {
            top_pcs.truncate(self.top_n);
        }

        if self.binary.is_some() || self.dsym.is_some() {
            let slide = match &self.slide {
                SlideMode::Manual(s) => Some(*s),
                SlideMode::Auto => match self.binary.as_deref() {
                    Some(bin) => {
                        let info = BinaryInfo::open(bin)?;
                        let loads = match self.bundle.image_loads() {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::warn!("image_loads failed: {e}");
                                Vec::new()
                            }
                        };
                        let s = info.slide_from(&loads);
                        match s {
                            Some(s) => {
                                tracing::info!(
                                    %s,
                                    "auto-detected slide from kdebug DBG_DYLD events"
                                );
                            }
                            None => {
                                tracing::warn!(
                                    "could not auto-detect slide; symbols will be wrong. \
                                     Enumerate candidates with `BinaryInfo::enumerate_slides` and \
                                     pass one through `SlideMode::Manual`."
                                );
                            }
                        }
                        s
                    }
                    None => None,
                },
            };
            let opts = SymbolicatorOptions {
                binary: self.binary.clone(),
                dsym: self.dsym.clone(),
                slide: slide.unwrap_or(Slide::ZERO),
            };
            let sym = Symbolicator::new(opts)?;
            for h in &mut top_pcs {
                if let Ok(frame) = sym.resolve(h.pc) {
                    h.function = frame.function;
                    h.file = frame.file;
                    h.line = frame.line;
                }
            }
        }

        if let Some(needle) = &self.filter {
            let needle_lower = needle.to_lowercase();
            top_pcs.retain(|h| {
                h.function
                    .as_deref()
                    .map(|f| f.to_lowercase().contains(&needle_lower))
                    .unwrap_or(false)
            });
            top_pcs.truncate(self.top_n);
        }

        Ok(HotspotReport {
            total_samples,
            per_cpu,
            timeline_buckets_ns: self.bucket_ns,
            timeline,
            top_pcs,
        })
    }
}

/// Rank two program counters: busiest first, address ascending.
///
/// The address is what makes this a total order, and a total order is what
/// makes the report reproducible.
/// A real trace has a long tail of counters tied at one or two samples, and
/// these come out of a `HashMap`, whose iteration order differs between
/// processes.
/// Ordering on the sample count alone therefore leaves the ties in an arbitrary
/// sequence, and truncating to the busiest N then keeps an arbitrary subset of
/// them: two reads of the same bundle report different frames.
///
/// Ascending rather than descending, and not arbitrarily.
/// Any second key would make the order total, but the app's own images load
/// below the dyld shared cache, so preferring low addresses keeps the tied
/// counters that can be given a symbol — which is what a caller asked for.
/// Reversing this stays reproducible and quietly fills the table with addresses
/// instead.
fn by_samples(a: &Hotspot, b: &Hotspot) -> Ordering {
    b.samples.cmp(&a.samples).then_with(|| a.pc.cmp(&b.pc))
}

const TIME_SAMPLE_XPATH: &str = "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"time-sample\"]";

#[derive(Debug)]
struct PcAccumulator {
    samples: u64,
    fmt: Option<String>,
}

#[derive(Debug)]
struct ParsedSample {
    time: SampleTime,
    pid: Pid,
    core: Option<CoreId>,
    core_label: Option<String>,
    pc: Option<RuntimePc>,
    pc_fmt: Option<String>,
    thread_state: Option<String>,
}

impl Default for ParsedSample {
    fn default() -> Self {
        Self {
            time: SampleTime::new(0),
            pid: Pid::unknown(),
            core: None,
            core_label: None,
            pc: None,
            pc_fmt: None,
            thread_state: None,
        }
    }
}

fn parse_time_sample(cells: &[Rc<Cell>]) -> Option<ParsedSample> {
    let mut s = ParsedSample::default();
    for cell in cells {
        match cell.element() {
            Some("sample-time") => {
                s.time = SampleTime::new(cell.as_u64().unwrap_or(0));
            }
            Some("thread") => {
                if let Some(pid_cell) = cell.find("pid") {
                    s.pid = Pid::new(pid_cell.as_i64().unwrap_or(-1));
                }
            }
            Some("core") => {
                s.core = cell.as_u64().map(|v| CoreId::new(v as u32));
                s.core_label = cell.fmt().map(str::to_string);
            }
            Some("thread-state") => {
                s.thread_state = cell.text().map(str::to_string);
            }
            Some("kperf-bt") => {
                if let Some(pc_cell) = cell.find("text-address") {
                    s.pc = pc_cell.as_u64().map(RuntimePc::new);
                    s.pc_fmt = pc_cell.fmt().map(str::to_string);
                }
            }
            _ => {}
        }
    }
    if s.time.ns() == 0 && s.core.is_none() {
        return None;
    }
    if matches!(s.thread_state.as_deref(), Some("Blocked")) {
        return None;
    }
    Some(s)
}

impl HotspotReport {
    pub fn empty(bucket_ns: u64) -> Self {
        Self {
            total_samples: 0,
            per_cpu: BTreeMap::new(),
            timeline_buckets_ns: bucket_ns,
            timeline: Vec::new(),
            top_pcs: Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "hotspots_tests.rs"]
mod tests;
