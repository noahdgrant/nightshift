//! `sf ask --role <role>`: run the configured harness headless with the prompt on stdin.

use std::io::{IsTerminal, Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::json;

use crate::config;
use crate::error::{SfError, EXIT_HARNESS_MISSING, EXIT_NOT_CONFIGURED, EXIT_NO_WRITE_COMMAND};
use crate::which::which;

fn not_configured(msg: String, path: &Path) -> anyhow::Error {
    SfError::new(EXIT_NOT_CONFIGURED, msg)
        .hint(format!(
            "add a role or a default to {}, for example:\n\n{}\n\n  check with: sf doctor",
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
                "sf ask --role build --write --cwd ../myrepo.worktrees/142-uart-timeout --prompt-file prompt.md",
            )
            .into());
        }
    }
    if role.is_empty() {
        return Err(SfError::usage(
            "--role must not be empty",
            "sf ask --role review.security --prompt-file prompt.md",
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

    if dry_run {
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
            "config": path.to_string_lossy(),
            "dry_run": true,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    let Some(bin) = bin else {
        return Err(SfError::new(
            EXIT_HARNESS_MISSING,
            format!(
                "harness {:?} needs `{}`, which is not on PATH",
                resolved.harness, command[0]
            ),
        )
        .hint("install it, or map the role to another harness; see installed harnesses with:\n  sf doctor")
        .into());
    };

    let prompt = read_prompt(prompt_file)?;
    if prompt.iter().all(u8::is_ascii_whitespace) {
        return Err(SfError::usage(
            "the prompt is empty",
            "sf ask --role review --prompt-file prompt.md",
        )
        .into());
    }

    let mut cmd = Command::new(&bin);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd
        .args(&command[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("cannot start {}", bin.display()))?;
    {
        let mut stdin = child.stdin.take().expect("stdin is piped");
        // A harness may exit before reading everything; its exit status tells the story.
        let _ = stdin.write_all(&prompt);
    }
    let status = child.wait().context("waiting for harness")?;
    if !status.success() {
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "a signal".to_string());
        return Err(SfError::general(format!(
            "harness {:?} (role {:?}, matched {:?}) exited with {code}: {}",
            resolved.harness,
            role,
            resolved.matched,
            command.join(" ")
        ))
        .hint(format!(
            "preview the command with:\n  sf ask --role {role} --dry-run"
        ))
        .into());
    }
    Ok(())
}

fn read_prompt(prompt_file: Option<&Path>) -> Result<Vec<u8>> {
    match prompt_file {
        Some(p) => std::fs::read(p).map_err(|e| {
            SfError::usage(
                format!("cannot read prompt file {}: {e}", p.display()),
                "sf ask --role review --prompt-file prompt.md",
            )
            .into()
        }),
        None => {
            let mut stdin = std::io::stdin();
            if stdin.is_terminal() {
                return Err(SfError::usage(
                    "no prompt: pipe it on stdin or pass --prompt-file",
                    "sf ask --role review --prompt-file prompt.md\n  echo 'Review this diff' | sf ask --role review",
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
