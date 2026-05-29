//! Parser for dhat heap-profile JSON.
//!
//! Each program point in the file carries the cumulative byte/block totals for
//! one unique allocation stack.
//! Frame strings in `ftbl` are pre-demangled by dhat-rs (via the `backtrace`
//! crate's `{:#}` formatting), so we use them as-is.

use std::collections::BTreeMap;

use rustc_demangle::demangle;
use serde::Deserialize;

/// One allocation site / call stack in the heap profile.
#[derive(Debug, Clone)]
pub(crate) struct ProgramPoint {
    /// Cumulative bytes ever allocated through this stack.
    pub total_bytes: u64,
    /// Cumulative allocation count.
    pub total_blocks: u64,
    /// Bytes still live at the global peak (sum across all PPs = peak total).
    pub peak_bytes: u64,
    /// Blocks still live at the global peak.
    pub peak_blocks: u64,
    /// Bytes still live at profile end.
    pub end_bytes: u64,
    /// Blocks still live at profile end.
    pub end_blocks: u64,
    /// Stack frames, leaf-first (`frames[0]` = closest to the allocation).
    pub frames: Vec<String>,
}

/// Parsed profile plus its high-level summary.
#[derive(Debug)]
pub(crate) struct Profile {
    pub elapsed_units: u64,
    pub time_unit: String,
    pub total_bytes: u64,
    pub total_blocks: u64,
    pub peak_bytes: u64,
    pub peak_blocks: u64,
    pub end_bytes: u64,
    pub end_blocks: u64,
    pub program_points: Vec<ProgramPoint>,
}

impl ProgramPoint {
    /// Return the first frame that looks like JP code, falling back to the raw
    /// leaf if no jp-prefixed frame exists in the stack.
    ///
    /// Heap traces bottom out in allocator plumbing (`<Global as Allocator>::
    /// allocate`, `<RawVec>::with_capacity_in`, etc.) which carries no signal
    /// about who called it.
    /// The `jp_` heuristic matches anything from our own crates — directly
    /// (`jp_config::PartialAppConfig::clone`), through trait impls
    /// (`<jp_conversation::Stream as Extend>::extend`), and as generic
    /// parameters (`drop_in_place::<jp_config::...>`).
    pub(crate) fn interesting_leaf(&self) -> &str {
        for frame in &self.frames {
            if frame.contains("jp_") {
                return frame;
            }
        }
        self.frames.first().map_or("[unknown]", String::as_str)
    }
}

impl Profile {
    /// Aggregate program points by their interesting (jp-prefixed) leaf, sorted
    /// by allocation count.
    pub(crate) fn aggregate_by_leaf(&self) -> Vec<LeafAgg> {
        let mut map: BTreeMap<&str, LeafAgg> = BTreeMap::new();
        for pp in &self.program_points {
            let leaf = pp.interesting_leaf();
            let entry = map.entry(leaf).or_default();
            entry.total_bytes += pp.total_bytes;
            entry.total_blocks += pp.total_blocks;
            entry.peak_bytes += pp.peak_bytes;
            entry.sites += 1;
        }
        let mut vec: Vec<LeafAgg> = map
            .into_iter()
            .map(|(leaf, mut agg)| {
                agg.leaf = leaf.to_owned();
                agg
            })
            .collect();
        vec.sort_by_key(|agg| std::cmp::Reverse(agg.total_blocks));
        vec
    }
}

/// Aggregate of all program points sharing the same leaf frame.
#[derive(Debug, Default)]
pub(crate) struct LeafAgg {
    pub leaf: String,
    pub total_bytes: u64,
    pub total_blocks: u64,
    pub peak_bytes: u64,
    pub sites: usize,
}

/// Parse a dhat JSON profile.
pub(crate) fn parse(json: &str) -> Result<Profile, serde_json::Error> {
    let raw: Raw = serde_json::from_str(json)?;
    let ftbl = raw.ftbl;

    let program_points: Vec<ProgramPoint> = raw
        .pps
        .into_iter()
        .map(|pp| ProgramPoint {
            total_bytes: pp.tb,
            total_blocks: pp.tbk,
            peak_bytes: pp.gb,
            peak_blocks: pp.gbk,
            end_bytes: pp.eb,
            end_blocks: pp.ebk,
            frames: pp
                .fs
                .into_iter()
                .map(|idx| {
                    ftbl.get(idx)
                        .map_or_else(|| "[unknown]".to_owned(), |raw| clean_frame(raw))
                })
                .filter(|frame| !is_dispatch_noise(frame))
                .collect(),
        })
        .collect();

    let total_bytes = program_points.iter().map(|pp| pp.total_bytes).sum();
    let total_blocks = program_points.iter().map(|pp| pp.total_blocks).sum();
    let peak_bytes = program_points.iter().map(|pp| pp.peak_bytes).sum();
    let peak_blocks = program_points.iter().map(|pp| pp.peak_blocks).sum();
    let end_bytes = program_points.iter().map(|pp| pp.end_bytes).sum();
    let end_blocks = program_points.iter().map(|pp| pp.end_blocks).sum();

    Ok(Profile {
        elapsed_units: raw.te,
        time_unit: raw.tu,
        total_bytes,
        total_blocks,
        peak_bytes,
        peak_blocks,
        end_bytes,
        end_blocks,
        program_points,
    })
}

/// Clean up a dhat frame string: strip the leading instruction-address prefix
/// (` 0xADDR:  `), demangle the symbol if needed, and shorten common stdlib
/// paths.
///
/// dhat-rs writes frames already demangled — the `_` check is here for
/// robustness if a profile from an older or differently-configured run emits
/// raw mangled symbols.
fn clean_frame(raw: &str) -> String {
    let symbol = strip_address_prefix(raw);
    let demangled = if symbol.starts_with('_') {
        format!("{:#}", demangle(symbol))
    } else {
        symbol.to_owned()
    };
    polish_frame(&demangled)
}

/// Shorten common stdlib module paths so frame strings fit on one line.
///
/// The replacements are conservative: only well-known prefixes that every Rust
/// developer would recognize without disambiguation.
/// Crate-internal module paths (`jp_config::conversation::tool::...`) are
/// preserved — they tell us *where* in our own code the work is happening.
fn polish_frame(s: &str) -> String {
    // Apply each replacement in turn. Order matters where one pattern is
    // a prefix of another (longer matches first).
    let mut out = s.to_owned();
    for (from, to) in REPLACEMENTS {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

/// Verbose stdlib path → short form.
/// Listed longest-first when one is a prefix of another, otherwise order
/// doesn't matter.
const REPLACEMENTS: &[(&str, &str)] = &[
    // Trait paths
    ("core::ops::function::FnOnce", "FnOnce"),
    ("core::ops::function::FnMut", "FnMut"),
    ("core::ops::function::Fn", "Fn"),
    ("core::iter::traits::iterator::Iterator", "Iterator"),
    ("core::iter::traits::collect::Extend", "Extend"),
    ("core::iter::traits::collect::FromIterator", "FromIterator"),
    ("core::clone::Clone", "Clone"),
    ("core::default::Default", "Default"),
    ("core::convert::From", "From"),
    ("core::convert::Into", "Into"),
    ("core::cmp::PartialEq", "PartialEq"),
    // Adapter types and helpers
    ("core::iter::adapters::map::map_fold", "map_fold"),
    ("core::iter::adapters::map::Map", "Map"),
    ("core::iter::adapters::cloned::Cloned", "Cloned"),
    ("core::iter::adapters::filter_map::FilterMap", "FilterMap"),
    ("core::iter::adapters::filter::Filter", "Filter"),
    ("core::slice::iter::Iter", "slice::Iter"),
    ("core::ptr::drop_in_place", "drop_in_place"),
    // Container types
    ("alloc::string::String", "String"),
    ("alloc::vec::Vec", "Vec"),
    ("alloc::boxed::Box", "Box"),
    ("alloc::sync::Arc", "Arc"),
    ("alloc::rc::Rc", "Rc"),
    ("alloc::raw_vec::RawVec", "RawVec"),
    ("alloc::collections::btree::map::BTreeMap", "BTreeMap"),
    ("alloc::collections::btree::set::BTreeSet", "BTreeSet"),
    ("alloc::collections::linked_list::LinkedList", "LinkedList"),
    ("std::collections::hash::map::HashMap", "HashMap"),
    ("std::collections::hash::set::HashSet", "HashSet"),
    ("std::vec::Vec", "Vec"),
    // Primitive paths that show up in turbofishes
    ("core::option::Option", "Option"),
    ("core::result::Result", "Result"),
];

/// Frames that are pure dispatch/trampoline boilerplate — carrying no
/// information about *who* or *what* is being called, only *how* the call gets
/// there.
///
/// Run on POLISHED frame strings (i.e. after [`polish_frame`]), so the patterns
/// match the short forms (`FnMut`, `Map`, `Cloned`, ...).
fn is_dispatch_noise(symbol: &str) -> bool {
    // Closure trampolines via the Fn family: `<X as FnMut<Args>>::call_mut`.
    // These show up as a layer between *whoever holds the closure* and the
    // closure body, adding no information.
    if symbol.contains(" as FnOnce<") && symbol.contains(">::call_once") {
        return true;
    }
    if symbol.contains(" as FnMut<") && symbol.contains(">::call_mut") {
        return true;
    }
    if symbol.contains(" as Fn<") && symbol.contains(">::call(") {
        return true;
    }

    // `map_fold` is `Iterator::fold`'s internal helper for `Map`. Always
    // sandwiched between two iterator-chain frames; contributes nothing.
    if symbol.starts_with("map_fold::") || symbol.starts_with("map_fold ") {
        return true;
    }

    // Iterator-chain wrappers: `<Map<...> as Iterator>::fold`,
    // `<Cloned<...> as Iterator>::for_each`, etc. The wrappers themselves
    // do nothing — they delegate to the inner iterator. We keep `Iterator`
    // method frames when the implementing type is *not* a stdlib adapter
    // (so our own `ConversationStream::IntoIter::next` etc. survives).
    if symbol.contains(" as Iterator>::")
        && (symbol.contains("<Map<")
            || symbol.contains("<Cloned<")
            || symbol.contains("<FilterMap<")
            || symbol.contains("<Filter<")
            || symbol.contains("<slice::Iter<"))
    {
        return true;
    }

    // `Vec::clone` / `Vec::clone_from` specialization helpers. The actual
    // work (`T::clone()` on each element) happens upstream; these are
    // inlined dispatch through `SpecExtend`, `SpecCloneIntoVec`,
    // `extend_trusted`, etc. They contribute nothing the parent
    // `<Vec<T> as Clone>::clone[_from]` frame doesn't already say.
    if symbol.contains("alloc::vec::spec_extend::SpecExtend")
        || symbol.contains("alloc::slice::SpecCloneIntoVec")
        || symbol.contains("::extend_trusted::")
        || symbol.contains("::extend_from_slice (")
        || symbol.contains("::extend_from_slice\n")
    {
        return true;
    }

    // `<T as <[_]>::to_vec_in::ConvertVec>::to_vec` and `<[T]>::to_vec_in`
    // are slice-to-vec clone specializations. Same story as above.
    if symbol.contains("ConvertVec>::to_vec") || symbol.contains("]>::to_vec_in::") {
        return true;
    }

    false
}

/// Strip a leading ` 0xHEX:  ` prefix from a frame string.
/// The instruction address is useful to a debugger and noise to a reader, so we
/// drop it.
fn strip_address_prefix(raw: &str) -> &str {
    let Some(rest) = raw.strip_prefix("0x") else {
        return raw;
    };
    let hex_end = rest
        .find(|c: char| !c.is_ascii_hexdigit())
        .unwrap_or(rest.len());
    let after_hex = &rest[hex_end..];
    // Expect `: ` immediately after the address.
    if let Some(stripped) = after_hex.strip_prefix(": ") {
        stripped
    } else {
        raw
    }
}

#[derive(Deserialize)]
struct Raw {
    // dhat's `cmd` field is the program's argv joined as a string; we
    // already have the launched-command args from the tool's parameters,
    // so the JSON's view of it is unused. Serde drops unknown fields by
    // default, so we don't need to declare it.
    #[serde(default)]
    te: u64,
    #[serde(default)]
    tu: String,
    pps: Vec<RawPP>,
    ftbl: Vec<String>,
}

#[derive(Deserialize)]
struct RawPP {
    tb: u64,
    tbk: u64,
    #[serde(default)]
    gb: u64,
    #[serde(default)]
    gbk: u64,
    #[serde(default)]
    eb: u64,
    #[serde(default)]
    ebk: u64,
    fs: Vec<usize>,
}

#[cfg(test)]
#[path = "profile_heap_parse_tests.rs"]
mod tests;
