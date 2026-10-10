//! The `nightshift/quality` data branch on `origin`: one `records.jsonl` that `ns` appends to
//! unattended (docs/DESIGN.md D39). Commits are built with plumbing (`hash-object`, `mktree`,
//! `commit-tree`), so writing never touches a working tree, an index or a local branch.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use crate::git;

pub const BRANCH: &str = "nightshift/quality";
pub const FILE: &str = "records.jsonl";
pub const TRACKING: &str = "refs/remotes/origin/nightshift/quality";
/// Pushes retried after the first one loses a race to another writer.
pub const RETRIES: u32 = 3;
/// A network command that moves fewer bytes a second than this, for `LOW_SPEED_SECS`, fails.
const LOW_SPEED_BYTES: &str = "1000";
const LOW_SPEED_SECS: &str = "60";
/// ssh that never prompts and gives up on a dead connection: 30 s to connect, and about a
/// minute of a server that stops answering.
const SSH: &str =
    "ssh -o BatchMode=yes -o ConnectTimeout=30 -o ServerAliveInterval=15 -o ServerAliveCountMax=4";

pub fn outbox(common: &Path) -> PathBuf {
    common.join("ns").join("quality-outbox.jsonl")
}

/// A git command that never prompts for credentials: `ns` writes the branch unattended.
fn cmd(root: &Path, args: &[&str]) -> Command {
    let mut c = git::command();
    c.arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null());
    c
}

/// [`cmd`] for a command that talks to `origin`: a stalled HTTP transfer fails, and ssh runs
/// in batch mode with timeouts ([`SSH`]) unless the user chose an ssh command (`GIT_SSH_COMMAND`, `GIT_SSH` or
/// `core.sshCommand`). `publish` holds the outbox lock across these calls, so they must end.
fn net(root: &Path, args: &[&str]) -> Command {
    let mut c = cmd(root, args);
    c.env("GIT_HTTP_LOW_SPEED_LIMIT", LOW_SPEED_BYTES)
        .env("GIT_HTTP_LOW_SPEED_TIME", LOW_SPEED_SECS);
    let chosen = ["GIT_SSH_COMMAND", "GIT_SSH"]
        .iter()
        .any(|v| std::env::var_os(v).is_some())
        || git::ok(root, &["config", "core.sshCommand"]);
    if !chosen {
        c.env("GIT_SSH_COMMAND", SSH);
    }
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
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to run git; is it installed and on PATH?")?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input)?;
    let out = child.wait_with_output()?;
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
fn fallback_identity(c: &mut Command, known: bool) {
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

/// Bring `TRACKING` up to origin's branch. `Ok(None)` when origin has no such branch (the
/// stale tracking ref is dropped); an error when there is no `origin` or it can't be reached.
pub fn fetch(root: &Path) -> Result<Option<String>> {
    let head = format!("refs/heads/{BRANCH}");
    let args = ["ls-remote", "--exit-code", "origin", head.as_str()];
    let out = net(root, &args).output().context("failed to run git")?;
    if out.status.code() == Some(2) {
        let _ = git::run(root, &["update-ref", "-d", TRACKING]);
        return Ok(None);
    }
    if !out.status.success() {
        return Err(fail(&args, &out.stderr));
    }
    let listed = String::from_utf8_lossy(&out.stdout);
    let Some(sha) = listed.split_whitespace().next().map(String::from) else {
        bail!("git ls-remote origin {head} printed nothing");
    };
    let have = git::run(root, &["rev-parse", "--verify", "--quiet", TRACKING]).ok();
    if have.as_deref() != Some(sha.as_str()) {
        let spec = format!("+{head}:{TRACKING}");
        let args = ["fetch", "--quiet", "--no-tags", "origin", spec.as_str()];
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

/// The records on origin as last fetched.
pub fn tracked(root: &Path) -> Result<String> {
    let tip = git::run(root, &["rev-parse", "--verify", "--quiet", TRACKING]).ok();
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
    /// Origin's branch moved since the fetch: another writer won the race.
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

fn push(root: &Path, commit: &str) -> Pushed {
    let spec = format!("{commit}:refs/heads/{BRANCH}");
    let args = [
        "push",
        "--porcelain",
        "--no-verify",
        "origin",
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
    if let Err(e) = git::run(root, &["update-ref", TRACKING, commit]) {
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

/// Append `lines`, plus any the outbox holds, to origin's branch: fetch, commit on its tip,
/// push, retrying `RETRIES` times when the push is rejected because another writer won the
/// race. When that still fails, any other push or fetch error, or no `origin`, the lines wait
/// in the outbox for the next call.
pub fn publish(root: &Path, common: &Path, lines: &[String]) -> Result<Sent> {
    publish_with(root, common, lines, &mut |_| {})
}

/// [`publish`], calling `before_push` with the try number (1-based) between fetch and push.
pub fn publish_with(
    root: &Path,
    common: &Path,
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
    let mut reason = String::new();
    for try_no in 1..=1 + RETRIES {
        let tip = match fetch(root) {
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
        match push(root, &made) {
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
    use crate::testutil::{commit_file, g};

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
        let sent = publish(&s.repo, &common(&s.repo), &lines(&["{\"a\":1}"])).unwrap();
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
            g(&s.repo, &["rev-parse", TRACKING]),
            g(&s.remote, &["rev-parse", &tip])
        );
    }

    #[test]
    fn a_later_write_appends_on_top() {
        let s = setup();
        let c = common(&s.repo);
        publish(&s.repo, &c, &lines(&["one"])).unwrap();
        publish(&s.repo, &c, &lines(&["two", "three"])).unwrap();
        assert_eq!(remote_file(&s), "one\ntwo\nthree\n");
        let tip = format!("refs/heads/{BRANCH}");
        assert_eq!(g(&s.remote, &["rev-list", "--count", &tip]), "2");
        assert_eq!(publish(&s.repo, &c, &[]).unwrap(), Sent::Nothing);
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
        publish(&s.repo, &common(&s.repo), &lines(&["x"])).unwrap();
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
        publish(&s.repo, &common(&s.repo), &lines(&["one"])).unwrap();
        let other = clone(&s.base, &s.remote, "other");
        g(&other, &["checkout", "-q", BRANCH]);
        commit_file(&other, "README.md", "data branch\n");
        commit_file(&other, FILE, "one");
        g(&other, &["push", "-q", "origin", BRANCH]);
        publish(&s.repo, &common(&s.repo), &lines(&["two"])).unwrap();
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
        let sent = publish_with(&s.repo, &common(&s.repo), &lines(&["ours"]), &mut |n| {
            raced.push(n);
            if n == 1 {
                publish(&other, &common(&other), &lines(&["theirs"])).unwrap();
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
        let sent = publish_with(&s.repo, &c, &lines(&["a", "b"]), &mut |n| {
            tries = n;
            publish(&other, &common(&other), &lines(&[&format!("theirs-{n}")])).unwrap();
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

        let sent = publish(&s.repo, &c, &lines(&["c"])).unwrap();
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
        let sent = publish(&repo, &c, &lines(&["one"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        let sent = publish(&repo, &c, &lines(&["two"])).unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 2, .. }), "{sent:?}");
        assert_eq!(fs::read_to_string(outbox(&c)).unwrap(), "one\ntwo\n");

        let remote = tmp.path().join("remote.git");
        g(tmp.path(), &["init", "-q", "--bare", "remote.git"]);
        g(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        assert_eq!(
            publish(&repo, &c, &[]).unwrap(),
            Sent::Pushed {
                records: 2,
                tries: 1
            }
        );
        assert_eq!(tracked(&repo).unwrap(), "one\ntwo\n");
    }

    #[test]
    fn a_push_a_hook_declines_goes_to_the_outbox_without_retrying() {
        let s = setup();
        let hook = s.remote.join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\necho no >&2\nexit 1\n").unwrap();
        Command::new("chmod").arg("+x").arg(&hook).status().unwrap();
        let mut pushes = 0;
        let sent = publish_with(&s.repo, &common(&s.repo), &lines(&["x"]), &mut |_| {
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
        let lock = common(&s.repo).join(format!("{TRACKING}.lock"));
        fs::create_dir_all(lock.parent().unwrap()).unwrap();
        fs::write(&lock, "").unwrap();
        let sent = publish(&s.repo, &common(&s.repo), &lines(&["x"])).unwrap();
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
        let env = |c: &Command, k: &str| {
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
            assert_eq!(env(&c, "GIT_SSH_COMMAND").as_deref(), Some(SSH));
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
        let sent = publish_with(&s.repo, &common(&s.repo), &lines(&["x"]), &mut |_| {
            pushes += 1
        })
        .unwrap();
        assert!(matches!(sent, Sent::Outbox { records: 1, .. }), "{sent:?}");
        assert_eq!(pushes, 0);
    }

    #[test]
    fn fetch_tells_a_missing_branch_from_an_unreachable_origin_and_drops_a_stale_ref() {
        let s = setup();
        assert_eq!(fetch(&s.repo).unwrap(), None);
        publish(&s.repo, &common(&s.repo), &lines(&["x"])).unwrap();
        let other = clone(&s.base, &s.remote, "other");
        let tip = fetch(&other).unwrap().unwrap();
        assert_eq!(g(&other, &["rev-parse", TRACKING]), tip);
        assert_eq!(tracked(&other).unwrap(), "x\n");
        g(
            &s.remote,
            &["update-ref", "-d", &format!("refs/heads/{BRANCH}")],
        );
        assert_eq!(fetch(&other).unwrap(), None);
        assert_eq!(tracked(&other).unwrap(), "");
        g(&other, &["remote", "remove", "origin"]);
        assert!(fetch(&other).is_err());
    }

    #[test]
    fn without_an_identity_the_commit_is_by_nightshift() {
        let mut c = Command::new("true");
        fallback_identity(&mut c, true);
        assert_eq!(c.get_envs().count(), 0);
        fallback_identity(&mut c, false);
        let envs: Vec<_> = c
            .get_envs()
            .map(|(k, v)| (k.to_str().unwrap(), v.unwrap().to_str().unwrap()))
            .collect();
        assert_eq!(envs.len(), 4);
        assert!(envs.contains(&("GIT_COMMITTER_EMAIL", "nightshift@localhost")));
    }
}
