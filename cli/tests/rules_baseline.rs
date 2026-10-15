//! The committed `.nightshift/rules-baseline.json` is what `ns rules baseline` writes for this
//! checkout's findings, byte for byte, so a hand-edited or stale entry fails CI.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

mod common;

use std::path::{Path, PathBuf};

use serde_json::Value;

fn checkout() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn ns_rules_baseline_dry_run_reproduces_the_committed_baseline() {
    assert!(checkout().join(".nightshift/rules-baseline.json").is_file());
    // The rules row runs clippy over `cli/`, which recompiles the crate on a loaded machine.
    let out = common::ns()
        .args(["rules", "baseline", "--dry-run"])
        .current_dir(checkout())
        .timeout(common::TIMEOUT * 10)
        .output();
    let stderr = String::from_utf8_lossy(&out.stderr);
    let v: Value = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("ns rules baseline printed no JSON ({e}): {stderr}"));
    assert_eq!(
        v["status"], "already-done",
        "ns rules baseline would rewrite .nightshift/rules-baseline.json; rerun it and commit the file:\n  ns rules baseline\n{v:#}\n{stderr}"
    );
}
