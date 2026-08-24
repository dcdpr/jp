use std::{cmp::Ordering, collections::HashMap, rc::Rc};

use crate::{
    address::{Pid, RuntimePc},
    error::Result,
    trace::TraceBundle,
    xml::{
        Cell,
        stream::{RowReader, RowReaderEvent},
    },
};

#[derive(Debug, Clone, Copy)]
pub struct PcSample {
    pub pc: RuntimePc,
    pub samples: u64,
}

/// One sampled callstack.
/// `frames[0]` is the innermost (leaf) PC; later indices are progressively
/// deeper in the call chain (callers, caller's caller, ...).
#[derive(Debug, Clone)]
pub struct Callstack {
    pub frames: Vec<RuntimePc>,
}

impl TraceBundle {
    /// Per-PC sample counts from the `time-sample` table (running-state rows
    /// only).
    /// Sorted descending by count, then ascending by address, so the same table
    /// always yields the same sequence.
    pub fn pc_samples(&self, pid: Option<Pid>) -> Result<Vec<PcSample>> {
        let xml = self
            .xctrace()
            .export_xpath(self.path(), TIME_SAMPLE_XPATH)?;
        let mut reader = RowReader::new(std::io::Cursor::new(xml));
        let mut counts: HashMap<RuntimePc, u64> = HashMap::new();
        while let Some(ev) = reader.next_event()? {
            let RowReaderEvent::Row(cells) = ev else {
                continue;
            };
            let mut sample_pid: i64 = -1;
            let mut state: Option<&str> = None;
            let mut pc: Option<RuntimePc> = None;
            for cell in &cells {
                match cell.element() {
                    Some("thread") => {
                        if let Some(pidc) = cell.find("pid") {
                            sample_pid = pidc.as_i64().unwrap_or(-1);
                        }
                    }
                    Some("thread-state") => {
                        state = match cell.as_ref() {
                            Cell::Leaf(l) => Some(l.text.as_str()),
                            _ => None,
                        };
                    }
                    Some("kperf-bt") => {
                        if let Some(pcc) = cell.find("text-address") {
                            pc = pcc.as_u64().map(RuntimePc::new);
                        }
                    }
                    _ => {}
                }
            }
            if state == Some("Blocked") {
                continue;
            }
            if let Some(want) = pid
                && Pid::new(sample_pid) != want
            {
                continue;
            }
            let Some(pc) = pc else { continue };
            *counts.entry(pc).or_insert(0) += 1;
        }
        let mut out: Vec<PcSample> = counts
            .into_iter()
            .map(|(pc, samples)| PcSample { pc, samples })
            .collect();
        out.sort_by(by_samples);
        Ok(out)
    }

    /// Full per-sample callstacks from `time-sample` (running-state rows only).
    /// One `Callstack` per row; identical stacks are NOT deduped - the caller
    /// can aggregate as needed.
    pub fn callstacks(&self, pid: Option<Pid>) -> Result<Vec<Callstack>> {
        let xml = self
            .xctrace()
            .export_xpath(self.path(), TIME_SAMPLE_XPATH)?;
        let mut reader = RowReader::new(std::io::Cursor::new(xml));
        let mut out: Vec<Callstack> = Vec::new();
        while let Some(ev) = reader.next_event()? {
            let RowReaderEvent::Row(cells) = ev else {
                continue;
            };
            let mut sample_pid: i64 = -1;
            let mut state: Option<&str> = None;
            let mut user_bt: Option<&Rc<Cell>> = None;
            for cell in &cells {
                match cell.element() {
                    Some("thread") => {
                        if let Some(pidc) = cell.find("pid") {
                            sample_pid = pidc.as_i64().unwrap_or(-1);
                        }
                    }
                    Some("thread-state") => {
                        state = match cell.as_ref() {
                            Cell::Leaf(l) => Some(l.text.as_str()),
                            _ => None,
                        };
                    }
                    Some("kperf-bt") => {
                        // The schema lists `cp-kernel-callstack` before
                        // `cp-user-callstack`; for user-mode samples the
                        // kernel one is sentinel, so the last non-empty
                        // kperf-bt is the user backtrace we want.
                        user_bt = Some(cell);
                    }
                    _ => {}
                }
            }
            if state == Some("Blocked") {
                continue;
            }
            if let Some(want) = pid
                && Pid::new(sample_pid) != want
            {
                continue;
            }
            let Some(bt) = user_bt else { continue };
            let Some(frames) = extract_frames(bt) else {
                continue;
            };
            if frames.is_empty() {
                continue;
            }
            out.push(Callstack { frames });
        }
        Ok(out)
    }
}

fn extract_frames(kperf_bt: &Rc<Cell>) -> Option<Vec<RuntimePc>> {
    let leaf = kperf_bt
        .find("text-address")
        .and_then(|c| c.as_u64())
        .map(RuntimePc::new)?;
    let mut frames = vec![leaf];
    if let Some(addrs) = kperf_bt.find("text-addresses")
        && let Some(text) = addrs.text()
    {
        for tok in text.split_ascii_whitespace() {
            let Ok(v) = tok.parse::<u64>() else { continue };
            if v == 0 {
                continue;
            }
            frames.push(RuntimePc::new(v));
        }
    }
    Some(frames)
}

/// Rank two program counters: busiest first, address ascending.
///
/// The address is the tie-breaker that makes this a total order.
/// Counts are accumulated in a `HashMap`, whose iteration order differs between
/// processes, so ordering on the count alone leaves every tie in an arbitrary
/// sequence — and a long tail of counters tied at one or two samples is what a
/// real trace looks like.
fn by_samples(a: &PcSample, b: &PcSample) -> Ordering {
    b.samples.cmp(&a.samples).then_with(|| a.pc.cmp(&b.pc))
}

const TIME_SAMPLE_XPATH: &str = "/trace-toc/run[@number=\"1\"]/data/table[@schema=\"time-sample\"]";

#[cfg(test)]
#[path = "samples_tests.rs"]
mod tests;
