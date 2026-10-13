//! The pass cache: a full run that passed is recorded by tree, and a later full run on that tree
//! skips.

use std::fs;
use std::path::Path;
use std::process::Output;

use super::{all_steps, common, git, stderr, steps, write_exe, Repo, FAST_STEPS, NS};

fn commit(dir: &Path, file: &str, text: &str) {
    let path = dir.join(file);
    let mut body = fs::read_to_string(&path).unwrap_or_default();
    body.push_str(text);
    fs::write(&path, body).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// The line a run that skips on a cached pass prints, up to the `where` it names.
fn already_passed(dir: &Path) -> String {
    let tree = git(dir, &["write-tree"]);
    format!("ci-local: tree {tree} already passed (")
}

fn ran_everything(out: &Output) {
    assert!(out.status.success(), "{}", stderr(out));
    assert_eq!(steps(out), all_steps());
    assert!(!stderr(out).contains("already passed"), "{}", stderr(out));
}

fn skipped(out: &Output, dir: &Path) {
    assert!(out.status.success(), "{}", stderr(out));
    assert!(steps(out).is_empty(), "{}", stderr(out));
    assert!(
        stderr(out).contains(&already_passed(dir)),
        "{}",
        stderr(out)
    );
}

#[test]
fn a_second_full_run_on_the_same_tree_skips_naming_when_and_where_it_passed() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));
    assert_eq!(repo.cached().len(), 1);

    let out = repo.run(&[], &[]);
    skipped(&out, root);
    let head = git(root, &["rev-parse", "--short", "HEAD"]);
    let err = stderr(&out);
    let line = err.lines().find(|l| l.contains("already passed")).unwrap();
    let rest = line.strip_prefix(&already_passed(root)).unwrap();
    let (when, place) = rest.strip_suffix(')').unwrap().split_once(", ").unwrap();
    assert_eq!(when.len(), "2026-10-12T00:00:00Z".len(), "{line}");
    assert!(when.ends_with('Z'), "{line}");
    let root = root.canonicalize().unwrap();
    assert_eq!(place, format!("{} at {head}", root.display()));
    assert_eq!(repo.cached().len(), 1);
}

#[test]
fn a_changed_file_tool_pin_or_ci_local_sh_runs_again() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));
    for (file, text) in [
        ("README", "changed\n"),
        ("scripts/ci-tools.d/ruff.toml", "\n# changed\n"),
        ("scripts/ci-local.sh", "\n# changed\n"),
    ] {
        commit(root, file, text);
        ran_everything(&repo.run(&[], &[]));
        skipped(&repo.run(&[], &[]), root);
    }
    assert_eq!(repo.cached().len(), 4);
}

#[test]
fn a_failure_is_never_cached() {
    let repo = Repo::new();
    repo.commit_all();
    let out = repo.run(&[], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    assert!(repo.cached().is_empty());
    let out = repo.run(&[], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    ran_everything(&repo.run(&[], &[]));
}

#[test]
fn no_cache_runs_on_a_tree_that_passed() {
    let repo = Repo::new();
    repo.commit_all();
    ran_everything(&repo.run(&[], &[]));
    ran_everything(&repo.run(&["--no-cache"], &[]));
    let out = repo.run(&["--no-cache"], &[("FAIL_ON", "ns ratchet")]);
    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    for args in [&["--fast", "--no-cache"][..], &["--no-cache", "--fast"]] {
        let out = repo.run(args, &[]);
        assert!(out.status.success(), "{}", stderr(&out));
        assert_eq!(steps(&out), FAST_STEPS);
    }
    let out = repo.run(&["--no-cache", "--no-cache"], &[]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn fast_neither_uses_nor_fills_the_cache() {
    let repo = Repo::new();
    repo.commit_all();
    let out = repo.run(&["--fast"], &[]);
    assert_eq!(steps(&out), FAST_STEPS);
    assert!(repo.cached().is_empty());
    ran_everything(&repo.run(&[], &[]));
    let out = repo.run(&["--fast"], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(steps(&out), FAST_STEPS);
}

#[test]
fn a_dirty_working_tree_never_uses_or_fills_the_cache() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));

    fs::write(root.join("README"), "unstaged\n").unwrap();
    let out = repo.run(&[], &[]);
    ran_everything(&out);
    assert!(stderr(&out).contains("ci-local: the working tree differs from the index"));
    assert_eq!(repo.cached().len(), 1);
    git(root, &["checkout", "README"]);

    fs::write(root.join("scratch"), "untracked\n").unwrap();
    let out = repo.run(&[], &[]);
    ran_everything(&out);
    assert!(stderr(&out).contains("ci-local: the working tree has untracked files"));
    assert_eq!(repo.cached().len(), 1);
    fs::remove_file(root.join("scratch")).unwrap();

    skipped(&repo.run(&[], &[]), root);
}

#[test]
fn a_run_that_changes_the_tree_it_tests_is_not_recorded() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    write_exe(
        &root.join("scripts/check-private.sh"),
        "#!/bin/sh\necho edited >> README\n",
    );
    git(root, &["commit", "-qam", "edits"]);
    let out = repo.run(&[], &[]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("ci-local: the tree changed during the run"));
    assert!(repo.cached().is_empty());
}

/// The pre-push hook records the pushed commit's tree; the gate in another worktree, and the
/// merge queue's gate on a merge result staged in a scratch worktree, skip on it.
#[test]
fn pre_push_the_gate_and_the_merge_queue_share_the_cache() {
    let repo = Repo::new();
    let root = repo.dir.path();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.githooks/pre-push");
    write_exe(
        &root.join(".githooks/pre-push"),
        fs::read_to_string(src).unwrap(),
    );
    repo.commit_all();
    write_exe(&root.join(".git/pre-push-target/debug/ns"), NS);
    git(root, &["checkout", "-q", "-b", "pr"]);
    commit(root, "pr.txt", "pr\n");
    let pr = git(root, &["rev-parse", "HEAD"]);
    git(root, &["checkout", "-q", "main"]);
    commit(root, "main.txt", "main\n");
    git(root, &["checkout", "-q", "pr"]);
    git(root, &["merge", "-q", "--no-edit", "main"]);
    let merged = git(root, &["rev-parse", "HEAD"]);

    let mut hook = common::command("sh");
    hook.arg(root.join(".githooks/pre-push"))
        .current_dir(root)
        .env("GIT_DIR", root.join(".git"))
        .env_remove("GIT_WORK_TREE")
        .write_stdin(format!(
            "refs/heads/pr {merged} refs/heads/pr {}\n",
            "0".repeat(40)
        ));
    repo.stub_env(&mut hook, &[]);
    let out = hook.output();
    ran_everything(&out);
    assert_eq!(repo.cached().len(), 1);

    let worktrees = tempfile::tempdir().unwrap();
    let gate = worktrees.path().join("gate");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            gate.to_str().unwrap(),
            &merged,
        ],
    );
    skipped(&repo.run_in(&gate, &[], &[]), &gate);

    let scratch = worktrees.path().join("merge-queue");
    git(
        root,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            scratch.to_str().unwrap(),
            &pr,
        ],
    );
    git(&scratch, &["merge", "-q", "--no-commit", "--no-ff", "main"]);
    skipped(&repo.run_in(&scratch, &[], &[]), &scratch);
    assert_eq!(repo.cached().len(), 1);
}

#[test]
fn the_cache_keeps_the_entries_used_last() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    let max = [("CI_LOCAL_CACHE_MAX", "2")];
    ran_everything(&repo.run(&[], &max));
    let first = git(root, &["rev-parse", "HEAD"]);
    commit(root, "README", "2\n");
    let second = git(root, &["rev-parse", "HEAD"]);
    ran_everything(&repo.run(&[], &max));
    git(root, &["checkout", "-q", &first]);
    skipped(&repo.run(&[], &max), root);
    git(root, &["checkout", "-q", "main"]);
    commit(root, "README", "3\n");
    ran_everything(&repo.run(&[], &max));
    assert_eq!(repo.cached().len(), 2);
    git(root, &["checkout", "-q", &first]);
    skipped(&repo.run(&[], &max), root);
    git(root, &["checkout", "-q", &second]);
    ran_everything(&repo.run(&[], &max));
}

/// The times of the pass cache's entries, by name.
fn cache_times(repo: &Repo) -> Vec<(String, std::time::SystemTime)> {
    let dir = repo.dir.path().join(".git/ns/ci-passed");
    repo.cached()
        .into_iter()
        .map(|e| {
            let time = fs::metadata(dir.join(&e)).unwrap().modified().unwrap();
            (e, time)
        })
        .collect()
}

/// Asserts the entry for the checked-out tree is the newest in the cache, tied with none.
fn newest_is_checked_out(repo: &Repo, out: &Output) {
    let times = cache_times(repo);
    let (newest, time) = times.iter().max_by_key(|(_, t)| *t).unwrap();
    let ties = times.iter().filter(|(_, t)| t == time).count();
    assert_eq!(ties, 1, "{times:?}\n{}", stderr(out));
    let body = fs::read_to_string(repo.dir.path().join(".git/ns/ci-passed").join(newest)).unwrap();
    let tree = git(repo.dir.path(), &["write-tree"]);
    assert!(body.starts_with(&format!("tree {tree}\n")), "{body}");
}

/// A `date` that runs `seconds` for `date +%s` and the real `date` otherwise, and the `PATH`
/// that puts it ahead of `repo`'s stubs. The directory lives as long as the returned guard.
fn clock(repo: &Repo, seconds: &str) -> (tempfile::TempDir, String) {
    let real = ["/usr/bin/date", "/bin/date"]
        .into_iter()
        .find(|p| Path::new(p).is_file())
        .expect("date is required to run ci-local");
    let dir = tempfile::tempdir().unwrap();
    write_exe(
        &dir.path().join("date"),
        format!("#!/bin/sh\n[ \"$1\" != +%s ] || {seconds}\nexec {real} \"$@\"\n"),
    );
    let path = format!(
        "{}:{}:/usr/bin:/bin",
        dir.path().display(),
        repo.dir.path().join("bin").display()
    );
    (dir, path)
}

/// Entries whose times tie with each other and with the clock, as after a coarse timestamp
/// tick or a clock that stepped back, still go in the order they were used.
#[test]
fn the_cache_keeps_the_entries_used_last_when_their_times_tie_with_the_clock() {
    let repo = Repo::new();
    repo.commit_all();
    let root = repo.dir.path();
    ran_everything(&repo.run(&[], &[]));
    let first = git(root, &["rev-parse", "HEAD"]);
    commit(root, "README", "2\n");
    let second = git(root, &["rev-parse", "HEAD"]);
    ran_everything(&repo.run(&[], &[]));

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600;
    let tie = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now);
    for (entry, _) in cache_times(&repo) {
        fs::File::options()
            .write(true)
            .open(root.join(".git/ns/ci-passed").join(entry))
            .unwrap()
            .set_modified(tie)
            .unwrap();
    }
    let (_clock, path) = clock(&repo, &format!("exec echo {now}"));
    let max = [("CI_LOCAL_CACHE_MAX", "2"), ("PATH", &path)];

    git(root, &["checkout", "-q", &first]);
    let out = repo.run(&[], &max);
    skipped(&out, root);
    newest_is_checked_out(&repo, &out);
    git(root, &["checkout", "-q", "main"]);
    commit(root, "README", "3\n");
    let out = repo.run(&[], &max);
    ran_everything(&out);
    newest_is_checked_out(&repo, &out);
    assert_eq!(repo.cached().len(), 2);
    git(root, &["checkout", "-q", &first]);
    skipped(&repo.run(&[], &max), root);
    git(root, &["checkout", "-q", &second]);
    ran_everything(&repo.run(&[], &max));
}
