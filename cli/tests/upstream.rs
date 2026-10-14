#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod common;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
        .arg(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    fs::write(dir.join(file), text).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-qm", file]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A checkout with skills/ns-a recording `acme/skills@<sha>:a.md`, whose upstream is a local
/// bare repo under `up/` that changed a.md after that sha.
struct World {
    _tmp: TempDir,
    root: PathBuf,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let work = root.join("work");
    fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    commit(&work, "b.md", "b\n");
    let recorded = commit(&work, "a.md", "old a\n");
    commit(&work, "a.md", "new a\n");
    let bare = root.join("up/acme/skills");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    git(&work, &["push", "-q", bare.to_str().unwrap(), "HEAD:main"]);

    let repo = root.join("repo");
    let skill = repo.join("skills/ns-a");
    fs::create_dir_all(&skill).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    fs::write(
        skill.join("SKILL.md"),
        format!(
            "---\nname: ns-a\ndescription: x\nmetadata:\n  upstream: acme/skills@{}:a.md\n---\n",
            &recorded[..12]
        ),
    )
    .unwrap();
    World { _tmp: tmp, root }
}

fn ns(w: &World) -> common::Ns {
    let mut c = common::ns();
    c.current_dir(w.root.join("repo"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_COUNT", "1")
        .env(
            "GIT_CONFIG_KEY_0",
            format!("url.file://{}/.insteadOf", w.root.join("up").display()),
        )
        .env("GIT_CONFIG_VALUE_0", "https://github.com/");
    c
}

#[test]
fn status_prints_json_naming_the_skills_whose_upstream_path_changed() {
    let w = world();
    let out = ns(&w)
        .args(["upstream", "status"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["changed"], 1);
    assert_eq!(v["skills"][0]["skill"], "ns-a");
    assert_eq!(v["skills"][0]["upstreams"][0]["changed"], true);
    assert!(w
        .root
        .join("repo/.git/ns/upstream/acme/skills/HEAD")
        .is_file());

    let out = ns(&w)
        .args(["upstream", "status", "--human"])
        .assert()
        .success()
        .get_output()
        .clone();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("ns-a: acme/skills@"), "{text}");
    assert!(
        text.contains(": changed\n1 upstream paths changed"),
        "{text}"
    );
}

#[test]
fn status_exits_1_when_an_upstream_cannot_be_fetched() {
    let w = world();
    let skill = w.root.join("repo/skills/ns-gone");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: ns-gone\ndescription: x\nmetadata:\n  upstream: acme/gone@abcdef1:a.md\n---\n",
    )
    .unwrap();
    let md = w.root.join("repo/skills/ns-a/SKILL.md");
    let still = w.root.join("repo/skills/ns-still");
    fs::create_dir_all(&still).unwrap();
    let text = fs::read_to_string(md).unwrap();
    fs::write(
        still.join("SKILL.md"),
        text.replace("ns-a", "ns-still").replace(":a.md", ":b.md"),
    )
    .unwrap();
    let out = ns(&w)
        .args(["upstream", "status", "--human"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains(":b.md: unchanged\n"), "{text}");
    assert!(
        text.contains("ns-gone: acme/gone@abcdef1:a.md: error: git fetch"),
        "{text}"
    );
}

#[test]
fn diff_prints_the_upstream_diff_and_human_labels_it() {
    let w = world();
    let out = ns(&w)
        .args(["upstream", "diff", "ns-a"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let diff = v["upstreams"][0]["diff"].as_str().unwrap();
    assert!(diff.contains("-old a\n+new a"), "{diff}");

    let md = w.root.join("repo/skills/ns-a/SKILL.md");
    let up = v["upstreams"][0]["upstream"].as_str().unwrap();
    let head = v["upstreams"][0]["head"].as_str().unwrap();
    let both = format!("\n    - {up}\n    - {}", up.replace(":a.md", ":b.md"));
    let text = fs::read_to_string(&md)
        .unwrap()
        .replace(&format!(" {up}"), &both);
    fs::write(&md, text).unwrap();
    let out = ns(&w)
        .args(["upstream", "diff", "a", "--human"])
        .assert()
        .success()
        .get_output()
        .clone();
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("=== acme/skills@"), "{text}");
    assert!(text.contains("+new a"), "{text}");
    assert!(
        text.ends_with(&format!(":b.md -> {head}\n(no changes)\n")),
        "{text}"
    );
}

#[test]
fn diff_of_an_unknown_skill_exits_2_and_names_the_status_command() {
    let w = world();
    let out = ns(&w)
        .args(["upstream", "diff", "ns-nope"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("ns upstream status"), "{err}");
}

#[test]
fn status_outside_a_git_repo_exits_2_with_a_hint() {
    let w = world();
    let out = ns(&w)
        .current_dir(&w.root)
        .args(["upstream", "status", "--skills-dir", "repo/skills"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("is not inside a git repository"), "{err}");
    assert!(
        err.contains("run from inside the nightshift checkout"),
        "{err}"
    );
    assert!(!err.contains("--repo"), "{err}");
}

#[test]
fn skills_dir_selects_another_skills_directory() {
    let w = world();
    let other = w.root.join("other");
    fs::create_dir_all(&other).unwrap();
    fs::rename(w.root.join("repo/skills/ns-a"), other.join("ns-a")).unwrap();
    let out = ns(&w)
        .args(["upstream", "status", "--skills-dir"])
        .arg(&other)
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["skills"][0]["skill"], "ns-a");
    let out = ns(&w)
        .args(["upstream", "status"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["skills"].as_array().unwrap().len(), 0);
}

#[test]
fn a_missing_skills_dir_exits_2_and_names_the_flag() {
    let w = world();
    for args in [&["status"][..], &["diff", "ns-a"][..]] {
        let out = ns(&w)
            .arg("upstream")
            .args(args)
            .args(["--skills-dir", "nowhere"])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let err = String::from_utf8(out.stderr).unwrap();
        assert!(err.contains("does not exist"), "{err}");
        assert!(err.contains("--skills-dir"), "{err}");
    }
}

#[test]
fn diff_exits_1_when_the_upstream_cannot_be_fetched() {
    let w = world();
    let skill = w.root.join("repo/skills/ns-gone");
    fs::create_dir_all(&skill).unwrap();
    fs::write(
        skill.join("SKILL.md"),
        "---\nname: ns-gone\ndescription: x\nmetadata:\n  upstream: acme/gone@abcdef1:a.md\n---\n",
    )
    .unwrap();
    let out = ns(&w)
        .args(["upstream", "diff", "ns-gone"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(err.contains("git fetch"), "{err}");
}
