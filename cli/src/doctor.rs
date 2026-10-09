//! `ns doctor`: config, harness availability and forge credential report.

use std::process::{Command, Stdio};

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::config;
use crate::forge;
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

    let forges = cfg.as_ref().map(|c| c.forge.clone()).unwrap_or_default();
    let mut forge = Map::new();
    for (name, var, host_var, f) in forges.each() {
        if let Some(f) = f {
            if let Err(e) = forge::export_one(name, var, host_var, f) {
                problems.push(e.message);
            }
        }
        let resolved = std::env::var(var).is_ok_and(|v| !v.is_empty());
        let mut entry = json!({ "configured": f.is_some(), "token_resolved": resolved });
        if name == "github" {
            entry["account"] = json!(resolved.then(github_account).flatten());
        }
        forge.insert(name.to_string(), entry);
    }

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
        "forge": Value::Object(forge),
        "problems": problems,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// The login `gh` authenticates as with the exported `GH_TOKEN`.
fn github_account() -> Option<String> {
    let out = Command::new("gh")
        .args(["api", "user", "--jq", ".login"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let login = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !login.is_empty()).then_some(login)
}
