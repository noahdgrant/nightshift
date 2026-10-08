//! `sf doctor`: config and harness availability report.

use anyhow::Result;
use serde_json::{json, Map, Value};

use crate::config;
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
                .push("no [roles.default]; roles without a match exit 3 from sf ask".to_string());
        }
    } else if !exists {
        problems.push(format!(
            "no config at {}; sf ask exits 3 until one exists (see sf ask --help)",
            path.display()
        ));
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
        "problems": problems,
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
