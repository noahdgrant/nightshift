//! The `nightshift/quality` data branch on a remote (`[quality] remote`, `origin` by default): one `records.jsonl` that `ns` appends to
//! unattended (docs/decisions/ D39). Commits are built with plumbing (`hash-object`, `mktree`,
//! `commit-tree`), so writing never touches a working tree, an index or a local branch.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::git;
use crate::proc::Proc;

pub const BRANCH: &str = "nightshift/quality";
pub const FILE: &str = "records.jsonl";
/// The ref that tracks `remote`'s branch.
pub fn tracking(remote: &str) -> String {
    format!("refs/remotes/{remote}/{BRANCH}")
}
/// Pushes retried after the first one loses a race to another writer.
pub const RETRIES: u32 = 3;
/// Why records stay in the outbox under `[quality] remote = "none"`.
pub const NO_REMOTE: &str = "[quality] remote = \"none\" keeps the records in the outbox";

/// Where [`publish`] sends records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest<'a> {
    /// Fetch and push this remote's branch.
    Remote(&'a str),
    /// Keep them in the outbox, and say why.
    Outbox(String),
}

impl Dest<'_> {
    /// `[quality] remote = "none"`.
    pub fn none() -> Dest<'static> {
        Dest::Outbox(NO_REMOTE.into())
    }
}

pub fn outbox(common: &Path) -> PathBuf {
    common.join("ns").join("quality-outbox.jsonl")
}

// `publish` holds the outbox lock across its network calls, so they must end (git::net).
use git::net;

/// Local git in `root`.
fn cmd(root: &Path, args: &[&str]) -> Proc {
    let mut c = git::command();
    c.arg("-C").arg(root).args(args);
    c
}

fn fail(args: &[&str], stderr: &[u8]) -> anyhow::Error {
    anyhow::anyhow!(
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(stderr).trim()
    )
}

/// Run git with `input` on stdin and return its raw stdout.
fn piped(root: &Path, args: &[&str], input: &[u8], ident: bool) -> Result<Vec<u8>> {
    let mut c = cmd(root, args);
    if ident {
        let known = cmd(root, &["var", "GIT_COMMITTER_IDENT"])
            .output()
            .is_ok_and(|o| o.status.success());
        fallback_identity(&mut c, known);
    }
    let out = c
        .stdin(input.to_vec())
        .output()
        .context("failed to run git; is it installed and on PATH?")?;
    if !out.status.success() {
        return Err(fail(args, &out.stderr));
    }
    Ok(out.stdout)
}

fn text(root: &Path, args: &[&str], input: &[u8]) -> Result<String> {
    let out = piped(root, args, input, false)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

/// Commit as the configured user, or as `nightshift` when the repo has no identity.
fn fallback_identity(c: &mut Proc, known: bool) {
    if !known {
        for (k, v) in [
            ("GIT_AUTHOR_NAME", "nightshift"),
            ("GIT_AUTHOR_EMAIL", "nightshift@localhost"),
            ("GIT_COMMITTER_NAME", "nightshift"),
            ("GIT_COMMITTER_EMAIL", "nightshift@localhost"),
        ] {
            c.env(k, v);
        }
    }
}

/// Bring the tracking ref up to `remote`'s branch. `Ok(None)` when the remote has no such
/// branch (the stale tracking ref is dropped); an error when there is no such remote or it
/// can't be reached.
pub fn fetch(root: &Path, remote: &str) -> Result<Option<String>> {
    let head = format!("refs/heads/{BRANCH}");
    let track = tracking(remote);
    let known = git::run(root, &["remote"]).unwrap_or_default();
    if !known.lines().any(|r| r == remote) {
        bail!("no remote named {remote:?} in this repository (git remote get-url {remote})");
    }
    let args = ["ls-remote", "--exit-code", "--", remote, head.as_str()];
    let out = net(root, &args).output().context("failed to run git")?;
    if out.status.code() == Some(2) {
        let _ = git::run(root, &["update-ref", "-d", &track]);
        return Ok(None);
    }
    if !out.status.success() {
        return Err(fail(&args, &out.stderr));
    }
    let listed = String::from_utf8_lossy(&out.stdout);
    let Some(sha) = listed.split_whitespace().next().map(String::from) else {
        bail!("git ls-remote {remote} {head} printed nothing");
    };
    let have = git::run(root, &["rev-parse", "--verify", "--quiet", &track]).ok();
    if have.as_deref() != Some(sha.as_str()) {
        let spec = format!("+{head}:{track}");
        let args = ["fetch", "--quiet", "--no-tags", "--", remote, spec.as_str()];
        let out = net(root, &args).output().context("failed to run git")?;
        if !out.status.success() {
            return Err(fail(&args, &out.stderr));
        }
    }
    Ok(Some(sha))
}

/// `records.jsonl` at `commit`, or empty when the commit has none.
fn file_at(root: &Path, commit: Option<&str>) -> Result<String> {
    let Some(c) = commit else {
        return Ok(String::new());
    };
    let spec = format!("{c}:{FILE}");
    if !git::ok(root, &["cat-file", "-e", &spec]) {
        return Ok(String::new());
    }
    let out = piped(root, &["cat-file", "blob", &spec], b"", false)?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The records on `remote` as last fetched.
pub fn tracked(root: &Path, remote: &str) -> Result<String> {
    let tip = git::run(
        root,
        &["rev-parse", "--verify", "--quiet", &tracking(remote)],
    )
    .ok();
    file_at(root, tip.as_deref())
}

/// A commit on `parent` (none: a root commit) whose `records.jsonl` gains `lines`. Other
/// files in the parent's tree are kept.
fn commit(root: &Path, parent: Option<&str>, lines: &[String]) -> Result<String> {
    let mut content = file_at(root, parent)?;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    for l in lines {
        content.push_str(l);
        content.push('\n');
    }
    let blob = text(root, &["hash-object", "-w", "--stdin"], content.as_bytes())?;
    let mut tree = match parent {
        Some(p) => git::run(root, &["ls-tree", p])?,
        None => String::new(),
    }
    .lines()
    .filter(|l| l.split('\t').nth(1) != Some(FILE))
    .map(|l| format!("{l}\n"))
    .collect::<String>();
    tree.push_str(&format!("100644 blob {blob}\t{FILE}\n"));
    let tree = text(root, &["mktree"], tree.as_bytes())?;
    let msg = format!("quality: {} record(s)", lines.len());
    let mut args = vec!["commit-tree", tree.as_str(), "-m", msg.as_str()];
    if let Some(p) = parent {
        args.extend(["-p", p]);
    }
    let out = piped(root, &args, b"", true)?;
    Ok(String::from_utf8_lossy(&out).trim().to_string())
}

enum Pushed {
    Done,
    /// The remote's branch moved since the fetch: another writer won the race.
    Behind(anyhow::Error),
    /// Anything else (no access, a hook declined, the network): retrying won't help.
    Failed(anyhow::Error),
}

/// Whether `git push --porcelain` output reports a non-fast-forward rejection (`[rejected]`
/// with `fetch first` or `non-fast-forward`), as opposed to `[remote rejected]` by a hook.
fn behind(porcelain: &str) -> bool {
    porcelain
        .lines()
        .any(|l| l.starts_with('!') && l.contains("\t[rejected]"))
}

fn push(root: &Path, remote: &str, commit: &str) -> Pushed {
    let spec = format!("{commit}:refs/heads/{BRANCH}");
    let args = [
        "push",
        "--porcelain",
        "--no-verify",
        "--",
        remote,
        spec.as_str(),
    ];
    let out = match net(root, &args).output() {
        Ok(o) => o,
        Err(e) => return Pushed::Failed(anyhow::Error::new(e).context("failed to run git")),
    };
    if !out.status.success() {
        let stdout = String::from_utf8_lossy(&out.stdout);
        let err = fail(
            &args,
            &[out.stderr.as_slice(), out.stdout.as_slice()].concat(),
        );
        return if behind(&stdout) {
            Pushed::Behind(err)
        } else {
            Pushed::Failed(err)
        };
    }
    // The push landed; a tracking ref left stale is refreshed by the next fetch.
    if let Err(e) = git::run(root, &["update-ref", &tracking(remote), commit]) {
        eprintln!("ns: pushed the quality records, but {e:#}");
    }
    Pushed::Done
}

#[derive(Debug, PartialEq)]
pub enum Sent {
    /// No new records and an empty outbox.
    Nothing,
    Pushed {
        records: usize,
        tries: u32,
    },
    /// Kept in the outbox for the next write.
    Outbox {
        records: usize,
        reason: String,
    },
}

fn read_lines(p: &Path) -> Vec<String> {
    fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(String::from)
        .collect()
}

fn write_outbox(p: &Path, lines: &[String]) -> Result<()> {
    let tmp = p.with_extension("jsonl.tmp");
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    fs::write(&tmp, body).with_context(|| format!("cannot write {}", tmp.display()))?;
    fs::rename(&tmp, p).with_context(|| format!("cannot write {}", p.display()))
}

/// Append `lines`, plus any the outbox holds, to `remote`'s branch: fetch, commit on its tip,
/// push, retrying `RETRIES` times when the push is rejected because another writer won the
/// race. When that still fails, any other push or fetch error, or no such remote, the lines
/// wait in the outbox for the next call. With no remote (`[quality] remote = "none"`) they go
/// to the outbox and nothing is fetched or pushed.
pub fn publish(root: &Path, common: &Path, dest: Dest<'_>, lines: &[String]) -> Result<Sent> {
    publish_with(root, common, dest, lines, &mut |_| {})
}

/// [`publish`], calling `before_push` with the try number (1-based) between fetch and push.
pub fn publish_with(
    root: &Path,
    common: &Path,
    dest: Dest<'_>,
    lines: &[String],
    before_push: &mut dyn FnMut(u32),
) -> Result<Sent> {
    let dir = common.join("ns");
    fs::create_dir_all(&dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join("quality.lock"))
        .context("cannot open the quality lock")?;
    File::lock(&lock).context("cannot lock the quality outbox")?;
    let ob = outbox(common);
    let mut pending = read_lines(&ob);
    pending.extend(lines.iter().cloned());
    if pending.is_empty() {
        return Ok(Sent::Nothing);
    }
    let remote = match dest {
        Dest::Remote(r) => r,
        Dest::Outbox(reason) => {
            if !lines.is_empty() {
                write_outbox(&ob, &pending)?;
            }
            return Ok(Sent::Outbox {
                records: pending.len(),
                reason,
            });
        }
    };
    let mut reason = String::new();
    for try_no in 1..=1 + RETRIES {
        let tip = match fetch(root, remote) {
            Ok(t) => t,
            Err(e) => {
                reason = format!("{e:#}");
                break;
            }
        };
        let made = match commit(root, tip.as_deref(), &pending) {
            Ok(c) => c,
            Err(e) => {
                reason = format!("{e:#}");
                break;
            }
        };
        before_push(try_no);
        match push(root, remote, &made) {
            Pushed::Done => {
                if ob.exists() {
                    fs::remove_file(&ob)
                        .with_context(|| format!("cannot clear {}", ob.display()))?;
                }
                return Ok(Sent::Pushed {
                    records: pending.len(),
                    tries: try_no,
                });
            }
            Pushed::Behind(e) => reason = format!("{e:#}"),
            Pushed::Failed(e) => {
                reason = format!("{e:#}");
                break;
            }
        }
    }
    write_outbox(&ob, &pending)?;
    Ok(Sent::Outbox {
        records: pending.len(),
        reason,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g, write_exe};

    const O: Dest<'static> = Dest::Remote("origin");

    struct Setup {
        _tmp: tempfile::TempDir,
        base: PathBuf,
        remote: PathBuf,
        repo: PathBuf,
    }

    fn clone(base: &Path, remote: &Path, name: &str) -> PathBuf {
        let dir = base.join(name);
        g(base, &["clone", "-q", remote.to_str().unwrap(), name]);
        dir
    }

    /// A bare `remote.git` with one commit on `main`, cloned as `repo`.
    fn setup() -> Setup {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let seed = base.join("seed");
        fs::create_dir(&seed).unwrap();
        g(&seed, &["init", "-q", "-b", "main"]);
        commit_file(&seed, "README", "hi\n");
        let remote = base.join("remote.git");
        g(&base, &["clone", "-q", "--bare", "seed", "remote.git"]);
        let repo = clone(&base, &remote, "repo");
        Setup {
            _tmp: tmp,
            base,
            remote,
            repo,
        }
    }

    fn lines(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn remote_file(s: &Setup) -> String {
        g(&s.remote, &["show", &format!("refs/heads/{BRANCH}:{FILE}")]) + "\n"
    }

    fn common(repo: &Path) -> PathBuf {
        repo.join(".git")
    }

    #[test]
    fn the_first_write_creates_an_orphan_branch_holding_only_the_records() {
        let s = setup();
        // A clone that fetches only main: push alone wouldn't move the tracking ref.
        let only_main = "+refs/heads/main:refs/remotes/origin/main";
        g(&s.repo, &["config", "remote.origin.fetch", only_main]);
        let sent = publish(&s.repo, &common(&s.repo), O, &lines(&["{\"a\":1}"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 1
            }
        );
        assert_eq!(remote_file(&s), "{\"a\":1}\n");
        let tip = format!("refs/heads/{BRANCH}");
        assert_eq!(g(&s.remote, &["rev-list", "--count", &tip]), "1");
        assert_eq!(g(&s.remote, &["ls-tree", "--name-only", &tip]), FILE);
        assert!(!git::ok(&s.remote, &["merge-base", &tip, "main"]));
        assert_eq!(
            g(&s.repo, &["rev-parse", &tracking("origin")]),
            g(&s.remote, &["rev-parse", &tip])
        );
    }

    #[test]
    fn a_later_write_appends_on_top() {
        let s = setup();
        let c = common(&s.repo);
        publish(&s.repo, &c, O, &lines(&["one"])).unwrap();
        publish(&s.repo, &c, O, &lines(&["two", "three"])).unwrap();
        assert_eq!(remote_file(&s), "one\ntwo\nthree\n");
        let tip = format!("refs/heads/{BRANCH}");
        assert_eq!(g(&s.remote, &["rev-list", "--count", &tip]), "2");
        assert_eq!(publish(&s.repo, &c, O, &[]).unwrap(), Sent::Nothing);
    }

    /// Exempt from the hooks rule (ns-contract, Git hooks): the records branch holds no code,
    /// and a pre-push hook checks the working tree, so a red or slow tree must not hold up a
    /// record.
    #[test]
    fn the_records_push_skips_the_checkouts_pre_push_hook() {
        let s = setup();
        let hook = s.repo.join(".git/hooks/pre-push");
        write_exe(&hook, "#!/bin/sh\ntouch ran\nexit 1\n");
        let sent = publish(&s.repo, &common(&s.repo), O, &lines(&["x"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 1
            }
        );
        assert!(!s.repo.join("ran").exists());
    }

    #[test]
    fn writing_leaves_the_working_tree_index_and_head_alone() {
        let s = setup();
        fs::write(s.repo.join("README"), "edited\n").unwrap();
        fs::write(s.repo.join("staged"), "s\n").unwrap();
        g(&s.repo, &["add", "staged"]);
        let before = (
            g(&s.repo, &["status", "--porcelain"]),
            g(&s.repo, &["diff", "--cached"]),
            g(&s.repo, &["rev-parse", "HEAD"]),
            g(&s.repo, &["symbolic-ref", "HEAD"]),
        );
        publish(&s.repo, &common(&s.repo), O, &lines(&["x"])).unwrap();
        let after = (
            g(&s.repo, &["status", "--porcelain"]),
            g(&s.repo, &["diff", "--cached"]),
            g(&s.repo, &["rev-parse", "HEAD"]),
            g(&s.repo, &["symbolic-ref", "HEAD"]),
        );
        assert_eq!(before, after);
        assert!(!git::ok(
            &s.repo,
            &["rev-parse", "--verify", &format!("refs/heads/{BRANCH}")]
        ));
    }

    #[test]
    fn other_files_on_the_branch_are_kept_and_a_hand_edit_keeps_its_last_line() {
        let s = setup();
        publish(&s.repo, &common(&s.repo), O, &lines(&["one"])).unwrap();
        let other = clone(&s.base, &s.remote, "other");
        g(&other, &["checkout", "-q", BRANCH]);
        commit_file(&other, "README.md", "data branch\n");
        commit_file(&other, FILE, "one");
        g(&other, &["push", "-q", "origin", BRANCH]);
        publish(&s.repo, &common(&s.repo), O, &lines(&["two"])).unwrap();
        let tip = format!("refs/heads/{BRANCH}");
        assert_eq!(
            g(&s.remote, &["ls-tree", "--name-only", &tip]),
            format!("README.md\n{FILE}")
        );
        assert_eq!(remote_file(&s), "one\ntwo\n");
    }

    #[test]
    fn a_push_that_loses_the_race_fetches_and_appends_again() {
        let s = setup();
        let other = clone(&s.base, &s.remote, "other");
        let mut raced = Vec::new();
        let sent = publish_with(&s.repo, &common(&s.repo), O, &lines(&["ours"]), &mut |n| {
            raced.push(n);
            if n == 1 {
                publish(&other, &common(&other), O, &lines(&["theirs"])).unwrap();
            }
        })
        .unwrap();
        assert_eq!(raced, [1, 2]);
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 2
            }
        );
        assert_eq!(remote_file(&s), "theirs\nours\n");
        assert!(!outbox(&common(&s.repo)).exists());
    }

    #[test]
    fn a_push_that_keeps_losing_goes_to_the_outbox_and_a_later_write_flushes_it() {
        let s = setup();
        let c = common(&s.repo);
        let other = clone(&s.base, &s.remote, "other");
        let mut tries = 0;
        let sent = publish_with(&s.repo, &c, O, &lines(&["a", "b"]), &mut |n| {
            tries = n;
            publish(
                &other,
                &common(&other),
                O,
                &lines(&[&format!("theirs-{n}")]),
            )
            .unwrap();
        })
        .unwrap();
        assert_eq!(tries, 1 + RETRIES);
        let Sent::Outbox { records, reason } = sent else {
            panic!("{sent:?}")
        };
        assert_eq!(records, 2);
        assert!(reason.contains("push"), "{reason}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "a\nb\n");
        assert!(!remote_file(&s).contains("\na\n"));

        let sent = publish(&s.repo, &c, O, &lines(&["c"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 3,
                tries: 1
            }
        );
        assert_eq!(
            remote_file(&s),
            "theirs-1\ntheirs-2\ntheirs-3\ntheirs-4\na\nb\nc\n"
        );
        assert!(!outbox(&c).exists());
    }

    #[test]
    fn no_origin_keeps_records_in_the_outbox_until_one_is_added() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("solo");
        fs::create_dir(&repo).unwrap();
        g(&repo, &["init", "-q", "-b", "main"]);
        let c = common(&repo);
        let sent = publish(&repo, &c, O, &lines(&["one"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        let sent = publish(&repo, &c, O, &lines(&["two"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 2, .. }), "{sent:?}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "one\ntwo\n");

        let remote = tmp.path().join("remote.git");
        g(tmp.path(), &["init", "-q", "--bare", "remote.git"]);
        g(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        assert_eq!(
            publish(&repo, &c, O, &[]).unwrap(),
            Sent::Pushed {
                records: 2,
                tries: 1
            }
        );
        assert_eq!(tracked(&repo, "origin").unwrap(), "one\ntwo\n");
    }

    #[test]
    fn a_push_a_hook_declines_goes_to_the_outbox_without_retrying() {
        let s = setup();
        let hook = s.remote.join("hooks/pre-receive");
        write_exe(&hook, "#!/bin/sh\necho no >&2\nexit 1\n");
        let mut pushes = 0;
        let sent = publish_with(&s.repo, &common(&s.repo), O, &lines(&["x"]), &mut |_| {
            pushes += 1
        })
        .unwrap();
        assert_eq!(pushes, 1);
        let Sent::Outbox { reason, .. } = sent else {
            panic!("{sent:?}")
        };
        assert!(reason.contains("remote rejected"), "{reason}");
    }

    #[test]
    fn only_a_non_fast_forward_rejection_is_a_lost_race() {
        let ok = "To /r.git\n=\tabc:refs/heads/x\t[up to date]\nDone\n";
        let race = "To /r.git\n!\tabc:refs/heads/x\t[rejected] (fetch first)\nDone\n";
        let nff = "To /r.git\n!\tabc:refs/heads/x\t[rejected] (non-fast-forward)\nDone\n";
        let hook =
            "To /r.git\n!\tabc:refs/heads/x\t[remote rejected] (pre-receive hook declined)\n";
        assert!(behind(race) && behind(nff));
        assert!(!behind(ok) && !behind(hook) && !behind(""));
    }

    #[test]
    fn a_landed_push_counts_even_if_the_tracking_ref_cannot_be_updated() {
        let s = setup();
        let lock = common(&s.repo).join(format!("{}.lock", tracking("origin")));
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, "").unwrap();
        let sent = publish(&s.repo, &common(&s.repo), O, &lines(&["x"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 1
            }
        );
        assert_eq!(remote_file(&s), "x\n");
        assert!(!outbox(&common(&s.repo)).exists());
    }

    #[test]
    fn network_commands_never_prompt_and_keep_a_chosen_ssh_command() {
        let s = setup();
        let env = |c: &Proc, k: &str| {
            c.get_envs()
                .find(|(n, _)| *n == k)
                .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
        };
        let c = net(&s.repo, &["fetch"]);
        assert_eq!(env(&c, "GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(env(&c, "GCM_INTERACTIVE").as_deref(), Some("never"));
        assert_eq!(env(&c, "GIT_HTTP_LOW_SPEED_LIMIT").as_deref(), Some("1000"));
        assert_eq!(env(&c, "GIT_HTTP_LOW_SPEED_TIME").as_deref(), Some("60"));
        let inherited = ["GIT_SSH_COMMAND", "GIT_SSH"]
            .iter()
            .any(|v| std::env::var_os(v).is_some());
        if !inherited {
            assert_eq!(env(&c, "GIT_SSH_COMMAND").as_deref(), Some(git::SSH));
        }
        g(&s.repo, &["config", "core.sshCommand", "ssh -i key"]);
        let c = net(&s.repo, &["fetch"]);
        assert_eq!(env(&c, "GIT_SSH_COMMAND"), None);
    }

    #[test]
    fn an_unreachable_origin_goes_to_the_outbox_without_retrying() {
        let s = setup();
        g(
            &s.repo,
            &["remote", "set-url", "origin", "/nonexistent/remote.git"],
        );
        let mut pushes = 0;
        let sent = publish_with(&s.repo, &common(&s.repo), O, &lines(&["x"]), &mut |_| {
            pushes += 1
        })
        .unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        assert_eq!(pushes, 0);
    }

    #[test]
    fn fetch_tells_a_missing_branch_from_an_unreachable_origin_and_drops_a_stale_ref() {
        let s = setup();
        assert_eq!(fetch(&s.repo, "origin").unwrap(), None);
        publish(&s.repo, &common(&s.repo), O, &lines(&["x"])).unwrap();
        let other = clone(&s.base, &s.remote, "other");
        let tip = fetch(&other, "origin").unwrap().unwrap();
        assert_eq!(g(&other, &["rev-parse", &tracking("origin")]), tip);
        assert_eq!(tracked(&other, "origin").unwrap(), "x\n");
        g(
            &s.remote,
            &["update-ref", "-d", &format!("refs/heads/{BRANCH}")],
        );
        assert_eq!(fetch(&other, "origin").unwrap(), None);
        assert_eq!(tracked(&other, "origin").unwrap(), "");
        g(&other, &["remote", "remove", "origin"]);
        assert!(fetch(&other, "origin").is_err());
    }

    #[test]
    fn a_named_remote_carries_the_branch_and_its_own_tracking_ref() {
        let s = setup();
        let records = s.base.join("records.git");
        g(&s.base, &["init", "-q", "--bare", "records.git"]);
        g(
            &s.repo,
            &["remote", "add", "records", records.to_str().unwrap()],
        );
        let c = common(&s.repo);
        let sent = publish(&s.repo, &c, Dest::Remote("records"), &lines(&["x"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 1
            }
        );
        let head = format!("refs/heads/{BRANCH}");
        assert_eq!(g(&records, &["show", &format!("{head}:{FILE}")]), "x");
        assert!(!git::ok(&s.remote, &["rev-parse", "--verify", &head]));
        assert_eq!(
            tracking("records"),
            "refs/remotes/records/nightshift/quality"
        );
        assert_eq!(
            g(&s.repo, &["rev-parse", &tracking("records")]),
            g(&records, &["rev-parse", &head])
        );
        assert!(!git::ok(
            &s.repo,
            &["rev-parse", "--verify", &tracking("origin")]
        ));
        assert_eq!(tracked(&s.repo, "records").unwrap(), "x\n");
        assert_eq!(tracked(&s.repo, "origin").unwrap(), "");
        let other = clone(&s.base, &s.remote, "other");
        g(
            &other,
            &["remote", "add", "records", records.to_str().unwrap()],
        );
        let tip = fetch(&other, "records").unwrap().unwrap();
        assert_eq!(g(&other, &["rev-parse", &tracking("records")]), tip);
        assert_eq!(tracked(&other, "records").unwrap(), "x\n");
    }

    #[test]
    fn a_missing_named_remote_waits_in_the_outbox_naming_it() {
        let s = setup();
        let c = common(&s.repo);
        let sent = publish(&s.repo, &c, Dest::Remote("records"), &lines(&["x"])).unwrap();
        let Sent::Outbox { records, reason } = sent else {
            panic!("{sent:?}")
        };
        assert_eq!(records, 1);
        assert!(reason.contains("records"), "{reason}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "x\n");
    }

    #[test]
    fn a_remote_name_git_would_read_as_an_option_creates_nothing_and_waits_in_the_outbox() {
        let s = setup();
        let c = common(&s.repo);
        let evil = "--upload-pack=touch pwned";
        let sent = publish(&s.repo, &c, Dest::Remote(evil), &lines(&["x"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        assert!(fetch(&s.repo, evil).is_err());
        assert!(!s.repo.join("pwned").exists());
        assert!(!Path::new("pwned").exists());
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "x\n");
    }

    #[test]
    fn a_remote_whose_configured_name_starts_with_a_dash_still_fetches_and_pushes() {
        let s = setup();
        let records = s.base.join("records.git");
        g(&s.base, &["init", "-q", "--bare", "records.git"]);
        let url = records.to_str().unwrap();
        g(&s.repo, &["config", "remote.-x.url", url]);
        g(
            &s.repo,
            &[
                "config",
                "remote.-x.fetch",
                "+refs/heads/*:refs/remotes/-x/*",
            ],
        );
        let c = common(&s.repo);
        let sent = publish(&s.repo, &c, Dest::Remote("-x"), &lines(&["x"])).unwrap();
        assert_eq!(
            sent,
            Sent::Pushed {
                records: 1,
                tries: 1
            }
        );
        let head = format!("refs/heads/{BRANCH}");
        assert_eq!(g(&records, &["show", &format!("{head}:{FILE}")]), "x");
        let other = clone(&s.base, &s.remote, "other");
        g(&other, &["config", "remote.-x.url", url]);
        let tip = fetch(&other, "-x").unwrap().unwrap();
        assert_eq!(g(&other, &["rev-parse", &tracking("-x")]), tip);
        assert_eq!(tracked(&other, "-x").unwrap(), "x\n");
    }

    #[test]
    fn a_url_is_not_a_remote_name() {
        let s = setup();
        let url = s.remote.to_str().unwrap();
        let err = fetch(&s.repo, url).unwrap_err();
        assert!(format!("{err:#}").contains("no remote named"), "{err:#}");
    }

    #[test]
    fn no_remote_writes_the_outbox_and_never_fetches_or_pushes() {
        let s = setup();
        let c = common(&s.repo);
        publish(&s.repo, &c, O, &lines(&["old"])).unwrap();
        g(&s.repo, &["update-ref", "-d", &tracking("origin")]);
        g(
            &s.repo,
            &["remote", "set-url", "origin", "/nonexistent/remote.git"],
        );
        let mut pushes = 0;
        let sent = publish_with(&s.repo, &c, Dest::none(), &lines(&["a", "b"]), &mut |_| {
            pushes += 1
        })
        .unwrap();
        assert_eq!(pushes, 0);
        assert_eq!(
            sent,
            Sent::Outbox {
                records: 2,
                reason: NO_REMOTE.into()
            }
        );
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "a\nb\n");
        let sent = publish(&s.repo, &c, Dest::none(), &lines(&["c"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 3, .. }), "{sent:?}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "a\nb\nc\n");
        assert!(!git::ok(
            &s.repo,
            &["rev-parse", "--verify", &tracking("origin")]
        ));
        assert_eq!(remote_file(&s), "old\n");
    }

    #[test]
    fn no_remote_and_nothing_new_leaves_the_outbox_untouched() {
        let s = setup();
        let c = common(&s.repo);
        assert_eq!(
            publish(&s.repo, &c, Dest::none(), &[]).unwrap(),
            Sent::Nothing
        );
        assert!(!outbox(&c).exists());
        fs::create_dir_all(c.join("ns")).unwrap();
        fs::write(outbox(&c), "a\n").unwrap();
        // A rewrite renames a new file over it, which changes the inode.
        let inode = || std::os::unix::fs::MetadataExt::ino(&fs::metadata(outbox(&c)).unwrap());
        let before = inode();
        let sent = publish(&s.repo, &c, Dest::none(), &[]).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "a\n");
        assert_eq!(inode(), before);
    }

    #[test]
    fn without_an_identity_the_commit_is_by_nightshift() {
        let set = |c: &Proc| -> Vec<(String, String)> {
            c.get_envs()
                .filter_map(|(k, v)| Some((k.to_str()?.into(), v?.to_str()?.into())))
                .collect()
        };
        let mut c = Proc::tool("true");
        fallback_identity(&mut c, true);
        assert_eq!(set(&c), []);
        fallback_identity(&mut c, false);
        let envs = set(&c);
        let envs: Vec<_> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(envs.len(), 4);
        assert!(envs.contains(&("GIT_COMMITTER_EMAIL", "nightshift@localhost")));
    }
}
