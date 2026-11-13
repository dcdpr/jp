use std::{env, process::Command};

use chrono::{DateTime, Utc};

fn main() {
    let git_hash = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_owned())
        .unwrap_or_default();

    // Use `SOURCE_DATE_EPOCH` when set (e.g. during release builds) so builds
    // from the same commit produce an identical version string. Do not emit
    // `cargo:rerun-if-env-changed`: any `rerun-if` directive disables Cargo's
    // default package-wide rebuild trigger, which would leave `git_hash` stale
    // when source files change.
    let built = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|seconds| seconds.trim().parse().ok())
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .unwrap_or_else(Utc::now);
    let build_date = built.format("%Y-%m-%d").to_string();
    let pkg_version = env!("CARGO_PKG_VERSION");

    println!("cargo:rustc-env=LONG_VERSION=v{pkg_version}-{git_hash} ({build_date})");
}
