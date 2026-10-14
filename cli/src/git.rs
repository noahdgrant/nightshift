//! Thin wrapper around the `git` binary.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::error::SfError;

/// Variables through which a parent git process (a hook, `git rebase --exec`, `git bisect run`)
/// points child git commands at its own repository. `GIT_DIR` overrides even `git -C <dir>`, so
/// an inherited one makes a command meant for a scratch repo act on the real one.
pub const LOCATION_VARS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

/// Remove inherited git location variables, so the command acts only on the repo it names.
pub fn scrub(cmd: &mut Command) -> &mut Command {
    for v in LOCATION_VARS {
        cmd.env_remove(v);
    }
    cmd
}

#[cfg(test)]
thread_local! {
    static SPAWNS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub fn reset_spawns() {
    SPAWNS.with(|c| c.set(0));
}

#[cfg(test)]
pub fn spawns() -> usize {
    SPAWNS.with(std::cell::Cell::get)
}

/// A `git` command with inherited location variables removed.
pub fn command() -> Command {
    #[cfg(test)]
    SPAWNS.with(|c| c.set(c.get() + 1));
    let mut c = Command::new("git");
    scrub(&mut c);
    c
}

/// The SSH command `ns run`'s phases push with when the user chose none. A pre-push hook that
/// runs all of CI leaves the connection idle for minutes; without keepalives it can be dropped,
/// and the push dies with SIGPIPE after the checks passed (#451).
pub const SSH_KEEPALIVE: &str = "ssh -o ServerAliveInterval=30 -o ServerAliveCountMax=40";

/// Whether the user chose how git runs ssh in `cwd`: `GIT_SSH_COMMAND` or `GIT_SSH` in the
/// environment, or `core.sshCommand` in its config.
pub fn ssh_command_set(cwd: &Path) -> bool {
    let set = |k| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    set("GIT_SSH_COMMAND")
        || set("GIT_SSH")
        || run(cwd, &["config", "core.sshCommand"]).is_ok_and(|v| !v.is_empty())
}

/// Run git in `cwd` and return trimmed stdout. Fails with git's stderr on non-zero exit.
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = command()
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("failed to run git; is it installed and on PATH?")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(
            SfError::general(format!("git {} failed: {}", args.join(" "), stderr.trim())).into(),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// A git command for `root` that never prompts for credentials, for work `ns` does unattended.
pub fn unattended(root: &Path, args: &[&str]) -> Command {
    let mut c = command();
    c.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    c
}

/// A transfer that moves fewer bytes a second than this, for `LOW_SPEED_SECS`, fails.
const LOW_SPEED_BYTES: &str = "1000";
const LOW_SPEED_SECS: &str = "60";
/// ssh that never prompts and gives up on a dead connection: 30 s to connect, and about a
/// minute of a server that stops answering.
pub const SSH: &str =
    "ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4";

/// [`unattended`] for a command that talks to `origin`, which must end even when origin
/// doesn't answer: a stalled HTTP transfer fails, and ssh runs in batch mode with timeouts
/// ([`SSH`]) unless the user chose an ssh command (`GIT_SSH_COMMAND`, `GIT_SSH` or
/// `core.sshCommand`).
pub fn net(root: &Path, args: &[&str]) -> Command {
    let mut c = unattended(root, args);
    c.env("GIT_HTTP_LOW_SPEED_LIMIT", LOW_SPEED_BYTES)
        .env("GIT_HTTP_LOW_SPEED_TIME", LOW_SPEED_SECS);
    let chosen = ["GIT_SSH_COMMAND", "GIT_SSH"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
        || ok(root, &["config", "core.sshCommand"]);
    if !chosen {
        c.env("GIT_SSH_COMMAND", SSH);
    }
    c
}

/// Run git and report only whether it succeeded.
/// Short shas may differ in length: compare by prefix.
pub fn same_sha(a: &str, b: &str) -> bool {
    !a.is_empty() && !b.is_empty() && (a.starts_with(b) || b.starts_with(a))
}

pub fn ok(cwd: &Path, args: &[&str]) -> bool {
    command()
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// The `git patch-id` of `sha`'s own changes, from [`unit_start`]. `None` when either is not a
/// commit here, or the diff is empty. Whitespace and context lines are ignored, so a base change
/// next to the unit's lines keeps the id.
pub fn diff_id(cwd: &Path, base: &str, sha: &str) -> Option<String> {
    patch_id(cwd, base, sha, false)
}

/// A stricter [`diff_id`] for deciding a branch holds everything: whitespace and context lines
/// count (`git patch-id --verbatim`), and the start is the plain merge base, so work merged in
/// from anywhere stays in the diff. `None` as well when git is too old for `--verbatim`.
pub fn exact_diff_id(cwd: &Path, base: &str, sha: &str) -> Option<String> {
    patch_id(cwd, base, sha, true)
}

fn patch_id(cwd: &Path, base: &str, sha: &str, verbatim: bool) -> Option<String> {
    if sha.is_empty() || sha.starts_with('-') || base.starts_with('-') {
        return None;
    }
    let mb = if verbatim {
        run(cwd, &["merge-base", sha, &freshest(cwd, base)]).ok()?
    } else {
        unit_start(cwd, base, sha)?
    };
    let context = if verbatim { "-U3" } else { "-U0" };
    let diff = command()
        .arg("-C")
        .arg(cwd)
        .args([
            "diff-tree",
            "-p",
            context,
            "--binary",
            "--no-color",
            &mb,
            sha,
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let mut child = command()
        .arg("-C")
        .arg(cwd)
        // `--verbatim` is `--stable` with whitespace kept; git refuses the two together.
        .args(["patch-id", if verbatim { "--verbatim" } else { "--stable" }])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(&diff.stdout);
    }
    let out = child.wait_with_output().ok()?;
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(String::from)
}

/// Whether the unit took in a newer base between `a` and `b`: its own changes start somewhere else
/// ([`unit_start`]), as after a merge of the base or a rebase onto it. False when either start
/// can't be read.
pub fn took_in_base(cwd: &Path, base: &str, a: &str, b: &str) -> bool {
    match (unit_start(cwd, base, a), unit_start(cwd, base, b)) {
        (Some(x), Some(y)) => x != y,
        _ => false,
    }
}

/// How many changed lines of the unit's own diff differ between `a` and `b`: the `+` and `-`
/// lines of each one's diff from its [`unit_start`], per file, with context and line numbers
/// dropped, counted as a multiset difference. 0 when the unit's changes are the same, as after a
/// clean rebase. `None` when either diff can't be read.
pub fn interdiff_lines(cwd: &Path, base: &str, a: &str, b: &str) -> Option<usize> {
    let lines = |sha: &str| -> Option<Vec<String>> {
        if sha.is_empty() || sha.starts_with('-') || base.starts_with('-') {
            return None;
        }
        let start = unit_start(cwd, base, sha)?;
        let out = run(cwd, &["diff-tree", "-p", "-U0", "--no-color", &start, sha]).ok()?;
        let mut file = String::new();
        let mut v = Vec::new();
        for l in out.lines() {
            if let Some(rest) = l.strip_prefix("diff --git ") {
                file = rest.to_string();
            } else if (l.starts_with('+') && !l.starts_with("+++"))
                || (l.starts_with('-') && !l.starts_with("---"))
            {
                v.push(format!("{file}\0{l}"));
            }
        }
        Some(v)
    };
    let (mut x, mut y) = (lines(a)?, lines(b)?);
    x.sort();
    y.sort();
    let (mut i, mut j, mut diff) = (0, 0, 0);
    while i < x.len() && j < y.len() {
        match x[i].cmp(&y[j]) {
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                diff += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                diff += 1;
                j += 1;
            }
        }
    }
    Some(diff + (x.len() - i) + (y.len() - j))
}

/// Where `sha`'s own changes start: the newest common ancestor of `sha` with `base` and with every
/// commit merged into `sha` since that is already in the base. A merge of the base brings in its
/// newer commits even when the `base` ref is behind them, as after `git pull origin <base>`; a
/// merged-in parent that is not an ancestor of the base is the unit's own work.
fn unit_start(cwd: &Path, base: &str, sha: &str) -> Option<String> {
    let base = freshest(cwd, base);
    let not_base = format!("^{base}");
    let merges = run(
        cwd,
        &[
            "rev-list",
            "--first-parent",
            "--merges",
            "--parents",
            sha,
            &not_base,
            "--",
        ],
    )
    .ok()?;
    let mut args = vec!["merge-base", sha, &base];
    args.extend(
        merges
            .lines()
            .flat_map(|l| l.split_whitespace().skip(2))
            .filter(|p| ok(cwd, &["merge-base", "--is-ancestor", p, &base])),
    );
    run(cwd, &args).ok()
}

/// `origin/<base>` when it is strictly ahead of the local `base`, else `base`. A unit rebased
/// onto a fetched `origin/<base>` shares no newer merge base with a stale local branch.
fn freshest<'a>(cwd: &Path, base: &'a str) -> std::borrow::Cow<'a, str> {
    let remote = format!("refs/remotes/origin/{base}");
    let ahead = ok(cwd, &["rev-parse", "--verify", "--quiet", &remote])
        && ok(cwd, &["merge-base", "--is-ancestor", base, &remote])
        && !ok(cwd, &["merge-base", "--is-ancestor", &remote, base]);
    if ahead {
        remote.into()
    } else {
        base.into()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeEntry {
    pub path: PathBuf,
    /// Full ref, e.g. `refs/heads/ns/142-uart-timeout`. `None` when detached.
    pub branch: Option<String>,
    pub bare: bool,
}

/// Parse `git worktree list --porcelain`.
pub fn parse_worktree_list(text: &str) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(e) = current.take() {
                entries.push(e);
            }
            current = Some(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
            });
        } else if let Some(e) = current.as_mut() {
            if let Some(b) = line.strip_prefix("branch ") {
                e.branch = Some(b.to_string());
            } else if line == "bare" {
                e.bare = true;
            }
        }
    }
    if let Some(e) = current {
        entries.push(e);
    }
    entries
}

pub fn worktrees(cwd: &Path) -> Result<Vec<WorktreeEntry>> {
    let text = run(cwd, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&text))
}

/// Context for a repository: the main worktree root and the common git dir.
pub struct Repo {
    pub root: PathBuf,
    pub common_dir: PathBuf,
}

impl Repo {
    /// The run log, `<git-common-dir>/ns/runs.jsonl`.
    pub fn run_log(&self) -> PathBuf {
        self.common_dir.join("ns").join("runs.jsonl")
    }

    /// Resolve the main worktree from any directory inside the repo (or a linked worktree).
    ///
    /// Works out what `git worktree list` gives as its first entry without running it: that
    /// reads every linked worktree's `.git/worktrees/<name>/commondir`, and fails on one a
    /// concurrent `git worktree add` is still writing (#514).
    pub fn discover(start: &Path) -> Result<Repo> {
        if !ok(start, &["rev-parse", "--git-dir"]) {
            return Err(SfError::usage(
                format!("{} is not inside a git repository", start.display()),
                "run from inside a repo, or pass --repo:\n  ns worktree new 142-uart-timeout --repo ~/src/myrepo",
            )
            .into());
        }
        let common = run(
            start,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        // Git's own test for a bare main worktree: `core.bare` set, or no work tree here.
        if run(start, &["rev-parse", "--is-bare-repository"])? == "true"
            || run(start, &["config", "--bool", "core.bare"]).is_ok_and(|v| v == "true")
        {
            return Err(SfError::general(
                "bare repositories are not supported; run from a repo with a main worktree",
            )
            .into());
        }
        // Git names the main worktree after the common dir (a real path, from
        // `--path-format=absolute`), less a trailing `/.git`.
        let common = PathBuf::from(common);
        let root = match common.parent() {
            Some(parent) if common.ends_with(".git") => parent.to_path_buf(),
            _ => common.clone(),
        };
        Ok(Repo {
            root,
            common_dir: common,
        })
    }

    /// The directory of `unit`'s phase `TMPDIR`s, `ns-<repo>-<hash>-<unit>` in the system temp dir
    /// (the `TMPDIR` `ns` inherited if it is absolute, else `/tmp`). `<hash>` is eight hex digits
    /// of the git common dir's path, so two checkouts with one name and one unit id never share a
    /// directory. It is outside the repo, so a phase's scratch file is no write into `.git` for
    /// the hook guard.
    pub fn unit_tmp(&self, unit: &str) -> PathBuf {
        let sys = absolute_temp(std::env::temp_dir());
        let digest = Sha256::digest(self.common_dir.as_os_str().as_encoded_bytes());
        let hash = crate::eval::hex(&digest[..4]);
        sys.join(format!("ns-{}-{hash}-{unit}", self.name()))
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "repo".to_string())
    }

    /// The repo's default branch: origin/HEAD (local branch if present), else the checked-out
    /// branch, else the detached HEAD's sha. Never the literal `HEAD`, which moves with the worktree.
    pub fn default_base(&self) -> Result<String> {
        if let Ok(r) = run(
            &self.root,
            &[
                "symbolic-ref",
                "--quiet",
                "--short",
                "refs/remotes/origin/HEAD",
            ],
        ) {
            if let Some(local) = r.strip_prefix("origin/") {
                if ok(
                    &self.root,
                    &[
                        "rev-parse",
                        "--verify",
                        "--quiet",
                        &format!("refs/heads/{local}"),
                    ],
                ) {
                    return Ok(local.to_string());
                }
            }
            return Ok(r);
        }
        if let Ok(sha) = run(&self.root, &["rev-parse", "--verify", "--quiet", "HEAD"]) {
            return Ok(
                run(&self.root, &["symbolic-ref", "--quiet", "--short", "HEAD"]).unwrap_or(sha),
            );
        }
        Err(
            SfError::general("repository has no commits yet; make a first commit, or pass --base")
                .hint("ns worktree new 142-uart-timeout --base main")
                .into(),
        )
    }
}

/// `temp` if it is absolute, else `/tmp`: a relative `TMPDIR` would resolve against each
/// process's own cwd, so a phase in its worktree and `ns clean` would look in different places.
fn absolute_temp(temp: PathBuf) -> PathBuf {
    if temp.is_absolute() {
        temp
    } else {
        PathBuf::from("/tmp")
    }
}

#[cfg(test)]
mod tests {
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
    fn parses_porcelain() {
        let text = "worktree /r\nHEAD abc\nbranch refs/heads/main\n\nworktree /w/x\nHEAD def\ndetached\n\nworktree /b\nbare\n";
        let e = parse_worktree_list(text);
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].branch.as_deref(), Some("refs/heads/main"));
        assert_eq!(e[1].branch, None);
        assert!(e[2].bare);
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
            let listed = &worktrees(&start).unwrap()[0];
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
            assert!(worktrees(&start).unwrap()[0].bare, "{}", start.display());
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
}
