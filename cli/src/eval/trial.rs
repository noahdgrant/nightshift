//! One trial's sandbox: scratch repo, throwaway HOME, the harness process and its timeout.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::error::{SfError, EXIT_HARNESS_MISSING};

use super::spec::Case;

/// Variables that would point the harness at the user's real config instead of the throwaway home.
const SCRUBBED_ENV: &[&str] = &[
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "NS_CONFIG",
];

/// Names inside the scratch root that case files must not clobber.
const RESERVED: &[&str] = &["repo", "home", "base", "logs"];

pub struct Scratch {
    /// Removed on drop.
    pub root: tempfile::TempDir,
    /// The harness's working directory.
    pub repo: PathBuf,
    /// The throwaway `$HOME`.
    pub home: PathBuf,
    /// The commit the agent starts from (after overlay and setup). `None` for trigger runs.
    pub start: Option<String>,
    pub env: Vec<(String, String)>,
}

impl Scratch {
    pub fn root(&self) -> &Path {
        self.root.path()
    }
}

/// Refuse to run anything outside a scratch dir under the system temp dir.
pub fn ensure_scratch(dir: &Path) -> Result<()> {
    let tmp = std::env::temp_dir()
        .canonicalize()
        .context("cannot resolve the system temp dir")?;
    let dir = dir
        .canonicalize()
        .with_context(|| format!("cannot resolve {}", dir.display()))?;
    if dir == tmp || !dir.starts_with(&tmp) {
        return Err(SfError::general(format!(
            "refusing to run a harness in {}: not a scratch dir under {}",
            dir.display(),
            tmp.display()
        ))
        .into());
    }
    Ok(())
}

fn new_root() -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix("ns-eval-")
        .tempdir()
        .context("cannot create a scratch dir")
}

/// Copy `src` into `dst` recursively. Symlinks are recreated, not followed.
pub fn copy_dir(src: &Path, dst: &Path, skip: &dyn Fn(&Path) -> bool) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("cannot create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("cannot read {}", src.display()))? {
        let entry = entry?;
        let from = entry.path();
        if skip(&from) {
            continue;
        }
        let to = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            let target = fs::read_link(&from)?;
            let _ = fs::remove_file(&to);
            symlink(&target, &to)?;
        } else if ft.is_dir() {
            copy_dir(&from, &to, &|_| false)?;
        } else {
            fs::copy(&from, &to)
                .with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("cannot link {} -> {}", link.display(), target.display()))
}

#[cfg(not(unix))]
fn symlink(_target: &Path, link: &Path) -> Result<()> {
    anyhow::bail!("symlinks are not supported here: {}", link.display())
}

/// Throwaway home with `skills` copied into `.claude/skills` and `.agents/skills`, and the
/// `carry` paths symlinked from the real home.
fn make_home(home: &Path, skills: &[PathBuf], carry: &[String]) -> Result<()> {
    for sub in [".claude/skills", ".agents/skills"] {
        let dir = home.join(sub);
        fs::create_dir_all(&dir)?;
        for s in skills {
            let name = s.file_name().context("skill dir has no name")?;
            copy_dir(s, &dir.join(name), &|_| false)?;
        }
    }
    let real = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    for c in carry {
        let rel = Path::new(c);
        if rel.is_absolute() || c.split('/').any(|p| p == "..") {
            continue;
        }
        let src = real.join(rel);
        if !src.exists() {
            continue;
        }
        let dst = home.join(rel);
        if let Some(p) = dst.parent() {
            fs::create_dir_all(p)?;
        }
        symlink(&src, &dst)?;
    }
    Ok(())
}

/// A `git` command for scratch repos: fixed identity, no user or system config.
pub fn git_cmd(dir: &Path) -> Command {
    let mut c = crate::git::command();
    c.arg("-C")
        .arg(dir)
        .args(["-c", "core.quotepath=off", "-c", "commit.gpgsign=false"])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "ns-eval")
        .env("GIT_AUTHOR_EMAIL", "ns-eval@localhost")
        .env("GIT_COMMITTER_NAME", "ns-eval")
        .env("GIT_COMMITTER_EMAIL", "ns-eval@localhost");
    c
}

pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = git_cmd(dir)
        .args(args)
        .output()
        .context("failed to run git")?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Scratch repo for a case: fixture copy, base commit, overlay and setup, start commit.
pub fn prepare_case(
    fixture: &Path,
    case: &Case,
    skills: &[PathBuf],
    carry: &[String],
    env: Vec<(String, String)>,
) -> Result<Scratch> {
    let root = new_root()?;
    let repo = root.path().join("repo");
    let home = root.path().join("home");
    copy_dir(fixture, &repo, &|p| {
        p.file_name()
            .is_some_and(|n| n == "fixture.toml" || n == ".git")
    })?;
    make_home(&home, skills, carry)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["config", "user.name", "ns-eval"])?;
    git(&repo, &["config", "user.email", "ns-eval@localhost"])?;
    let info = repo.join(".git/info");
    fs::create_dir_all(&info)?;
    fs::write(info.join("exclude"), ".ns/\n")?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "fixture"])?;

    let overlay = case.dir.join("files");
    if overlay.is_dir() {
        copy_dir(&overlay, &repo, &|_| false)?;
    }
    // Case-dir files sit beside the repo during setup, so `cp ../brief.md ...` works.
    let mut staged = Vec::new();
    for e in fs::read_dir(&case.dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name == "files" || name == "case.toml" || RESERVED.contains(&name.as_str()) {
            continue;
        }
        let to = root.path().join(&name);
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &to, &|_| false)?;
        } else {
            fs::copy(e.path(), &to)?;
        }
        staged.push(to);
    }
    let mut scratch = Scratch {
        root,
        repo,
        home,
        start: None,
        env,
    };
    let logs = scratch.root().join("logs");
    fs::create_dir_all(&logs)?;
    for (i, c) in case.file.setup.commands.iter().enumerate() {
        let r = run_sh(
            c,
            &scratch,
            &scratch.repo,
            Duration::from_secs(600),
            &logs.join(format!("setup-{i}.log")),
        )?;
        if r.exit != Some(0) {
            anyhow::bail!(
                "setup command {c:?} {} in case {}: {}",
                r.describe(),
                case.id,
                r.tail
            );
        }
    }
    for p in staged {
        if p.is_dir() {
            fs::remove_dir_all(&p)?;
        } else {
            fs::remove_file(&p)?;
        }
    }
    git(&scratch.repo, &["add", "-A"])?;
    git(
        &scratch.repo,
        &["commit", "-q", "--allow-empty", "-m", "setup"],
    )?;
    scratch.start = Some(git(&scratch.repo, &["rev-parse", "HEAD"])?);
    Ok(scratch)
}

/// Empty scratch dir with the skills installed, for trigger runs.
pub fn prepare_empty(skills: &[PathBuf], carry: &[String]) -> Result<Scratch> {
    let root = new_root()?;
    let repo = root.path().join("repo");
    let home = root.path().join("home");
    fs::create_dir_all(&repo)?;
    make_home(&home, skills, carry)?;
    fs::create_dir_all(root.path().join("logs"))?;
    Ok(Scratch {
        root,
        repo,
        home,
        start: None,
        env: Vec::new(),
    })
}

pub struct ProcResult {
    pub exit: Option<i32>,
    pub timed_out: bool,
    pub wall_s: f64,
    /// The last few KB of output, for check details.
    pub tail: String,
}

impl ProcResult {
    pub fn describe(&self) -> String {
        match (self.timed_out, self.exit) {
            (true, _) => "timed out".into(),
            (false, Some(c)) => format!("exited {c}"),
            (false, None) => "was killed by a signal".into(),
        }
    }
}

fn tail_of(path: &Path, max: usize) -> String {
    let mut buf = Vec::new();
    if let Ok(mut f) = File::open(path) {
        let _ = f.read_to_end(&mut buf);
    }
    let start = buf.len().saturating_sub(max);
    String::from_utf8_lossy(&buf[start..]).trim().to_string()
}

/// Run `cmd` in its own session and process group, feeding `stdin`, killing the group at the
/// timeout. A `kill 0` or group kill inside the process stays out of the caller's group.
pub fn run_process(
    mut cmd: Command,
    stdin: Option<Vec<u8>>,
    timeout: Duration,
) -> std::io::Result<(Option<ExitStatus>, bool, f64)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: setsid(2) is async-signal-safe; the child is not yet a group leader, so it
        // gets a new session and a process group whose id is its pid.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let started = Instant::now();
    let mut child = cmd.spawn()?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(bytes), Some(mut pipe)) => Some(std::thread::spawn(move || {
            // A process may exit before reading everything; its exit status tells the story.
            let _ = pipe.write_all(&bytes);
        })),
        _ => None,
    };
    let deadline = started + timeout;
    let mut timed_out = false;
    let status = loop {
        if let Some(st) = child.try_wait()? {
            break Some(st);
        }
        if Instant::now() >= deadline {
            timed_out = true;
            kill_group(child.id());
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // Reap anything the process left running in its group.
    kill_group(child.id());
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok((status, timed_out, started.elapsed().as_secs_f64()))
}

#[cfg(unix)]
fn kill_group(pid: u32) {
    // SAFETY: kill(2) with a negative pid signals that process group; failures are ignored.
    unsafe {
        libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_group(_pid: u32) {}

fn sandbox_env(cmd: &mut Command, scratch: &Scratch) {
    for k in SCRUBBED_ENV {
        cmd.env_remove(k);
    }
    cmd.env("HOME", &scratch.home);
    for (k, v) in &scratch.env {
        cmd.env(k, v);
    }
}

/// `sh -c <script>` in `cwd` with the trial's environment; output goes to `log`.
pub fn run_sh(
    script: &str,
    scratch: &Scratch,
    cwd: &Path,
    timeout: Duration,
    log: &Path,
) -> Result<ProcResult> {
    let out = File::create(log).with_context(|| format!("cannot create {}", log.display()))?;
    let mut cmd = Command::new("sh");
    crate::git::scrub(&mut cmd);
    cmd.arg("-c")
        .arg(script)
        .current_dir(cwd)
        .stdout(out.try_clone()?)
        .stderr(out);
    sandbox_env(&mut cmd, scratch);
    let (status, timed_out, wall_s) = run_process(cmd, None, timeout).context("cannot run sh")?;
    Ok(ProcResult {
        exit: status.and_then(|s| s.code()),
        timed_out,
        wall_s,
        tail: tail_of(log, 2000),
    })
}

/// Run the harness with the prompt on stdin, in the scratch repo, with the throwaway home.
pub fn run_harness(
    argv: &[String],
    prompt: &str,
    scratch: &Scratch,
    timeout: Duration,
    stdout_path: &Path,
    stderr_path: &Path,
    subscription: bool,
) -> Result<ProcResult> {
    ensure_scratch(&scratch.repo)?;
    let (bin, args) = argv
        .split_first()
        .ok_or_else(|| SfError::general("the eval harness command is empty"))?;
    let out = File::create(stdout_path)
        .with_context(|| format!("cannot create {}", stdout_path.display()))?;
    let err = File::create(stderr_path)
        .with_context(|| format!("cannot create {}", stderr_path.display()))?;
    let mut cmd = Command::new(bin);
    crate::git::scrub(&mut cmd);
    cmd.args(args)
        .current_dir(&scratch.repo)
        .stdout(out)
        .stderr(err);
    sandbox_env(&mut cmd, scratch);
    if crate::billing::is_claude(argv) {
        crate::billing::wait_for_bg_tasks(&mut cmd);
        crate::billing::disable_auto_memory(&mut cmd);
        if subscription {
            crate::billing::scrub(&mut cmd);
        }
    }
    let (status, timed_out, wall_s) = match run_process(cmd, Some(prompt.as_bytes().to_vec()), timeout) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(SfError::new(
                EXIT_HARNESS_MISSING,
                format!("eval harness needs `{bin}`, which is not on PATH"),
            )
            .hint("install it, or point [eval] harness at another one; preview with:\n  ns eval --dry-run")
            .into())
        }
        Err(e) => return Err(e).with_context(|| format!("cannot start {bin}")),
    };
    Ok(ProcResult {
        exit: status.and_then(|s| s.code()),
        timed_out,
        wall_s,
        tail: tail_of(stderr_path, 2000),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_kills_the_process_group() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("survivor");
        let mut cmd = Command::new("sh");
        crate::git::scrub(&mut cmd);
        cmd.arg("-c")
            .arg(format!("(sleep 1; touch {}) & sleep 30", marker.display()));
        let (status, timed_out, wall) = run_process(cmd, None, Duration::from_millis(200)).unwrap();
        assert!(timed_out);
        assert!(status.is_none());
        assert!(wall < 5.0);
        std::thread::sleep(Duration::from_millis(1300));
        assert!(!marker.exists(), "background child survived the kill");
    }

    #[test]
    fn the_process_runs_in_its_own_session_and_group() {
        let tmp = tempfile::tempdir().unwrap();
        let ids = tmp.path().join("ids");
        let mut cmd = Command::new("sh");
        crate::git::scrub(&mut cmd);
        cmd.arg("-c")
            .arg("ps -o sid=,pgid= -p $$")
            .stdout(File::create(&ids).unwrap());
        let (status, _, _) = run_process(cmd, None, Duration::from_secs(30)).unwrap();
        assert!(status.unwrap().success());
        let text = std::fs::read_to_string(&ids).unwrap();
        let got: Vec<i32> = text
            .split_whitespace()
            .map(|n| n.parse().unwrap())
            .collect();
        // SAFETY: getsid and getpgrp only read this process's ids.
        let (sid, pgid) = unsafe { (libc::getsid(0), libc::getpgrp()) };
        assert_eq!(got.len(), 2, "{text}");
        assert_ne!(got[0], sid, "shares the caller's session");
        assert_ne!(got[1], pgid, "shares the caller's process group");
    }

    #[test]
    fn scratch_guard() {
        assert!(ensure_scratch(&std::env::temp_dir()).is_err());
        assert!(ensure_scratch(Path::new("/")).is_err());
        let t = new_root().unwrap();
        assert!(ensure_scratch(t.path()).is_ok());
    }
}
