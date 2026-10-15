//! One trial's sandbox: scratch repo, throwaway HOME, the harness process and its timeout.

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};

use crate::error::{SfError, EXIT_HARNESS_MISSING};
use crate::install::symlink;
use crate::proc::Proc;

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
pub fn git_cmd(dir: &Path) -> crate::proc::Proc {
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
    File::open(path)
        .and_then(|mut f| tail_window(&mut f, max))
        .unwrap_or_default()
}

/// The last `max` bytes of `r`, decoded lossily and trimmed, read without touching the bytes before them.
fn tail_window(r: &mut (impl Read + Seek), max: usize) -> std::io::Result<String> {
    let len = r.seek(SeekFrom::End(0))?;
    r.seek(SeekFrom::Start(len.saturating_sub(max as u64)))?;
    let mut buf = Vec::new();
    r.read_to_end(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).trim().to_string())
}

fn sandbox_env(cmd: &mut Proc, scratch: &Scratch) {
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
    let mut cmd = Proc::tool("sh");
    cmd.arg("-c").arg(script).current_dir(cwd).output_to(out);
    sandbox_env(&mut cmd, scratch);
    let ended = cmd.timeout(timeout).run().context("cannot run sh")?;
    Ok(ProcResult {
        exit: ended.status.and_then(|s| s.code()),
        timed_out: ended.timed_out(),
        wall_s: ended.wall_s,
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
    let mut cmd = crate::harness::command(bin, args, subscription)?;
    cmd.current_dir(&scratch.repo).output_to(out).stderr_to(err);
    sandbox_env(&mut cmd, scratch);
    let ended = match cmd.stdin(prompt.as_bytes().to_vec()).timeout(timeout).run() {
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
        exit: ended.status.and_then(|s| s.code()),
        timed_out: ended.timed_out(),
        wall_s: ended.wall_s,
        tail: tail_of(stderr_path, 2000),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_guard() {
        assert!(ensure_scratch(&std::env::temp_dir()).is_err());
        assert!(ensure_scratch(Path::new("/")).is_err());
        let t = new_root().unwrap();
        assert!(ensure_scratch(t.path()).is_ok());
    }

    struct Counted {
        inner: std::io::Cursor<Vec<u8>>,
        read: usize,
    }

    impl Read for Counted {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let n = self.inner.read(out)?;
            self.read += n;
            Ok(n)
        }
    }

    impl Seek for Counted {
        fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
            self.inner.seek(pos)
        }
    }

    #[test]
    fn tail_window_reads_only_the_last_max_bytes() {
        let mut r = Counted {
            inner: std::io::Cursor::new(b"0123456789".repeat(1000)),
            read: 0,
        };
        assert_eq!(tail_window(&mut r, 15).unwrap(), "567890123456789");
        assert_eq!(r.read, 15);
    }

    #[test]
    fn tail_of_returns_the_raw_byte_window() {
        let t = new_root().unwrap();
        let p = t.path().join("log");

        fs::write(&p, format!("first line\n{}\n", "x".repeat(30))).unwrap();
        assert_eq!(tail_of(&p, 11), "xxxxxxxxxx", "window starts mid-line");

        fs::write(&p, "  short log \n").unwrap();
        assert_eq!(tail_of(&p, 2000), "short log");

        fs::write(&p, "").unwrap();
        assert_eq!(tail_of(&p, 2000), "");

        fs::write(&p, "abc").unwrap();
        assert_eq!(tail_of(&p, 0), "");

        fs::write(&p, "a\u{e9}").unwrap();
        assert_eq!(
            tail_of(&p, 1),
            "\u{fffd}",
            "a cut multi-byte char decodes lossily"
        );

        assert_eq!(tail_of(&t.path().join("missing"), 2000), "");
        assert_eq!(tail_of(t.path(), 2000), "", "a directory is unreadable");
    }
}
