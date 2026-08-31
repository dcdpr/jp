//! Newtypes for addresses, slides, sample times, core ids, and pids.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A program counter as observed at runtime (after ASLR slide).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RuntimePc(u64);

impl RuntimePc {
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
    /// Subtract a slide to recover the file (preferred) address.
    pub fn to_file(self, slide: Slide) -> Option<FilePc> {
        self.0.checked_sub(slide.0).map(FilePc)
    }
}

impl fmt::Display for RuntimePc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

impl fmt::LowerHex for RuntimePc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

/// A program counter as encoded in the Mach-O binary (preferred address, before
/// any ASLR slide is applied at load time).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FilePc(u64);

impl FilePc {
    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
    pub fn to_runtime(self, slide: Slide) -> RuntimePc {
        RuntimePc(self.0.wrapping_add(slide.0))
    }
}

impl fmt::Display for FilePc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:016x}", self.0)
    }
}

impl fmt::LowerHex for FilePc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

/// ASLR slide: how far the binary's `__TEXT` was shifted at load time.
/// Always page-aligned (0x4000 on Apple Silicon).
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Slide(u64);

impl Slide {
    pub const ZERO: Self = Self(0);
    pub const fn new(offset: u64) -> Self {
        Self(offset)
    }
    pub const fn raw(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Slide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "0x{:x}", self.0)
    }
}

impl fmt::LowerHex for Slide {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::LowerHex::fmt(&self.0, f)
    }
}

/// A nanosecond timestamp from the trace's `sample-time` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SampleTime(u64);

impl SampleTime {
    pub const fn new(ns: u64) -> Self {
        Self(ns)
    }
    pub const fn ns(self) -> u64 {
        self.0
    }
    pub fn ms(self) -> u64 {
        self.0 / 1_000_000
    }
}

/// Logical CPU id (P-core / E-core index).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CoreId(u32);

impl CoreId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl fmt::Display for CoreId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A POSIX process id.
/// Negative values mean "unknown" in trace context.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Pid(i64);

impl Pid {
    pub const fn new(id: i64) -> Self {
        Self(id)
    }
    pub const fn raw(self) -> i64 {
        self.0
    }
    pub const fn unknown() -> Self {
        Self(-1)
    }
}

impl fmt::Display for Pid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
