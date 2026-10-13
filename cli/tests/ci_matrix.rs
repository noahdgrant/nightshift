//! The rule pack matrix `.github/workflows/ci.yml` builds from `scripts/rule-packs.sh`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

fn real_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The matrix entries the `rule-pack-list` job in ci.yml builds from the real
/// `scripts/rule-packs.sh` output, by running the `jq` filter written in that job.
fn ci_matrix() -> Vec<serde_json::Value> {
    let ci = fs::read_to_string(real_root().join(".github/workflows/ci.yml")).unwrap();
    let filter = ci
        .lines()
        .find_map(|l| l.split_once("jq -Rcn '")?.1.strip_suffix("' <<<\"$list\")"))
        .expect("the rule-pack-list job runs jq -Rcn '<filter>' <<<\"$list\"")
        .to_owned();
    let list = common::command("bash")
        .arg(real_root().join("scripts/rule-packs.sh"))
        .output();
    assert!(list.status.success());
    let out = common::command("jq")
        .args(["-Rcn", &filter])
        .write_stdin(list.stdout)
        .output();
    assert!(out.status.success());
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn the_matrix_names_keep_the_required_check_names_of_the_main_ruleset() {
    let matrix = ci_matrix();
    for (pack, name) in [
        ("python", "python"),
        ("c", "C"),
        ("js", "JavaScript/TypeScript"),
        ("java", "Java"),
    ] {
        assert!(
            matrix.contains(&serde_json::json!({"pack": pack, "name": name})),
            "{pack} / {name} missing from {matrix:?}"
        );
    }
}
