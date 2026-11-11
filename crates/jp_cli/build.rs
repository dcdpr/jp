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

    // A release build sets `SOURCE_DATE_EPOCH` to the time of the commit it
    // builds, so the version names the day the release was cut, and two builds
    // of one commit agree. No `rerun-if-env-changed`: any `rerun-if` directive
    // stops Cargo from running this script again when the package's files
    // change, which keeps the hash above current.
    let built = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|seconds| seconds.trim().parse().ok())
        .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        .unwrap_or_else(Utc::now);
    let build_date = built.format("%Y-%m-%d").to_string();
    let pkg_version = env!("CARGO_PKG_VERSION");

    println!("cargo:rustc-env=LONG_VERSION=v{pkg_version}-{git_hash} ({build_date})");
}
