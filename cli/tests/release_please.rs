//! `.github/workflows/release-please.yml` opens release PRs with a token that starts CI on them.

use std::fs;
use std::path::Path;

use serde_yaml::Value;

fn workflow() -> (String, Value) {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows/release-please.yml");
    let text = fs::read_to_string(path).unwrap();
    let yaml = serde_yaml::from_str(&text).expect("release-please.yml parses as YAML");
    (text, yaml)
}

#[test]
fn the_release_please_step_passes_the_pat_with_the_github_token_as_fallback() {
    let (_, yaml) = workflow();
    let steps = yaml["jobs"]["release-please"]["steps"]
        .as_sequence()
        .expect("the release-please job has steps");
    let step = steps
        .iter()
        .find(|s| {
            s["uses"]
                .as_str()
                .is_some_and(|u| u.starts_with("googleapis/release-please-action@"))
        })
        .expect("a step uses googleapis/release-please-action");
    assert_eq!(
        step["with"]["token"].as_str(),
        Some("${{ secrets.RELEASE_PLEASE_TOKEN || github.token }}")
    );
}

#[test]
fn a_comment_names_the_secret_and_why_the_github_token_is_not_enough() {
    let (text, _) = workflow();
    let comments: String = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    for needle in ["RELEASE_PLEASE_TOKEN", "GITHUB_TOKEN", "ci.yml", "#682"] {
        assert!(comments.contains(needle), "no comment mentions {needle}");
    }
}
