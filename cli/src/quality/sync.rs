//! Quality records in and out: written when a unit ends, imported from archived worktrees, and
//! read back for `ns quality` (docs/DESIGN.md D39).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use super::artifacts::{self, Unparsed};
use super::metrics::Unit;
use super::record::{self, Meta, Record};
use super::runlog;
use super::store::{self, Sent};
use crate::clock::Clock;
use crate::error::SfError;
use crate::git::Repo;
use crate::run::currency::known_pr;
use crate::worktree;

/// The records on origin's branch (fetched first) and in the outbox, and a summary of where
/// they came from.
fn stored(repo: &Repo) -> (Vec<Record>, Value) {
    let fetched = store::fetch(&repo.root);
    let (mut records, bad_remote) = record::parse(&store::tracked(&repo.root).unwrap_or_default());
    let on_branch = records.len();
    let outbox = fs::read_to_string(store::outbox(&repo.common_dir)).unwrap_or_default();
    let (waiting, bad_outbox) = record::parse(&outbox);
    let info = json!({
        "branch": format!("origin/{}", store::BRANCH),
        "fetched": fetched.is_ok(),
        "fetch_error": fetched.err().map(|e| format!("{e:#}")),
        "on_branch": on_branch,
        "in_outbox": waiting.len(),
        "bad_lines": bad_remote + bad_outbox,
    });
    records.extend(waiting);
    (records, info)
}

/// The units the records hold, and where the records came from.
pub fn stored_units(repo: &Repo) -> (Vec<Unit>, Value) {
    let (records, mut info) = stored(repo);
    let units = record::units(&records);
    info["units"] = json!(units.len());
    (units, info)
}

fn lines(records: &[Record]) -> Result<Vec<String>> {
    records
        .iter()
        .map(|r| Ok(serde_json::to_string(r)?))
        .collect()
}

fn sent_json(sent: &Sent) -> Value {
    match sent {
        Sent::Nothing => json!({"status": "nothing", "records": 0}),
        Sent::Pushed { records, tries } => {
            json!({"status": "pushed", "records": records, "tries": tries})
        }
        Sent::Outbox { records, reason } => {
            json!({"status": "outbox", "records": records, "reason": reason})
        }
    }
}

/// The review attempts the unit in `worktree` has, their records as JSON lines, and the
/// review artifacts that couldn't be read, which no record holds.
fn unit_lines(
    worktree: &Path,
    meta: &Meta,
    now: i64,
) -> (usize, Result<Vec<String>>, Vec<Unparsed>) {
    let dir = worktree.join(".ns").join(&meta.unit);
    let mut gaps = Vec::new();
    let attempts = artifacts::read_attempts(worktree, &dir, worktree, &mut gaps);
    let lines = record::build(meta, &attempts, now, worktree).and_then(|r| lines(&r));
    (attempts.len(), lines, gaps)
}

/// Write the quality records of a unit that ended, and flush the outbox. Never fails: the
/// result is returned as a run-log event, and anything unpushed waits in the outbox.
pub fn record_unit(repo: &Repo, worktree: &Path, meta: &Meta) -> Value {
    let (attempts, built, gaps) = unit_lines(worktree, meta, Clock::from_env().now());
    let mut ev = json!({"event": "quality_record", "attempts": attempts});
    if !gaps.is_empty() {
        ev["unparsed"] = json!(gaps);
    }
    let new = match built {
        Ok(l) => l,
        Err(e) => {
            ev["error"] = json!(format!("{e:#}"));
            Vec::new()
        }
    };
    match store::publish(&repo.root, &repo.common_dir, &new) {
        Ok(sent) => {
            if let Sent::Outbox { reason, .. } = &sent {
                eprintln!("ns: quality records kept in the outbox: {reason}");
            }
            ev.as_object_mut()
                .unwrap()
                .extend(sent_json(&sent).as_object().unwrap().clone());
        }
        Err(e) => ev["error"] = json!(format!("{e:#}")),
    }
    if let Some(e) = ev["error"].as_str() {
        eprintln!("ns: quality record not written: {e}");
    }
    ev
}

/// Write the quality records of several units in one push, and flush the outbox: before their
/// worktrees are removed. Never fails. Returns the run-log event and, for each unit in order,
/// why its records are not saved, or `None` when they are: pushed, waiting in the outbox, or
/// none to write. A review artifact that can't be read is not saved: no record holds it.
pub fn record_units(repo: &Repo, units: &[(&Path, &Meta)]) -> (Value, Vec<Option<String>>) {
    let now = Clock::from_env().now();
    let mut unsaved = vec![None; units.len()];
    let mut new = Vec::new();
    for (i, (worktree, meta)) in units.iter().enumerate() {
        let (_, built, gaps) = unit_lines(worktree, meta, now);
        match built {
            Ok(l) => new.extend(l),
            Err(e) => unsaved[i] = Some(format!("{e:#}")),
        }
        if let Some(g) = gaps.first().filter(|_| unsaved[i].is_none()) {
            unsaved[i] = Some(format!("cannot read {}: {}", g.path, g.reason));
        }
    }
    let names: Vec<&str> = units.iter().map(|(_, m)| m.unit.as_str()).collect();
    let mut ev = json!({"event": "quality_record", "units": names});
    match store::publish(&repo.root, &repo.common_dir, &new) {
        Ok(sent) => {
            if let Sent::Outbox { reason, .. } = &sent {
                eprintln!("ns: quality records kept in the outbox: {reason}");
            }
            ev.as_object_mut()
                .unwrap()
                .extend(sent_json(&sent).as_object().unwrap().clone());
        }
        Err(e) => {
            let e = format!("{e:#}");
            ev["error"] = json!(e);
            for u in unsaved.iter_mut().filter(|u| u.is_none()) {
                *u = Some(e.clone());
            }
        }
    }
    let failed: Vec<Value> = names
        .iter()
        .zip(&unsaved)
        .filter_map(|(n, u)| Some(json!({"unit": n, "error": u.as_ref()?})))
        .collect();
    if !failed.is_empty() {
        ev["unsaved"] = json!(failed);
    }
    (ev, unsaved)
}

/// `ns quality import <dir>`: records for each archived `<dir>/<unit>/.ns/<unit>/` that origin's
/// branch doesn't hold yet, pushed in one commit.
pub fn import(dir: &Path, dry_run: bool) -> Result<ExitCode> {
    if !dir.is_dir() {
        return Err(SfError::usage(
            format!("{} is not a directory", dir.display()),
            "pass the directory holding <unit>/.ns/<unit>/:\n  ns quality import /tmp/unit-artifacts --dry-run",
        )
        .into());
    }
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let (existing, info) = stored(&repo);
    let have: BTreeSet<&str> = existing.iter().map(|r| r.unit.as_str()).collect();
    let (_, runs) = runlog::read(&repo.common_dir.join("ns").join("runs.jsonl"), None);
    let now = Clock::from_env().now();
    let mut names: Vec<String> = fs::read_dir(dir)
        .with_context(|| format!("cannot read {}", dir.display()))?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let (mut new, mut imported, mut already, mut skipped) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut unparsed: Vec<Unparsed> = Vec::new();
    for name in names {
        let wt = dir.join(&name);
        let art = wt.join(".ns").join(&name);
        if !worktree::valid_unit_id(&name) {
            skipped.push(json!({"unit": name, "reason": "not a unit id"}));
            continue;
        }
        if !art.is_dir() {
            skipped.push(json!({"unit": name, "reason": format!("no .ns/{name}/ inside")}));
            continue;
        }
        if have.contains(name.as_str()) {
            already.push(name);
            continue;
        }
        let attempts = artifacts::read_attempts(&wt, &art, &repo.root, &mut unparsed);
        if attempts.is_empty() {
            skipped.push(json!({"unit": name, "reason": "no review artifact"}));
            continue;
        }
        let logged = runs.get(&name);
        let meta = Meta {
            issue: record::issue_of(&name),
            pr: known_pr(&art).or(logged.and_then(|s| s.pr)),
            outcome: logged.and_then(|s| s.outcome.clone()),
            unit: name.clone(),
        };
        match record::build(&meta, &attempts, now, &repo.root) {
            Ok(rs) => {
                new.extend(rs);
                imported.push(name);
            }
            Err(e) => skipped.push(json!({"unit": name, "reason": format!("{e:#}")})),
        }
    }
    let mut out = json!({
        "ok": true,
        "command": "quality import",
        "dir": dir.display().to_string(),
        "dry_run": dry_run,
        "units": imported.len(),
        "records": new.len(),
        "imported": imported,
        "already_recorded": already,
        "skipped": skipped,
        "unparsed": unparsed,
        "branch": info,
    });
    let code = if imported.is_empty() && already.is_empty() {
        out["ok"] = json!(false);
        out["status"] = json!("nothing-to-import");
        eprintln!(
            "error: no unit in {} has a review artifact to import",
            dir.display()
        );
        eprintln!("  expected: {}/<unit>/.ns/<unit>/review.md", dir.display());
        if !skipped.is_empty() {
            eprintln!(
                "  skipped:  {} (see \"skipped\" in the output)",
                skipped.len()
            );
        }
        eprintln!("  check:    ns quality import {} --dry-run", dir.display());
        ExitCode::from(1)
    } else if dry_run {
        let status = if imported.is_empty() {
            "already-done"
        } else {
            "planned"
        };
        out["status"] = json!(status);
        ExitCode::SUCCESS
    } else {
        let sent = store::publish(&repo.root, &repo.common_dir, &lines(&new)?)?;
        out["push"] = sent_json(&sent);
        out["status"] = json!(match sent {
            Sent::Nothing => "already-done",
            Sent::Pushed { .. } => "pushed",
            Sent::Outbox { .. } => "outbox",
        });
        if matches!(sent, Sent::Outbox { .. }) {
            out["ok"] = json!(false);
            eprintln!("error: the records are in the outbox, not on origin");
            eprintln!("  check:  git remote get-url origin");
            eprintln!("  then:   ns quality import {}", dir.display());
            ExitCode::from(1)
        } else {
            ExitCode::SUCCESS
        }
    };
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{commit_file, g};

    #[test]
    fn a_unit_whose_records_cannot_be_built_is_unsaved_and_the_others_are_saved() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        g(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "README", "hi\n");
        let repo = Repo::discover(&root).unwrap();
        let metas: Vec<Meta> = ["x /srv/y", "7-fine"]
            .iter()
            .map(|u| {
                let dir = root.join(".ns").join(u);
                fs::create_dir_all(&dir).unwrap();
                fs::write(dir.join("review.md"), "---\nstatus: pass\n---\n").unwrap();
                Meta {
                    unit: u.to_string(),
                    ..Meta::default()
                }
            })
            .collect();
        let units: Vec<(&Path, &Meta)> = metas.iter().map(|m| (root.as_path(), m)).collect();
        let (ev, unsaved) = record_units(&repo, &units);
        assert!(
            unsaved[0].as_deref().unwrap().contains("absolute path"),
            "{ev}"
        );
        assert_eq!(unsaved[1], None);
        // No origin: the good unit's record waits in the outbox, which counts as saved.
        assert_eq!(
            (ev["status"].as_str(), ev["records"].as_u64()),
            (Some("outbox"), Some(1))
        );
        assert_eq!(ev["unsaved"][0]["unit"], "x /srv/y");
        let outbox = fs::read_to_string(store::outbox(&repo.common_dir)).unwrap();
        assert!(outbox.contains("\"unit\":\"7-fine\""), "{outbox}");
    }

    #[test]
    fn a_record_that_fails_the_privacy_check_is_reported_and_the_outbox_still_flushes() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let root = base.join("repo");
        fs::create_dir(&root).unwrap();
        g(&root, &["init", "-q", "-b", "main"]);
        commit_file(&root, "README", "hi\n");
        g(&base, &["init", "-q", "--bare", "remote.git"]);
        let remote = base.join("remote.git");
        g(
            &root,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        let repo = Repo::discover(&root).unwrap();
        fs::create_dir_all(repo.common_dir.join("ns")).unwrap();
        fs::write(store::outbox(&repo.common_dir), "{\"waiting\":1}\n").unwrap();
        let unit = "x /srv/y";
        let dir = root.join(".ns").join(unit);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("review.md"), "---\nstatus: pass\n---\n").unwrap();
        let meta = Meta {
            unit: unit.into(),
            ..Meta::default()
        };
        let ev = record_unit(&repo, &root, &meta);
        assert_eq!(ev["attempts"], 1);
        assert!(
            ev["error"].as_str().unwrap().contains("absolute path"),
            "{ev}"
        );
        assert_eq!(
            (ev["status"].as_str(), ev["records"].as_u64()),
            (Some("pushed"), Some(1))
        );
        let text = g(&remote, &["show", "nightshift/quality:records.jsonl"]);
        assert_eq!(text, "{\"waiting\":1}");
    }
}
