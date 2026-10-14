//! `ns ask --role <role>`: run the configured harness headless with the prompt on stdin.

use std::fs::File;
use std::io::{IsTerminal, Read};
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

use crate::error::{SfError, EXIT_HARNESS_MISSING, EXIT_NOT_CONFIGURED, EXIT_NO_WRITE_COMMAND};
use crate::proc::Proc;
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
    plan.exec(&bin, &prompt, cwd, Some(dup(std::io::stdout())?))?;
    Ok(())
}

/// Run `role` read-only, as `ns ask --role <role>` does, and return what it printed.
pub fn answer(role: &str, prompt: &str) -> Result<String> {
    let plan = plan(role, false)?;
    let bin = plan.bin()?;
    let out = plan.exec(&bin, prompt.as_bytes(), None, None)?;
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
    let command = with_phase_settings(command)?;
    Ok(Plan {
        resolved,
        command,
        bin,
        config: path,
    })
}

/// Inside an `ns run` phase (`NS_WORKTREE` and `NS_UNIT` set), a claude command gets the
/// phase's settings and settings sources, like the phase. A command that carries either flag
/// itself, or a phase whose settings file (`NS_PHASE_SETTINGS`) is gone, is an error. The
/// variables are the phase's own, so this is cooperative: a phase that clears them asks claude
/// with the user's settings.
fn with_phase_settings(mut command: Vec<String>) -> Result<Vec<String>> {
    let (Some(wt), Some(unit)) = (
        std::env::var_os("NS_WORKTREE"),
        std::env::var("NS_UNIT").ok(),
    ) else {
        return Ok(command);
    };
    if !billing::is_claude(&command) {
        return Ok(command);
    }
    crate::worktree::check_unit_id(&unit, "ns run 142-uart-timeout --issue 142")?;
    let settings = std::env::var_os(crate::pin::SETTINGS_VAR)
        .map(PathBuf::from)
        .filter(|s| s.is_file());
    let Some(settings) = settings else {
        return Err(SfError::usage(
            "the phase settings file (NS_PHASE_SETTINGS) is missing",
            "ns run writes it before each phase; ask from a phase that ns run started",
        )
        .into());
    };
    crate::pin::apply(&mut command, &settings, Path::new(&wt), &unit, "ns ask")?;
    Ok(command)
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

    /// Run the harness with `prompt` on stdin and its stderr on ours. Returns its stdout, or
    /// sends it to `stdout` as it writes and returns nothing.
    fn exec(
        &self,
        bin: &Path,
        prompt: &[u8],
        cwd: Option<&Path>,
        stdout: Option<File>,
    ) -> Result<Vec<u8>> {
        let (resolved, command) = (&self.resolved, &self.command);
        let mut cmd = Proc::harness(bin, subscription(cwd)?);
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        if let Some(out) = stdout {
            cmd.output_to(out);
        }
        let out = cmd
            .args(&command[1..])
            .stdin(prompt.to_vec())
            .stderr_to(dup(std::io::stderr())?)
            .output()
            .with_context(|| format!("cannot run {}", bin.display()))?;
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

/// A file on the same open file as `fd`, for a child to write to as this process would.
fn dup(fd: impl AsFd) -> Result<File> {
    Ok(File::from(
        fd.as_fd()
            .try_clone_to_owned()
            .context("cannot duplicate a standard stream")?,
    ))
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
