#[test]
fn same_sha_compares_by_prefix_and_rejects_empty() {
    assert!(super::same_sha("abc1234", "abc1234def"));
    assert!(!super::same_sha("", "abc"));
    assert!(!super::same_sha("abc", ""));
    assert!(!super::same_sha("abc1", "abd1"));
}

use super::*;
use crate::testutil::{commit_file, g, rebased_unit, write_commit};
use std::fs;

#[test]
fn a_relative_system_temp_dir_falls_back_to_tmp() {
    assert_eq!(absolute_temp(PathBuf::from("rel/tmp")), Path::new("/tmp"));
    assert_eq!(absolute_temp(PathBuf::from("/var/t")), Path::new("/var/t"));
}

#[test]
fn same_named_checkouts_get_different_unit_tmp_parents() {
    let t = tempfile::tempdir().unwrap();
    let mk = |parent: &str| {
        let dir = t.path().join(parent).join("app");
        fs::create_dir_all(&dir).unwrap();
        g(&dir, &["init", "-q", "-b", "main"]);
        Repo::discover(&dir).unwrap()
    };
    let (a, b) = (mk("a"), mk("b"));
    assert_eq!(a.name(), b.name());
    let (ta, tb) = (a.unit_tmp("7-x"), b.unit_tmp("7-x"));
    assert_ne!(ta, tb);
    assert_eq!(ta, a.unit_tmp("7-x"));
    fs::create_dir_all(ta.join("build-1")).unwrap();
    fs::create_dir_all(tb.join("build-1")).unwrap();
    fs::remove_dir_all(&ta).unwrap();
    assert!(tb.join("build-1").is_dir());
    fs::remove_dir_all(&tb).unwrap();
}

fn same_diff(cwd: &Path, base: &str, a: &str, b: &str) -> bool {
    diff_id(cwd, base, a).is_some_and(|x| diff_id(cwd, base, b) == Some(x))
}

#[test]
fn run_refuses_each_network_subcommand_and_names_run_net() {
    let dir = tempfile::tempdir().unwrap();
    g(dir.path(), &["init", "-q"]);
    for args in [
        &["fetch", "-q", "origin"][..],
        &["push", "origin", "main"],
        &["pull"],
        &["clone", "-q", "x", "y"],
        &["ls-remote", "origin"],
        &["remote", "update"],
        &["remote", "-v", "update", "origin"],
        &["-c", "k.v=1", "fetch"],
        &["--no-pager", "-C", ".", "push"],
        &["--git-dir", ".git", "ls-remote"],
        &["--work-tree", ".", "pull"],
        &["--namespace", "n", "clone"],
        &["--config-env", "k.v=V", "fetch"],
        &["--attr-source", "HEAD", "fetch"],
    ] {
        reset_spawns();
        let err = run(dir.path(), args).unwrap_err();
        assert!(
            format!("{err:#}").contains("git::run_net"),
            "{args:?}: {err:#}"
        );
        assert_eq!(spawns(), 0, "{args:?} spawned git");
    }
}

#[test]
fn run_still_runs_local_remote_subcommands() {
    let dir = tempfile::tempdir().unwrap();
    g(dir.path(), &["init", "-q"]);
    run(
        dir.path(),
        &["remote", "add", "origin", "https://example.com/r.git"],
    )
    .unwrap();
    assert_eq!(
        run(dir.path(), &["remote", "get-url", "origin"]).unwrap(),
        "https://example.com/r.git"
    );
    assert_eq!(
        run(dir.path(), &["remote", "-v"]).unwrap().lines().count(),
        2
    );
    assert_eq!(run(dir.path(), &["remote"]).unwrap(), "origin");
    assert!(run(dir.path(), &["--version"])
        .unwrap()
        .starts_with("git version"));
    assert!(run(dir.path(), &["-c", "fetch.prune=true", "status"]).is_ok());
    assert!(run(dir.path(), &["config", "remote.origin.push", "x"]).is_ok());
    for word in ["fetch", "push"] {
        let sub = dir.path().join(word);
        fs::create_dir(&sub).unwrap();
        g(&sub, &["init", "-q"]);
        assert!(run(dir.path(), &["-C", word, "rev-parse", "--git-dir"]).is_ok());
    }
    run(
        dir.path(),
        &["remote", "add", "update", "https://example.com/u.git"],
    )
    .unwrap();
}

#[test]
fn run_net_returns_trimmed_stdout_and_fails_with_stderr() {
    let t = tempfile::tempdir().unwrap();
    let (origin, wt) = (t.path().join("origin"), t.path().join("wt"));
    fs::create_dir_all(&origin).unwrap();
    g(&origin, &["init", "-q", "-b", "main"]);
    let head = commit_file(&origin, "f", "x\n");
    run_net(t.path(), &["clone", "-q", "origin", "wt"]).unwrap();
    reset_spawns();
    let out = run_net(&wt, &["ls-remote", "origin", "refs/heads/main"]).unwrap();
    assert_eq!(out, format!("{head}\trefs/heads/main"));
    assert!(spawns() >= 1, "the ls-remote spawned git");
    fs::remove_dir_all(&origin).unwrap();
    let err = run_net(&wt, &["fetch", "-q", "origin"]).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.starts_with("git fetch -q origin failed: "), "{msg}");
    assert!(msg.contains("origin"), "git's stderr is in {msg}");
    assert!(
        msg.to_lowercase().contains("fatal"),
        "git's stderr is in {msg}"
    );
}

#[test]
fn a_network_command_runs_in_the_repo_it_names() {
    let dir = tempfile::tempdir().unwrap();
    g(dir.path(), &["init", "-q"]);
    let out = net(dir.path(), &["rev-parse", "--show-toplevel"])
        .output()
        .unwrap();
    let want = dir.path().canonicalize().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim_end(),
        want.to_str().unwrap()
    );
}

#[test]
fn parses_porcelain() {
    let text = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /w/x\nHEAD def\ndetached\n\nworktree /b\nbare\n";
    let e = parse_worktree_list(text);
    assert_eq!(e.len(), 3);
    assert_eq!(e[0].branch.as_deref(), Some("refs/heads/main"));
    assert_eq!(e[1].branch, None);
    assert!(e[2].bare);
}

fn listed(cwd: &Path) -> Vec<WorktreeEntry> {
    parse_worktree_list(&run(cwd, &["worktree", "list", "--porcelain"]).unwrap())
}

#[test]
fn discover_ignores_a_worktree_another_git_is_still_writing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let main = dir.join("main");
    fs::create_dir(&main).unwrap();
    g(&main, &["init", "-q", "-b", "main"]);
    commit_file(&main, "README", "hi\n");
    // What `git worktree add` leaves before it writes `commondir`.
    let half = main.join(".git/worktrees/half");
    fs::create_dir_all(&half).unwrap();
    fs::write(
        half.join("gitdir"),
        dir.join("half/.git").to_string_lossy().as_bytes(),
    )
    .unwrap();
    fs::write(half.join("commondir"), "").unwrap();
    let repo = Repo::discover(&main).unwrap();
    assert_eq!(repo.root, main);
    assert_eq!(repo.common_dir, main.join(".git"));
}

#[test]
fn discover_finds_the_main_worktree_git_lists_first() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let main = dir.join("main");
    fs::create_dir(&main).unwrap();
    g(&main, &["init", "-q", "-b", "main"]);
    commit_file(&main, "README", "hi\n");
    fs::create_dir(main.join("sub")).unwrap();
    g(&main, &["worktree", "add", "-q", "../linked"]);
    std::os::unix::fs::symlink(&main, dir.join("link")).unwrap();
    let sep = dir.join("sepwt");
    g(
        &dir,
        &["init", "-q", "--separate-git-dir", "sep.git", "sepwt"],
    );
    commit_file(&sep, "README", "hi\n");
    g(
        &main,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            main.to_str().unwrap(),
            "sm",
        ],
    );
    for start in [
        main.clone(),
        main.join("sub"),
        main.join(".git"),
        dir.join("linked"),
        dir.join("link"),
        sep,
        main.join("sm"),
    ] {
        let repo = Repo::discover(&start).unwrap();
        let listed = &listed(&start)[0];
        assert_eq!(repo.root, listed.path, "from {}", start.display());
        assert_eq!(
            repo.common_dir.to_string_lossy(),
            run(
                &start,
                &["rev-parse", "--path-format=absolute", "--git-common-dir"]
            )
            .unwrap(),
        );
    }
}

#[test]
fn discover_refuses_a_bare_repo_and_its_worktrees() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    g(&dir, &["init", "-q", "-b", "main", "src"]);
    commit_file(&dir.join("src"), "README", "hi\n");
    g(&dir, &["clone", "-q", "--bare", "src", "bare.git"]);
    let bare = dir.join("bare.git");
    g(&bare, &["worktree", "add", "-q", "../bwt"]);
    g(&dir, &["clone", "-q", "--bare", "src", "unset.git"]);
    let unset = dir.join("unset.git");
    g(&unset, &["config", "--unset", "core.bare"]);
    for start in [bare, dir.join("bwt"), unset] {
        assert!(listed(&start)[0].bare, "{}", start.display());
        let err = Repo::discover(&start).err().unwrap().to_string();
        assert!(
            err.contains("bare repositories"),
            "{}: {err}",
            start.display()
        );
    }
}

#[test]
fn a_rebase_keeps_the_diff_id_and_a_content_change_does_not() {
    let r = rebased_unit();
    let d = &r.dir;
    assert_ne!(r.reviewed, r.rebased);
    assert!(same_diff(d, "main", &r.reviewed, &r.rebased));
    assert!(!same_diff(d, "main", &r.reviewed, "0000000"));
    let changed = commit_file(d, "work.txt", "two\n");
    assert!(!same_diff(d, "main", &r.reviewed, &changed));
    assert!(!same_diff(d, "main", "0000000", &r.rebased));
    assert!(!same_diff(d, "main", "", &r.rebased));
}

#[test]
fn a_whitespace_only_change_is_the_same_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().canonicalize().unwrap();
    g(d, &["init", "-q", "-b", "main"]);
    commit_file(d, "README", "hi\n");
    g(d, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(d, "work.txt", "a b\n");
    let spaced = commit_file(d, "work.txt", "a   b\n");
    assert!(same_diff(d, "main", &reviewed, &spaced));
}

#[test]
fn a_stale_local_base_still_sees_a_unit_rebased_onto_the_remote_branch() {
    let r = rebased_unit();
    let d = &r.dir;
    let ahead = g(d, &["rev-parse", "main"]);
    g(d, &["update-ref", "refs/remotes/origin/main", &ahead]);
    g(d, &["update-ref", "refs/heads/main", &format!("{ahead}~1")]);
    assert_eq!(freshest(d, "main"), "refs/remotes/origin/main");
    assert!(same_diff(d, "main", &r.reviewed, &r.rebased));
    let changed = commit_file(d, "work.txt", "two\n");
    assert!(!same_diff(d, "main", &r.reviewed, &changed));
}

/// A unit reviewed at `reviewed` that then merged a newer `main`, at `merged`, with
/// local `main` and `origin/main` left at the old tip, as after `git pull origin main`.
fn merged_behind_a_stale_base() -> (tempfile::TempDir, PathBuf, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path().canonicalize().unwrap();
    g(&d, &["init", "-q", "-b", "main"]);
    let old = commit_file(&d, "README", "hi\n");
    g(&d, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(&d, "work.txt", "one\n");
    g(&d, &["checkout", "-q", "main"]);
    let upstream = commit_file(&d, "UPSTREAM", "x\n");
    g(&d, &["checkout", "-q", "unit"]);
    g(&d, &["merge", "-q", "--no-ff", "--no-edit", "main"]);
    let merged = g(&d, &["rev-parse", "HEAD"]);
    g(&d, &["update-ref", "refs/heads/main", &old]);
    g(&d, &["update-ref", "refs/remotes/origin/main", &upstream]);
    (tmp, d, reviewed, merged)
}

#[test]
fn a_merge_of_a_newer_base_keeps_the_diff_id_though_the_base_ref_is_behind() {
    let (_tmp, d, reviewed, merged) = merged_behind_a_stale_base();
    assert!(same_diff(&d, "main", &reviewed, &merged));
    g(
        &d,
        &["checkout", "-q", "--detach", "refs/remotes/origin/main"],
    );
    let later = commit_file(&d, "LATER", "y\n");
    g(&d, &["update-ref", "refs/remotes/origin/main", &later]);
    g(&d, &["checkout", "-q", "unit"]);
    assert!(same_diff(&d, "main", &reviewed, &merged));
    g(&d, &["checkout", "-q", "unit"]);
    let changed = commit_file(&d, "work.txt", "two\n");
    assert!(!same_diff(&d, "main", &reviewed, &changed));
}

#[test]
fn a_merge_of_a_side_branch_is_not_the_same_diff() {
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().canonicalize().unwrap();
    g(d, &["init", "-q", "-b", "main"]);
    commit_file(d, "README", "hi\n");
    g(d, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(d, "work.txt", "one\n");
    g(d, &["checkout", "-qb", "side", "main"]);
    commit_file(d, "side.txt", "unreviewed\n");
    g(d, &["checkout", "-q", "unit"]);
    g(d, &["merge", "-q", "--no-ff", "--no-edit", "side"]);
    let merged = g(d, &["rev-parse", "HEAD"]);
    assert!(!same_diff(d, "main", &reviewed, &merged));
}

#[test]
fn exact_diff_id_counts_context_and_merged_in_work() {
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().canonicalize().unwrap();
    g(d, &["init", "-q", "-b", "main"]);
    commit_file(d, "f.txt", "a\nb\nc\nd\ne\nf\ng\n");
    g(d, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(d, "f.txt", "a\nb\nc\nd\nE\nf\ng\n");
    g(d, &["checkout", "-q", "main"]);
    commit_file(d, "f.txt", "a\nb\nC\nd\ne\nf\ng\n");
    g(d, &["checkout", "-q", "unit"]);
    g(d, &["merge", "-q", "--no-edit", "main"]);
    let merged = g(d, &["rev-parse", "HEAD"]);
    assert_ne!(
        exact_diff_id(d, "main", &reviewed),
        exact_diff_id(d, "main", &merged)
    );
}

#[test]
fn a_merge_that_resolves_a_conflict_differently_is_not_the_same_diff() {
    let r = rebased_unit();
    let d = &r.dir;
    g(d, &["checkout", "-q", "main"]);
    commit_file(d, "work.txt", "main's\n");
    g(d, &["checkout", "-q", "unit"]);
    assert!(!ok(d, &["merge", "-q", "--no-edit", "main"]));
    fs::write(d.join("work.txt"), "resolved\n").unwrap();
    g(d, &["add", "work.txt"]);
    g(d, &["commit", "-q", "-m", "resolve"]);
    let merged = g(d, &["rev-parse", "HEAD"]);
    assert!(!same_diff(d, "main", &r.rebased, &merged));
}

#[test]
fn a_base_change_next_to_the_units_lines_keeps_the_diff_id() {
    let tmp = tempfile::tempdir().unwrap();
    let d = &tmp.path().canonicalize().unwrap();
    g(d, &["init", "-q", "-b", "main"]);
    commit_file(d, "f.txt", "a\nb\nc\nd\ne\nf\ng\n");
    g(d, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(d, "f.txt", "a\nb\nc\nd\nE\nf\ng\n");
    g(d, &["checkout", "-q", "main"]);
    commit_file(d, "f.txt", "a\nb\nC\nd\ne\nf\ng\n");
    g(d, &["checkout", "-q", "unit"]);
    g(d, &["merge", "-q", "--no-edit", "main"]);
    let merged = g(d, &["rev-parse", "HEAD"]);
    assert!(same_diff(d, "main", &reviewed, &merged));
    g(d, &["reset", "-q", "--hard", &reviewed]);
    g(d, &["rebase", "-q", "main"]);
    let rebased = g(d, &["rev-parse", "HEAD"]);
    assert!(same_diff(d, "main", &reviewed, &rebased));
}

#[test]
fn an_empty_diff_is_never_the_same_diff() {
    let r = rebased_unit();
    assert!(!same_diff(&r.dir, "main", "main", "main"));
    assert!(same_diff(&r.dir, "main", &r.rebased, &r.rebased));
}

#[test]
fn option_looking_shas_and_bases_have_no_diff_id() {
    let r = rebased_unit();
    assert_eq!(diff_id(&r.dir, "main", "--help"), None);
    assert_eq!(diff_id(&r.dir, "--all", &r.rebased), None);
    assert_eq!(diff_id(&r.dir, "main", "-p"), None);
}

#[test]
fn a_binary_change_after_review_is_not_the_same_diff_but_a_rebase_is() {
    let tmp = tempfile::tempdir().unwrap();
    let d = tmp.path();
    g(d, &["init", "-q", "-b", "main"]);
    commit_file(d, "README", "hi\n");
    g(d, &["checkout", "-qb", "unit"]);
    write_commit(d, "blob.bin", &[0, 1, 2, 0, 3]);
    let reviewed = g(d, &["rev-parse", "HEAD"]);
    write_commit(d, "blob.bin", &[0, 9, 9, 0, 9]);
    let changed = g(d, &["rev-parse", "HEAD"]);
    g(d, &["reset", "-q", "--hard", &reviewed]);
    g(d, &["checkout", "-q", "main"]);
    commit_file(d, "UPSTREAM", "x\n");
    g(d, &["checkout", "-q", "unit"]);
    g(d, &["rebase", "-q", "main"]);
    let rebased = g(d, &["rev-parse", "HEAD"]);
    assert!(same_diff(d, "main", &reviewed, &rebased));
    assert!(!same_diff(d, "main", &changed, &reviewed));
}

#[test]
fn the_default_base_is_never_the_literal_head() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    g(&dir, &["init", "-q", "-b", "dev"]);
    assert!(Repo::discover(&dir).unwrap().default_base().is_err());
    let sha = commit_file(&dir, "README", "hi\n");
    let repo = Repo::discover(&dir).unwrap();
    assert_eq!(repo.default_base().unwrap(), "dev");
    g(&dir, &["update-ref", "refs/remotes/origin/trunk", "HEAD"]);
    g(
        &dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/trunk",
        ],
    );
    assert_eq!(repo.default_base().unwrap(), "origin/trunk");
    g(
        &dir,
        &["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"],
    );
    g(&dir, &["checkout", "-q", "--detach"]);
    assert_eq!(repo.default_base().unwrap(), sha);
}

#[test]
fn interdiff_counts_only_lines_the_unit_itself_changed() {
    let r = rebased_unit();
    assert_eq!(
        interdiff_lines(&r.dir, "main", &r.reviewed, &r.rebased),
        Some(0),
        "a clean rebase changes none of the unit's lines"
    );
    g(&r.dir, &["checkout", "-q", "unit"]);
    let edited = commit_file(&r.dir, "work.txt", "one\ntwo\n");
    assert_eq!(
        interdiff_lines(&r.dir, "main", &r.rebased, &edited),
        Some(1),
        "one added line"
    );
    let swapped = commit_file(&r.dir, "work.txt", "uno\ntwo\n");
    assert_eq!(
        interdiff_lines(&r.dir, "main", &r.rebased, &swapped),
        Some(3),
        "one line removed and two added"
    );
    assert_eq!(interdiff_lines(&r.dir, "main", "nope", &swapped), None);
}

#[test]
fn took_in_base_sees_a_rebase_and_not_an_own_commit() {
    let r = rebased_unit();
    assert!(took_in_base(&r.dir, "main", &r.reviewed, &r.rebased));
    g(&r.dir, &["checkout", "-q", "unit"]);
    let own = commit_file(&r.dir, "work.txt", "one\ntwo\n");
    assert!(!took_in_base(&r.dir, "main", &r.rebased, &own));
}

#[test]
fn took_in_base_sees_a_merge_of_the_base() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    g(&dir, &["init", "-q", "-b", "main"]);
    commit_file(&dir, "README", "hi\n");
    g(&dir, &["checkout", "-qb", "unit"]);
    let reviewed = commit_file(&dir, "work.txt", "one\n");
    g(&dir, &["checkout", "-q", "main"]);
    commit_file(&dir, "UPSTREAM", "x\n");
    g(&dir, &["checkout", "-q", "unit"]);
    g(
        &dir,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "merge",
            "-q",
            "--no-edit",
            "main",
        ],
    );
    let merged = g(&dir, &["rev-parse", "HEAD"]);
    assert!(took_in_base(&dir, "main", &reviewed, &merged));
}
