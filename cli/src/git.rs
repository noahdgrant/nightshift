//! Thin wrapper around the `git` binary.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};

use crate::error::SfError;
use crate::proc::Proc;

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

/// A `git` command on a local repo ([`Proc::git_local`]).
pub fn command() -> Proc {
    #[cfg(test)]
    SPAWNS.with(|c| c.set(c.get() + 1));
    Proc::git_local()
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

/// Run git in `cwd` and return trimmed stdout. Fails with git's stderr on non-zero exit, and
/// without running git for a subcommand that talks to a remote ([`network_subcommand`]), which
/// goes through [`run_net`].
pub fn run(cwd: &Path, args: &[&str]) -> Result<String> {
    if let Some(sub) = network_subcommand(args) {
        return Err(SfError::general(format!(
            "git {sub} talks to a remote: run it with git::run_net, not git::run"
        ))
        .into());
    }
    let mut c = command();
    c.arg("-C").arg(cwd).args(args);
    stdout(c, args)
}

/// [`run`] for a git command that talks to `origin`, under the network policy of [`net`].
pub fn run_net(root: &Path, args: &[&str]) -> Result<String> {
    stdout(net(root, args), args)
}

/// Global git options that take their value as the next argument.
const VALUE_OPTIONS: [&str; 7] = [
    "-c",
    "-C",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--attr-source",
];

/// The subcommand of `args` that talks to a remote (`fetch`, `push`, `pull`, `clone`,
/// `ls-remote`, `remote update`), past any global options before it (`-c k=v`, `--no-pager`).
pub fn network_subcommand<'a>(args: &[&'a str]) -> Option<&'a str> {
    let mut rest = args.iter().copied();
    let sub = loop {
        let a = rest.next()?;
        if VALUE_OPTIONS.contains(&a) {
            rest.next();
        } else if !a.starts_with('-') {
            break a;
        }
    };
    match sub {
        "fetch" | "push" | "pull" | "clone" | "ls-remote" => Some(sub),
        "remote" => rest
            .find(|a| !a.starts_with('-'))
            .filter(|a| *a == "update")
            .map(|_| "remote update"),
        _ => None,
    }
}

fn stdout(mut c: Proc, args: &[&str]) -> Result<String> {
    let out = c
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

/// A transfer that moves fewer bytes a second than this, for `LOW_SPEED_SECS`, fails.
const LOW_SPEED_BYTES: &str = "1000";
const LOW_SPEED_SECS: &str = "60";
/// ssh that never prompts and gives up on a dead connection: 30 s to connect, and about a
/// minute of a server that stops answering.
pub const SSH: &str =
    "ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4";

/// A git command in `root` that talks to `origin` ([`Proc::git_network`], which never prompts
/// for credentials), and must end even when origin doesn't answer: a stalled HTTP transfer
/// fails, and ssh runs in batch mode with timeouts ([`SSH`]) unless the user chose an ssh
/// command (`GIT_SSH_COMMAND`, `GIT_SSH` or `core.sshCommand`).
pub fn net(root: &Path, args: &[&str]) -> Proc {
    #[cfg(test)]
    SPAWNS.with(|c| c.set(c.get() + 1));
    let mut c = Proc::git_network();
    c.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_HTTP_LOW_SPEED_LIMIT", LOW_SPEED_BYTES)
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
    let out = command()
        .arg("-C")
        .arg(cwd)
        // `--verbatim` is `--stable` with whitespace kept; git refuses the two together.
        .args(["patch-id", if verbatim { "--verbatim" } else { "--stable" }])
        .stdin(diff.stdout)
        .output()
        .ok()?;
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
    let (x, y) = (unit_lines(cwd, base, a)?, unit_lines(cwd, base, b)?);
    Some(multiset_diff(&x, &y).len())
}

/// The files whose own changed lines differ between `a` and `b`, as `git diff --name-only`
/// would name them: what a phase that moved HEAD from `a` to `b` changed in the unit, without
/// the files a merge of the base brought in.
pub fn interdiff_files(cwd: &Path, base: &str, a: &str, b: &str) -> Option<Vec<String>> {
    let (x, y) = (unit_lines(cwd, base, a)?, unit_lines(cwd, base, b)?);
    let mut files: Vec<String> = multiset_diff(&x, &y)
        .into_iter()
        .filter_map(|l| {
            let header = l.split('\0').next()?;
            // `diff --git a/<path> b/<path>`: the b side, as --name-only names it.
            header.rsplit_once(" b/").map(|(_, p)| p.to_string())
        })
        .collect();
    files.sort();
    files.dedup();
    Some(files)
}

/// The lines in only one of the two sorted-or-not lists, counting repeats.
fn multiset_diff(a: &[String], b: &[String]) -> Vec<String> {
    let (mut x, mut y) = (a.to_vec(), b.to_vec());
    x.sort();
    y.sort();
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < x.len() && j < y.len() {
        match x[i].cmp(&y[j]) {
            std::cmp::Ordering::Equal => {
                i += 1;
                j += 1;
            }
            std::cmp::Ordering::Less => {
                out.push(x[i].clone());
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                out.push(y[j].clone());
                j += 1;
            }
        }
    }
    out.extend(x[i..].iter().cloned());
    out.extend(y[j..].iter().cloned());
    out
}

/// The unit's own `+` and `-` lines at `sha`, each prefixed by its file's `diff --git` header.
fn unit_lines(cwd: &Path, base: &str, sha: &str) -> Option<Vec<String>> {
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
    lines(sha)
}

/// The files `sha`'s own changes touch, from [`unit_start`]: each path as it is, unquoted, and
/// both ends of a rename.
pub fn unit_files(cwd: &Path, base: &str, sha: &str) -> Option<Vec<String>> {
    let start = unit_start(cwd, base, sha)?;
    let out = run(
        cwd,
        &["diff", "--name-only", "-z", "--no-renames", &start, sha],
    )
    .ok()?;
    Some(
        out.split('\0')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

/// Where `sha`'s own changes start: the newest common ancestor of `sha` with `base` and with every
/// commit merged into `sha` since that is already in the base. A merge of the base brings in its
/// newer commits even when the `base` ref is behind them, as after `git pull origin <base>`; a
/// merged-in parent that is not an ancestor of the base is the unit's own work.
fn unit_start(cwd: &Path, base: &str, sha: &str) -> Option<String> {
    if sha.is_empty() || sha.starts_with('-') || base.starts_with('-') {
        return None;
    }
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
mod tests;
