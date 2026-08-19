//! Top-N functions and per-function callee aggregation, computed from full
//! callstack samples (every `time-sample` row's backtrace, not just the leaf).

use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    path::PathBuf,
};

use serde::Serialize;

use crate::{
    address::{Pid, RuntimePc},
    analysis::{Callstack, SlideMode},
    error::Result,
    symbol::{BinaryInfo, Symbolicator, SymbolicatorOptions},
    trace::TraceBundle,
};

/// Per-function aggregated sample count.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionStat {
    pub function: String,
    pub samples: u64,
    pub fraction: f64,
}

/// Result of a `CallgraphBuilder::run()` query.
#[derive(Debug, Clone, Serialize)]
pub struct CallgraphReport {
    /// What the report measures (`"top functions (inclusive)"` or `"callees of
    /// <NAME>"`), included so the renderer can label.
    pub view: String,
    pub total_samples: u64,
    pub stats: Vec<FunctionStat>,
}

pub struct CallgraphBuilder<'a> {
    bundle: &'a TraceBundle,
    pid: Option<Pid>,
    top_n: usize,
    binary: Option<PathBuf>,
    dsym: Option<PathBuf>,
    slide: SlideMode,
    function: Option<String>,
}

impl<'a> CallgraphBuilder<'a> {
    pub fn new(bundle: &'a TraceBundle) -> Self {
        Self {
            bundle,
            pid: None,
            top_n: 10,
            binary: None,
            dsym: None,
            slide: SlideMode::default(),
            function: None,
        }
    }

    pub fn pid(mut self, pid: Pid) -> Self {
        self.pid = Some(pid);
        self
    }
    pub fn top(mut self, n: usize) -> Self {
        self.top_n = n;
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
    /// When set, the report shows top callees of this function (the next-deeper
    /// frame, toward the leaf) instead of the global top-functions view.
    pub fn function(mut self, name: Option<String>) -> Self {
        self.function = name;
        self
    }

    pub fn run(self) -> Result<CallgraphReport> {
        let stacks = self.bundle.callstacks(self.pid)?;
        let symbolicator = self.build_symbolicator()?;

        // Cache PC -> function name (or fall back to `0xADDR`). One
        // cache for the whole report so we don't re-resolve identical
        // PCs across thousands of stacks.
        //
        // We use `symbol_at` (binary-symbol lookup) instead of
        // `resolve` (DWARF inlining-aware) - for stack-frame analysis we
        // want the *concrete* function the PC lives in, not the
        // innermost inlined source function. Resolving via DWARF would
        // attribute every PC where `Vec::len` was inlined to "Vec::len",
        // wrecking the callgraph.
        let mut name_cache: HashMap<RuntimePc, String> = HashMap::new();
        let resolve = |pc: RuntimePc, cache: &mut HashMap<RuntimePc, String>| -> String {
            if let Some(name) = cache.get(&pc) {
                return name.clone();
            }
            let name = symbolicator
                .as_ref()
                .and_then(|s| s.symbol_at(pc))
                .unwrap_or_else(|| format!("0x{:x}", pc.raw()));
            cache.insert(pc, name.clone());
            name
        };

        match self.function.clone() {
            None => Ok(self.top_inclusive(&stacks, &mut name_cache, resolve)),
            Some(needle) => Ok(self.callees_of(&stacks, &needle, &mut name_cache, resolve)),
        }
    }

    fn build_symbolicator(&self) -> Result<Option<Symbolicator>> {
        if self.binary.is_none() && self.dsym.is_none() {
            return Ok(None);
        }
        let slide = match &self.slide {
            SlideMode::Manual(s) => *s,
            SlideMode::Auto => {
                let detected = match self.binary.as_deref() {
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
                            Some(s) => tracing::info!(
                                %s,
                                "auto-detected slide from kdebug DBG_DYLD events"
                            ),
                            None => tracing::warn!(
                                "could not auto-detect slide; symbols will be wrong. Enumerate \
                                 candidates with `BinaryInfo::enumerate_slides` and pass one \
                                 through `SlideMode::Manual`."
                            ),
                        }
                        s
                    }
                    None => None,
                };
                detected.unwrap_or_default()
            }
        };
        Ok(Some(Symbolicator::new(SymbolicatorOptions {
            binary: self.binary.clone(),
            dsym: self.dsym.clone(),
            slide,
        })?))
    }

    fn top_inclusive<F>(
        self,
        stacks: &[Callstack],
        cache: &mut HashMap<RuntimePc, String>,
        mut resolve: F,
    ) -> CallgraphReport
    where
        F: FnMut(RuntimePc, &mut HashMap<RuntimePc, String>) -> String,
    {
        // "Inclusive" means: a function counts once per stack it appears
        // in, regardless of depth. Dedup per stack via BTreeSet so a
        // recursive function isn't counted N times for one sample.
        let mut counts: HashMap<String, u64> = HashMap::new();
        for stack in stacks {
            let names: BTreeSet<String> =
                stack.frames.iter().map(|pc| resolve(*pc, cache)).collect();
            for name in names {
                *counts.entry(name).or_insert(0) += 1;
            }
        }
        let total = stacks.len() as u64;
        let mut stats: Vec<FunctionStat> = counts
            .into_iter()
            .map(|(function, samples)| FunctionStat {
                function,
                samples,
                fraction: if total == 0 {
                    0.0
                } else {
                    samples as f64 / total as f64
                },
            })
            .collect();
        stats.sort_by(by_samples);
        stats.truncate(self.top_n);
        CallgraphReport {
            view: "top functions (inclusive)".to_string(),
            total_samples: total,
            stats,
        }
    }

    fn callees_of<F>(
        self,
        stacks: &[Callstack],
        needle: &str,
        cache: &mut HashMap<RuntimePc, String>,
        mut resolve: F,
    ) -> CallgraphReport
    where
        F: FnMut(RuntimePc, &mut HashMap<RuntimePc, String>) -> String,
    {
        // For each stack, find the *deepest* (closest-to-root) position
        // of `needle`. The "callee" is the frame one closer to the leaf
        // - i.e. what `needle` was calling at the moment of the sample.
        // If `needle` IS the leaf, it has no callee in this sample.
        let mut counts: HashMap<String, u64> = HashMap::new();
        let mut matched_samples: u64 = 0;
        for stack in stacks {
            let mut deepest: Option<usize> = None;
            for (i, pc) in stack.frames.iter().enumerate().rev() {
                if name_matches(&resolve(*pc, cache), needle) {
                    deepest = Some(i);
                    break;
                }
            }
            let Some(idx) = deepest else { continue };
            matched_samples += 1;
            if idx == 0 {
                continue;
            }
            let callee = resolve(stack.frames[idx - 1], cache);
            *counts.entry(callee).or_insert(0) += 1;
        }
        let mut stats: Vec<FunctionStat> = counts
            .into_iter()
            .map(|(function, samples)| FunctionStat {
                function,
                samples,
                fraction: if matched_samples == 0 {
                    0.0
                } else {
                    samples as f64 / matched_samples as f64
                },
            })
            .collect();
        stats.sort_by(by_samples);
        stats.truncate(self.top_n);
        CallgraphReport {
            view: format!("callees of {needle}"),
            total_samples: matched_samples,
            stats,
        }
    }
}

/// Rank two functions: busiest first, name ascending.
///
/// The name is what makes this a total order.
/// Every frame on one call chain carries that chain's whole sample count, so
/// ties are the common case rather than the exception, and these come out of a
/// `HashMap` whose iteration order differs between processes.
/// Without the second key, truncating to the top N keeps an arbitrary subset of
/// the tie and two reads of one bundle disagree.
fn by_samples(a: &FunctionStat, b: &FunctionStat) -> Ordering {
    b.samples
        .cmp(&a.samples)
        .then_with(|| a.function.cmp(&b.function))
}

fn name_matches(haystack: &str, needle: &str) -> bool {
    haystack == needle || haystack.contains(needle)
}

#[cfg(test)]
#[path = "callgraph_tests.rs"]
mod tests;
