//! Embed the browser UI's bundle when `JP_REVIEW_UI` names one.
//!
//! Writes `ui.rs` to `OUT_DIR`, which `src/ui.rs` includes: `BUNDLE`, holding
//! the files `wasm-bindgen --target web` and the UI's stylesheet make up, or
//! `None` when `JP_REVIEW_UI` is unset, which leaves `jp review serve` with its
//! placeholder page.
//!
//! `JP_REVIEW_UI` is the directory holding the bundle: an absolute path, or one
//! relative to this crate's directory.
//! A directory that doesn't hold a bundle fails the build, rather than leave a
//! binary meant to carry the UI with the placeholder.
//! Nothing here runs `cargo`.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

/// The variable naming the bundle's directory.
const VARIABLE: &str = "JP_REVIEW_UI";

/// The bundle's files: the JavaScript module that loads the WebAssembly, the
/// WebAssembly, and the stylesheet.
///
/// `src/ui.rs` serves them under these names.
const FILES: [&str; 3] = ["jp_review_ui.js", "jp_review_ui_bg.wasm", "style.css"];

/// What every WebAssembly module opens with.
const WASM_MAGIC: &[u8] = b"\0asm";

fn main() -> Result<(), String> {
    println!("cargo:rerun-if-env-changed={VARIABLE}");
    let out = env::var("OUT_DIR").map_err(|error| format!("OUT_DIR: {error}"))?;
    let out = PathBuf::from(out).join("ui.rs");

    let source = match env::var_os(VARIABLE) {
        Some(dir) if !dir.is_empty() => bundle(Path::new(&dir))?,
        _ => "pub(crate) static BUNDLE: Option<Bundle> = None;\n".to_owned(),
    };

    fs::write(&out, source).map_err(|error| format!("{}: {error}", out.display()))
}

/// The source of `BUNDLE`, embedding the files in `dir`.
fn bundle(dir: &Path) -> Result<String, String> {
    let dir = dir.canonicalize().map_err(|error| {
        format!(
            "{VARIABLE} names {}, which can't be read: {error}",
            dir.display()
        )
    })?;

    let mut paths = Vec::with_capacity(FILES.len());
    for name in FILES {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        if !path.is_file() {
            return Err(format!(
                "{VARIABLE} names {}, which holds no {name}: it is not the UI's bundle.",
                dir.display()
            ));
        }

        let path = path
            .into_os_string()
            .into_string()
            .map_err(|path| format!("{} is not a UTF-8 path.", PathBuf::from(path).display()))?;
        paths.push(path);
    }
    let [script, wasm, style] = paths.as_slice() else {
        return Err(format!("{VARIABLE}: expected {} files.", FILES.len()));
    };

    let module = fs::read(wasm).map_err(|error| format!("{wasm}: {error}"))?;
    if !module.starts_with(WASM_MAGIC) {
        return Err(format!("{wasm} is not a WebAssembly module."));
    }

    Ok(format!(
        "pub(crate) static BUNDLE: Option<Bundle> = Some(Bundle {{\n    script: \
         include_str!({script:?}),\n    wasm: include_bytes!({wasm:?}),\n    style: \
         include_str!({style:?}),\n}});\n"
    ))
}
