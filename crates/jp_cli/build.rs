use std::process::Command;

use chrono::Utc;

fn main() {
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .unwrap_or_default();

    let build_date = Utc::now().format("%Y-%m-%d").to_string();
    let pkg_version = env!("CARGO_PKG_VERSION");

    println!("cargo:rustc-env=LONG_VERSION={pkg_version}-{git_hash} ({build_date})");
}
