//! The GitLab adapter: `ns run`'s guards after each phase and `ns clean` on a GitLab project,
//! read through `glab` with no `gh` to fall back on.

#![allow(
    clippy::disallowed_methods,
    reason = "an integration test is a crate of its own and cannot use crate::proc"
)]

#[path = "common/mod.rs"]
mod common;
#[path = "common/factory_env.rs"]
mod factory_env;

use std::fs;

use common::write_exe;
use factory_env::*;
use serde_json::Value;

/// An env whose forge is GitLab, holding issue 7, with a `gh` that records each call and fails.
fn gl_guard_env() -> Env {
    let e = Env::new();
    e.factory("[queue]\nsource = \"gitlab\"\n");
    e.gh_file(
        "gl-issue-7.raw",
        r#"{"iid":7,"title":"Fix the thing","state":"opened","web_url":"https://gitlab.example/o/r/-/issues/7"}"#,
    );
    write_exe(
        &e.bin.join("gh"),
        "#!/bin/sh\necho \"$*\" >> \"$FAKE_GH_DIR/gh-on-gitlab\"\nexit 1\n",
    );
    e
}

fn assert_no_gh(e: &Env) {
    let calls = fs::read_to_string(e.ghd.join("gh-on-gitlab")).unwrap_or_default();
    assert!(calls.is_empty(), "gh ran on a GitLab project: {calls}");
}

fn gl_run(e: &Env, code: i32) -> Value {
    e.run(&["run", "--issue", "7"], code)
}

#[test]
fn gitlab_run_passes_the_guard_after_ship_reading_the_mr() {
    let e = gl_guard_env();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let v = gl_run(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    assert_eq!(v["unit"], UNIT, "{v}");
    assert!(e
        .prompt(1, "triage")
        .contains("https://gitlab.example/o/r/-/issues/7"));
    assert!(
        e.glab_calls()
            .contains("api projects/:id/merge_requests/12"),
        "{}",
        e.glab_calls()
    );
    assert_no_gh(&e);
}

#[test]
fn gitlab_run_stops_on_an_mr_merged_by_ship_but_not_on_a_closed_one() {
    let e = gl_guard_env();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.queue("ship", &["pass:merge"]);
    let v = gl_run(&e, 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("PR merged by run: #12"), "{v}");
    assert_no_gh(&e);

    let e = gl_guard_env();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    e.gh_file("pr-12.state", "CLOSED");
    let v = gl_run(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
}

#[test]
fn gitlab_run_names_glab_when_it_cannot_read_the_mr() {
    let e = gl_guard_env();
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    // Every glab call fails once ship has written pr.md.
    e.ctl("ship.sh", "touch \"$FAKE_GH_DIR/glab.fail\"\n");
    e.queue("ship", &["pass:script"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(
        err.contains("glab api projects/:id/merge_requests/12"),
        "{err}"
    );
    assert_no_gh(&e);
}

const GL_MERGED_B: &str = r#"[{"iid":3,"state":"merged","source_branch":"ns/3-fix-b"}]"#;

#[test]
fn gitlab_another_units_merge_then_a_rebase_is_not_this_units_push() {
    let e = gl_guard_env();
    another_actor_lands_linked(&e, "", SQUASH_B, "gl-mrs", GL_MERGED_B);
    let v = gl_run(&e, 0);
    assert_eq!(v["outcome"], "done", "{v}");
    let main = git(&e.base.join("remote.git"), &["rev-parse", "main"]);
    assert!(
        e.glab_calls().contains(&format!(
            "api --paginate projects/:id/repository/commits/{main}/merge_requests"
        )),
        "{}",
        e.glab_calls()
    );
    let log = run_log(&e);
    assert!(
        log.contains("\"event\":\"default_moved_by_other_actor\"")
            && log.contains("\"prs\":[3]")
            && !log.contains("\"breach\""),
        "{log}"
    );
    assert_no_gh(&e);
}

#[test]
fn gitlab_a_push_of_its_own_under_another_merge_is_still_stuck() {
    let e = gl_guard_env();
    another_actor_lands_linked(
        &e,
        "git push -q origin HEAD:main",
        SQUASH_B,
        "gl-mrs",
        GL_MERGED_B,
    );
    let v = gl_run(&e, 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.starts_with("default branch moved"), "{v}");
    assert!(
        reason.contains("is in no merged PR of another branch"),
        "{v}"
    );
    assert_no_gh(&e);
}

#[test]
fn gitlab_a_closed_mr_or_one_from_this_units_branch_is_no_other_actors_merge() {
    for mrs in [
        r#"[{"iid":3,"state":"closed","source_branch":"ns/3-fix-b"}]"#.to_string(),
        GL_MERGED_B.replace("ns/3-fix-b", &format!("ns/{UNIT}")),
    ] {
        let e = gl_guard_env();
        another_actor_lands_linked(&e, "", SQUASH_B, "gl-mrs", &mrs);
        let v = gl_run(&e, 1);
        let reason = v["reason"].as_str().unwrap();
        assert!(
            reason.contains("is in no merged PR of another branch"),
            "{mrs}: {v}"
        );
    }
}

#[test]
fn gitlab_a_merge_glab_cannot_attribute_is_stuck_naming_glab() {
    let e = gl_guard_env();
    another_actor_lands_linked(&e, "", SQUASH_B, "gl-mrs", GL_MERGED_B);
    e.gh_file("gl-commit-mrs.fail", "");
    let v = gl_run(&e, 1);
    let reason = v["reason"].as_str().unwrap();
    assert!(reason.contains("cannot tell who made"), "{v}");
    assert!(reason.contains("glab api"), "{v}");
    assert_no_gh(&e);
}

/// [`unit_with_pr`] on GitLab: issue `n` in `issue_state` (`opened` or `closed`).
fn gl_unit_with_pr(
    e: &Env,
    n: u64,
    unit: &str,
    pr: u64,
    issue_state: &str,
    pr_state: &str,
) -> std::path::PathBuf {
    let wt = unit_with_pr(e, n, unit, pr, "OPEN", pr_state);
    e.gh_file(&format!("gl-state-{n}"), issue_state);
    wt
}

#[test]
fn gitlab_clean_removes_a_merged_unit_and_keeps_open_and_closed_unmerged_ones() {
    let e = gl_guard_env();
    gl_unit_with_pr(&e, 3, "3-done", 30, "closed", "MERGED");
    gl_unit_with_pr(&e, 4, "4-open", 40, "opened", "MERGED");
    gl_unit_with_pr(&e, 5, "5-closed-mr", 50, "closed", "CLOSED");
    let v = e.run(&["clean"], 0);
    assert_eq!(v["status"], "cleaned", "{v}");
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    let r = reasons(&v);
    assert_eq!(r["4-open"], "issue #4 is open", "{v}");
    assert_eq!(r["5-closed-mr"], "PR #50 is CLOSED", "{v}");
    assert!(!e.worktree("3-done").exists());
    assert!(e.worktree("5-closed-mr").exists());
    assert!(
        e.glab_calls()
            .contains("api projects/:id/merge_requests/30"),
        "{}",
        e.glab_calls()
    );
    assert_no_gh(&e);
}

/// The merged MR's head was made on GitLab (a merge of main into the branch), so it isn't here
/// until clean fetches it from the MR's own ref.
#[test]
fn gitlab_clean_fetches_a_merged_head_missing_here_from_its_merge_request_ref() {
    let e = gl_guard_env();
    let wt = gl_unit_with_pr(&e, 3, "3-done", 30, "closed", "MERGED");
    git(&wt, &["commit", "-q", "--allow-empty", "-m", "merge main"]);
    let head = git(&wt, &["rev-parse", "HEAD"]);
    git(
        &wt,
        &["push", "-q", "origin", "HEAD:refs/merge-requests/30/head"],
    );
    git(&wt, &["reset", "-q", "--hard", "HEAD~1"]);
    git(&e.root, &["reflog", "expire", "--expire=now", "--all"]);
    git(&e.root, &["gc", "-q", "--prune=now"]);
    let commit = format!("{head}^{{commit}}");
    let here = std::process::Command::new("git")
        .args(["-C", wt.to_str().unwrap(), "cat-file", "-e", &commit])
        .env_remove("GIT_DIR")
        .status()
        .unwrap();
    assert!(!here.success(), "the MR head is still here");
    e.gh_file("pr-30.head", &head);
    let v = e.run(&["clean"], 0);
    assert_eq!(units_in(&v["removed"]), ["3-done"], "{v}");
    assert_no_gh(&e);
}

#[test]
fn gitlab_merge_step_stops_naming_its_issue_and_runs_no_gh() {
    let e = gl_guard_env();
    e.factory("[queue]\nsource = \"gitlab\"\n\n[merge]\npolicy = \"auto\"\n");
    e.ctl("pr", "12");
    e.queue("build", &["pass:commit"]);
    let out = e.ns().args(["run", "--issue", "7"]).output();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success(), "{err}");
    assert!(
        err.contains("ns run's merge step is not supported on GitLab yet (#323)"),
        "{err}"
    );
    assert!(!e.glab_calls().contains("merge_requests/12/merge"));
    assert_no_gh(&e);
}
