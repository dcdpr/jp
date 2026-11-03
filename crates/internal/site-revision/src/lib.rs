//! The revision of the site files `jp vite` writes into a project.
//!
//! The site's loaders read what `jp ticket` and `jp rfd` write, so the three
//! plugins have to agree about those formats.
//! Each carries this revision, computed when it is built from the site files in
//! `crates/plugins/command/vite/site/`, and `jp vite serve` refuses to trust a
//! site whose plugins carry different ones.
//!
//! The revision is a hash of the files' paths and contents.
//! It changes when the site files change and at no other time, so two builds
//! from different commits that leave the site alone agree on it, and a project
//! that has synced the site can check that its copy came from the `jp vite` it
//! has installed.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// One file of the site, as [`files`] finds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteFile {
    /// The path below the site directory, with `/` between components.
    pub path: String,

    /// Where the file is on disk.
    pub source: PathBuf,
}

/// Every file under `dir`, ordered by [`SiteFile::path`].
///
/// Files and directories whose names start with a dot are left out: they are
/// what an editor or a file manager leaves behind (`.DS_Store`), and a stray
/// one would otherwise change the revision on one machine only.
///
/// # Errors
///
/// Returns the I/O error of a directory or file that can't be read.
pub fn files(dir: &Path) -> io::Result<Vec<SiteFile>> {
    let mut found = vec![];
    collect(dir, "", &mut found)?;
    found.sort_by(|a, b| a.path.cmp(&b.path));

    Ok(found)
}

/// The revision of the site files under `dir`, as sixteen lowercase hex digits.
///
/// Line endings are read as `\n` whatever the checkout wrote, so a Windows
/// checkout agrees with a Unix one.
///
/// # Errors
///
/// Returns the I/O error of a directory or file that can't be read.
pub fn revision(dir: &Path) -> io::Result<String> {
    let mut hash = Fnv::default();

    for file in files(dir)? {
        let contents = fs::read(&file.source)?;
        let contents = without_carriage_returns(&contents);

        hash.write(file.path.as_bytes());
        hash.write(&[0]);
        hash.write(&(contents.len() as u64).to_le_bytes());
        hash.write(&contents);
    }

    Ok(format!("{:016x}", hash.0))
}

/// Add the files under `dir` to `found`, `prefix` being the path of `dir` below
/// the site directory.
fn collect(dir: &Path, prefix: &str, found: &mut Vec<SiteFile>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "{}: a site file's name is not UTF-8",
                    entry.path().display()
                ),
            ));
        };

        if name.starts_with('.') {
            continue;
        }

        let path = if prefix.is_empty() {
            name.to_owned()
        } else {
            format!("{prefix}/{name}")
        };

        if entry.file_type()?.is_dir() {
            collect(&entry.path(), &path, found)?;
        } else {
            found.push(SiteFile {
                path,
                source: entry.path(),
            });
        }
    }

    Ok(())
}

/// `bytes` with every `\r\n` read as `\n`.
fn without_carriage_returns(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            continue;
        }
        out.push(*byte);
    }

    out
}

/// 64-bit FNV-1a: small, and the same on every machine and toolchain, which
/// `std`'s hasher does not promise.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
