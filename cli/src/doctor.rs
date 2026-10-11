//! `ns doctor`: config, harness availability, harness skill dirs and forge credential report.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::Result;
use clap::ValueEnum;
use serde_json::{json, Map, Value};

use crate::config::{self, Forge, ForgeKind};
use crate::forge;
use crate::git;
use crate::install;
use crate::which::which;

pub const KNOWN_HARNESSES: &[&str] = &["claude", "codex", "cursor-agent", "gemini", "opencode"];

pub fn run() -> Result<()> {
    let path = config::path();
    let exists = path.is_file();
    let (parses, error, cfg) = match config::load(&path) {
        Ok(Some(c)) => (true, None, Some(c)),
        Ok(None) => (false, None, None),
        Err(e) => (false, Some(format!("{e:#}")), None),
    };

    let mut harnesses = Map::new();
    for h in KNOWN_HARNESSES {
        let p = which(h);
        harnesses.insert(
            h.to_string(),
            json!({ "on_path": p.is_some(), "path": p.map(|p| p.to_string_lossy().into_owned()) }),
        );
    }

    let mut skill_dirs = Map::new();
    for h in install::Harness::ALL {
        let dir = h.dir();
        let ns_skills = install::skill_dirs(&dir).map_or(0, |s| s.len());
        skill_dirs.insert(
            h.to_possible_value()
                .expect("no skipped variants")
                .get_name()
                .to_string(),
            json!({ "path": dir.to_string_lossy(), "ns_skills": ns_skills }),
        );
    }

    let mut roles = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    if let Some(cfg) = &cfg {
        for (name, role) in &cfg.roles {
            let resolved = cfg.resolve(name).expect("role exists");
            let bin = resolved.command.as_ref().and_then(|c| c.first().cloned());
            let on_path = bin.as_deref().and_then(which).is_some();
            match &bin {
                None => problems.push(format!(
                    "role {name:?}: harness {:?} has no command",
                    role.harness
                )),
                Some(b) if !on_path => {
                    problems.push(format!("role {name:?}: `{b}` is not on PATH"))
                }
                _ => {}
            }
            roles.push(json!({
                "name": name,
                "harness": role.harness,
                "model": role.model,
                "command": resolved.command,
                "command_write": resolved.command_write,
                "on_path": on_path,
            }));
        }
        if !cfg.roles.contains_key("default") {
            problems
                .push("no [roles.default]; roles without a match exit 3 from ns ask".to_string());
        }
    } else if !exists {
        problems.push(format!(
            "no config at {}; ns ask exits 3 until one exists (see ns ask --help)",
            path.display()
        ));
    }

    let (forge, warnings) = forges_entry(cfg.as_ref(), &mut problems);
    let hooks = hooks_entry(&mut problems);

    let out = json!({
        "ok": parses && problems.is_empty(),
        "config": {
            "path": path.to_string_lossy(),
            "exists": exists,
            "parses": parses,
            "error": error,
        },
        "roles": roles,
        "distinct_harnesses": cfg
            .as_ref()
            .map(|c| c.roles.values().map(|r| r.harness.clone()).collect::<std::collections::BTreeSet<_>>())
            .unwrap_or_default(),
        "harnesses": Value::Object(harnesses),
        "skill_dirs": Value::Object(skill_dirs),
        "claude_phases": {
            "auto_memory": false,
            "env": format!("{}=1", crate::billing::AUTO_MEMORY_ENV),
        },
        "forge": forge,
        "hooks": hooks,
        "problems": problems,
        "warnings": warnings,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Whether the current repo's `core.hooksPath` points at its `.githooks/`. A repo without
/// `.githooks/`, or no repo at all, has nothing to point at and passes.
fn hooks_entry(problems: &mut Vec<String>) -> Value {
    const FIX: &str = "git config core.hooksPath .githooks";
    let Some(root) = git::run(Path::new("."), &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
        .filter(|r| r.join(".githooks").is_dir())
    else {
        return json!({ "ok": true, "path": null, "detail": "no .githooks/ in this repo", "fix": null });
    };
    let read = git::command()
        .arg("-C")
        .arg(&root)
        .args(["config", "core.hooksPath"])
        .output();
    let (path, failure) = match read {
        Ok(o) if o.status.success() => (
            Some(String::from_utf8_lossy(&o.stdout).trim_end().to_string()),
            None,
        ),
        Ok(o) if o.status.code() == Some(1) => (None, None),
        Ok(o) => (
            None,
            Some(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        ),
        Err(e) => (None, Some(e.to_string())),
    };
    let ok = path.as_deref().is_some_and(|p| {
        root.join(p).canonicalize().ok() == root.join(".githooks").canonicalize().ok()
    });
    let detail = match &path {
        _ if ok => "core.hooksPath points at .githooks".to_string(),
        None if failure.is_some() => format!(
            "could not read core.hooksPath: {}",
            failure.as_deref().unwrap_or_default()
        ),
        None => "core.hooksPath is unset, so .githooks/ never runs".to_string(),
        Some(p) => format!("core.hooksPath is {p:?}, so .githooks/ never runs"),
    };
    if !ok {
        problems.push(format!("{detail}; fix: {FIX}"));
    }
    json!({ "ok": ok, "path": path, "detail": detail, "fix": (!ok).then_some(FIX) })
}

/// `[queue] source` from the current repo's factory file, when it has one that loads.
fn queue_source() -> Option<String> {
    let repo = crate::git::Repo::discover(Path::new(".")).ok()?;
    crate::factory::load(&crate::factory::root(None, &repo.root))
        .ok()?
        .queue
        .source
}

/// Each forge's entry, and the warnings they raise, also printed to stderr.
fn forges_entry(cfg: Option<&config::Config>, problems: &mut Vec<String>) -> (Value, Vec<String>) {
    let source = queue_source();
    let origin = forge::Origin::read(Path::new("."), source.as_deref(), cfg);
    let mut warnings: Vec<String> = Vec::new();
    let mut forge = Map::new();
    for (kind, f) in origin.forges.each() {
        let entry = forge_entry(kind, f, origin.url_for(kind), problems, &mut warnings);
        forge.insert(kind.name().to_string(), entry);
    }
    for w in &warnings {
        eprintln!("ns: warning: {w}");
    }
    (Value::Object(forge), warnings)
}

/// One forge's entry. `origin` is set for the forge `origin` is on: then the entry also names the
/// repo, whether the token can read it, and the account `ssh -T` greets.
fn forge_entry(
    kind: ForgeKind,
    f: Option<&Forge>,
    origin: Option<&str>,
    problems: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> Value {
    let repo = origin.and_then(forge::repo_path);
    let repo = repo.as_deref();
    if !forge::inherited(kind, repo) {
        warnings.extend(forge::env_warning(kind, f, repo));
    }
    let creds = match f.map(|f| forge::resolve(kind, f, repo)).transpose() {
        Ok(c) => c,
        Err(e) => {
            problems.push(e.message);
            None
        }
    };
    let token_resolved = match f {
        Some(_) => creds.is_some(),
        None => forge::is_set(kind.token_var()),
    };
    let account = token_resolved
        .then(|| forge::account(kind, creds.as_ref()))
        .flatten();
    let readable = repo.filter(|_| f.is_none() || creds.is_some()).map(|r| {
        forge::can_read(kind, r, creds.as_ref()).map_err(|e| {
            if f.is_some() {
                let source = forge::source(kind, f, repo);
                problems.push(forge::unreadable(kind, r, account.as_deref(), &source, &e));
            }
        })
    });
    let ssh = origin.and_then(forge::ssh_target).map(|t| {
        let greeted = ssh_account(&t);
        match (&greeted, &account) {
            (None, _) => warnings.push(format!("ssh -T {} greeted no account", t.dest)),
            (Some(s), Some(a)) if !s.eq_ignore_ascii_case(a) => warnings.push(format!(
                "ssh to {} authenticates as {s}, but the {} token is account {a}: git and the forge API act as different accounts",
                t.dest,
                kind.name()
            )),
            _ => {}
        }
        json!({ "dest": t.dest, "account": greeted })
    });
    json!({
        "configured": f.is_some(),
        "token_resolved": token_resolved,
        "account": account,
        "table": f.map(|f| f.choose(repo).1.label(kind)),
        "repo": repo,
        "repo_readable": readable.map(|r| r.is_ok()),
        "ssh": ssh,
    })
}

/// The account `ssh -T` greets on `t`: GitHub's `Hi <account>!` or GitLab's `Welcome to
/// GitLab, @<account>!`. Batch mode, so a missing key or unknown host fails instead of asking.
fn ssh_account(t: &forge::SshTarget) -> Option<String> {
    ssh_account_within(OsStr::new("ssh"), t, forge::PROBE_TIMEOUT)
}

fn ssh_account_within(program: &OsStr, t: &forge::SshTarget, limit: Duration) -> Option<String> {
    let mut cmd = Command::new(program);
    cmd.args(["-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10"]);
    if let Some(p) = &t.port {
        cmd.args(["-p", p]);
    }
    cmd.arg("--").arg(&t.dest).stdin(Stdio::null());
    let out = forge::output_within(cmd, limit).ok()?;
    let text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    greeted(&text)
}

fn greeted(text: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.trim();
        let rest = l
            .strip_prefix("Hi ")
            .or_else(|| l.split_once(", @").map(|(_, r)| r))?;
        let name = rest.split_once('!')?.0;
        (!name.is_empty()).then(|| name.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::{greeted, ssh_account_within};
    use crate::forge::SshTarget;
    use std::process::Command;
    use std::time::{Duration, Instant};

    #[test]
    fn ssh_account_gives_up_on_a_host_that_stalls() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("ssh");
        let made = Command::new("sh")
            .arg("-c")
            .arg(r#"printf '%s\n' '#!/bin/sh' 'exec sleep 30' > "$0" && chmod 755 "$0""#)
            .arg(&fake)
            .status()
            .unwrap();
        assert!(made.success());
        let t = SshTarget {
            dest: "git@example".into(),
            port: None,
        };
        let limit = Duration::from_millis(300);
        let started = Instant::now();
        let got = ssh_account_within(fake.as_os_str(), &t, limit);
        let elapsed = started.elapsed();
        assert_eq!(got, None);
        assert!(elapsed >= limit, "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(10), "{elapsed:?}");
    }

    #[test]
    fn greeted_reads_github_and_gitlab_greetings() {
        assert_eq!(
            greeted("Hi work-account! You've successfully authenticated, but GitHub does not provide shell access.\n").as_deref(),
            Some("work-account")
        );
        assert_eq!(
            greeted("Welcome to GitLab, @someone!\n").as_deref(),
            Some("someone")
        );
        assert_eq!(
            greeted("git@github.com: Permission denied (publickey).\n"),
            None
        );
        assert_eq!(greeted("Hi !\n"), None);
        assert_eq!(greeted(""), None);
    }
}
