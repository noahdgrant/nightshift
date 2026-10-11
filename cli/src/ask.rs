//! `ns ask --role <role>`: run the configured harness headless with the prompt on stdin.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::json;

use crate::error::{SfError, EXIT_HARNESS_MISSING, EXIT_NOT_CONFIGURED, EXIT_NO_WRITE_COMMAND};
use crate::which::which;
use crate::{billing, config, factory, git};

fn not_configured(msg: String, path: &Path) -> anyhow::Error {
    SfError::new(EXIT_NOT_CONFIGURED, msg)
        .hint(format!(
            "add a role or a default to {}, for example:\n\n{}\n\n  check with: ns doctor",
            path.display(),
            config::EXAMPLE
        ))
        .into()
}

pub struct AskArgs<'a> {
    pub role: &'a str,
    pub prompt_file: Option<&'a Path>,
    pub write: bool,
    pub cwd: Option<&'a Path>,
    pub dry_run: bool,
}

pub fn run(args: AskArgs<'_>) -> Result<()> {
    let AskArgs {
        role,
        prompt_file,
        write,
        cwd,
        dry_run,
    } = args;
    if let Some(dir) = cwd {
        if !dir.is_dir() {
            return Err(SfError::usage(
                format!("--cwd {} is not a directory", dir.display()),
                "ns ask --role build --write --cwd ../myrepo.worktrees/142-uart-timeout --prompt-file prompt.md",
            )
            .into());
        }
    }
    let plan = plan(role, write)?;

    if dry_run {
        let Plan {
            resolved,
            command,
            bin,
            config,
        } = &plan;
        let out = json!({
            "role": resolved.requested,
            "matched_role": resolved.matched,
            "harness": resolved.harness,
            "model": resolved.model,
            "mode": if write { "write" } else { "read-only" },
            "command": command,
            "cwd": match cwd {
                Some(d) => d.to_string_lossy().into_owned(),
                None => std::env::current_dir()?.to_string_lossy().into_owned(),
            },
            "binary": bin.as_ref().map(|p| p.to_string_lossy().into_owned()),
            "on_path": bin.is_some(),
            "prompt": match prompt_file {
                Some(p) => p.to_string_lossy().into_owned(),
                None => "stdin".to_string(),
            },
            "config": config.to_string_lossy(),
            "dry_run": true,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let bin = plan.bin()?;
    let prompt = read_prompt(prompt_file)?;
    if prompt.iter().all(u8::is_ascii_whitespace) {
        return Err(SfError::usage(
            "the prompt is empty",
            "ns ask --role review --prompt-file prompt.md",
        )
        .into());
    }
    plan.exec(&bin, &prompt, cwd, Stdio::inherit())?;
    Ok(())
}

/// Run `role` read-only, as `ns ask --role <role>` does, and return what it printed.
pub fn answer(role: &str, prompt: &str) -> Result<String> {
    let plan = plan(role, false)?;
    let bin = plan.bin()?;
    let out = plan.exec(&bin, prompt.as_bytes(), None, Stdio::piped())?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// The command `ns ask` runs for a role, from the user config.
struct Plan {
    resolved: config::Resolved,
    command: Vec<String>,
    /// `command[0]` on PATH, if it is there.
    bin: Option<PathBuf>,
    config: PathBuf,
}

fn plan(role: &str, write: bool) -> Result<Plan> {
    if role.is_empty() {
        return Err(SfError::usage(
            "--role must not be empty",
            "ns ask --role review.security --prompt-file prompt.md",
        )
        .into());
    }
    let path = config::path();
    let cfg = match config::load(&path) {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Err(not_configured(
                format!("no config file at {}", path.display()),
                &path,
            ))
        }
        Err(e) => {
            return Err(SfError::new(EXIT_NOT_CONFIGURED, format!("{e:#}"))
                .hint(format!(
                    "fix the file; a valid example:\n\n{}",
                    config::EXAMPLE
                ))
                .into())
        }
    };
    let chain = config::role_chain(role).join(" -> ");
    let Some(resolved) = cfg.resolve(role) else {
        return Err(not_configured(
            format!("role {role:?} is not configured (looked up {chain})"),
            &path,
        ));
    };
    if write && resolved.command.is_some() && resolved.command_write.is_none() {
        return Err(SfError::new(
            EXIT_NO_WRITE_COMMAND,
            format!(
                "--write needs a command_write for harness {:?} (role {:?}), and none is configured",
                resolved.harness, resolved.matched
            ),
        )
        .hint(format!(
            "add one to {}, for example:\n\n[harness.{}]\ncommand = [...]\ncommand_write = [\"my-harness\", \"--allow-edits\", \"--model\", \"{{model}}\"]\n\nor run read-only by dropping --write",
            path.display(),
            resolved.harness
        ))
        .into());
    }
    let chosen = if write {
        resolved.command_write.clone()
    } else {
        resolved.command.clone()
    };
    let Some(command) = chosen.filter(|c| !c.is_empty()) else {
        return Err(not_configured(
            format!(
                "role {:?} uses harness {:?}, which has no command; built-ins are claude and codex",
                resolved.matched, resolved.harness
            ),
            &path,
        ));
    };
    let bin = which(&command[0]);
    Ok(Plan {
        resolved,
        command,
        bin,
        config: path,
    })
}

impl Plan {
    fn bin(&self) -> Result<PathBuf> {
        self.bin.clone().ok_or_else(|| {
            SfError::new(
                EXIT_HARNESS_MISSING,
                format!(
                    "harness {:?} needs `{}`, which is not on PATH",
                    self.resolved.harness, self.command[0]
                ),
            )
            .hint("install it, or map the role to another harness; see installed harnesses with:\n  ns doctor")
            .into()
        })
    }

    /// Run the harness with `prompt` on stdin. Returns its stdout when `stdout` is piped.
    fn exec(
        &self,
        bin: &Path,
        prompt: &[u8],
        cwd: Option<&Path>,
        stdout: Stdio,
    ) -> Result<Vec<u8>> {
        let (resolved, command) = (&self.resolved, &self.command);
        let mut cmd = Command::new(bin);
        git::scrub(&mut cmd);
        if subscription(cwd)? {
            billing::scrub(&mut cmd, command);
        }
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd
            .args(&command[1..])
            .stdin(Stdio::piped())
            .stdout(stdout)
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("cannot start {}", bin.display()))?;
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // Write from a thread, so a harness that answers before reading all of a long prompt
        // can't deadlock against a full stdout pipe. A harness may exit before reading
        // everything; its exit status tells the story.
        let prompt = prompt.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&prompt);
        });
        let out = child.wait_with_output().context("waiting for harness")?;
        let _ = writer.join();
        if !out.status.success() {
            let code = out
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "a signal".to_string());
            let role = &resolved.requested;
            return Err(SfError::general(format!(
                "harness {:?} (role {:?}, matched {:?}) exited with {code}: {}",
                resolved.harness,
                role,
                resolved.matched,
                command.join(" ")
            ))
            .hint(format!(
                "preview the command with:\n  ns ask --role {role} --dry-run"
            ))
            .into());
        }
        Ok(out.stdout)
    }
}

/// The factory's billing mode for the repo holding `cwd` (default: the current directory).
/// Without a main checkout to read (no repo, a bare repo), or with no factory file, billing is
/// `subscription`.
fn subscription(cwd: Option<&Path>) -> Result<bool> {
    let start = match cwd {
        Some(d) => d.to_path_buf(),
        None => std::env::current_dir().context("cannot read current directory")?,
    };
    let Ok(repo) = git::Repo::discover(&start) else {
        return Ok(true);
    };
    Ok(factory::load(&factory::root(None, &repo.root))?.subscription())
}

fn read_prompt(prompt_file: Option<&Path>) -> Result<Vec<u8>> {
    match prompt_file {
        Some(p) => std::fs::read(p).map_err(|e| {
            SfError::usage(
                format!("cannot read prompt file {}: {e}", p.display()),
                "ns ask --role review --prompt-file prompt.md",
            )
            .into()
        }),
        None => {
            let mut stdin = std::io::stdin();
            if stdin.is_terminal() {
                return Err(SfError::usage(
                    "no prompt: pipe it on stdin or pass --prompt-file",
                    "ns ask --role review --prompt-file prompt.md\n  echo 'Review this diff' | ns ask --role review",
                )
                .into());
            }
            let mut buf = Vec::new();
            stdin
                .read_to_end(&mut buf)
                .context("reading prompt from stdin")?;
            Ok(buf)
        }
    }
}
