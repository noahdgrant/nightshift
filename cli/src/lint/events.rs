//! Every run-log event the CLI emits has a reader: `skills/ns-supervise/` names it in a code
//! span, `cli/src/status.rs` names it in a string literal, or `ALLOWED` says why neither needs to.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

use super::{code_spans, markdown_files, LintError};

/// A step inside one unit's run. The supervisor reads the unit's `end` event, which carries the
/// outcome these lead to, and `ns log` shows them for a diagnosis.
const UNIT_STEP: &str = "a step inside one unit's run; its end event carries the outcome";
/// A step of the merge phase. The unit's `end` event says whether it merged, needs a human or
/// is stuck.
const MERGE_STEP: &str = "a step of the merge; the unit's end event says merged, done or stuck";
/// `ns watch`'s own bookkeeping, printed on its stderr and in its summary.
const WATCH_STEP: &str = "ns watch bookkeeping, printed on its stderr and in its summary";

/// What `ns quality` found about the merge conflicts a unit resolved. Its records carry the
/// count.
const CONFLICT_NOTE: &str =
    "a note on the conflicts a unit resolved; ns quality's records count them";

/// The emitted events no reader needs, each with the reason.
const ALLOWED: &[(&str, &str)] = &[
    ("artifact_invalid", UNIT_STEP),
    ("baseline_pruned", MERGE_STEP),
    ("baseline_restore_failed", MERGE_STEP),
    ("baseline_shrinks", MERGE_STEP),
    ("branch_not_deleted", MERGE_STEP),
    (
        "breach",
        "the unit ends stuck with the breach as its reason",
    ),
    ("build_output_removed", UNIT_STEP),
    ("ci_failed", MERGE_STEP),
    ("ci_passed", MERGE_STEP),
    (
        "cleanup",
        "ns clean's record of a removed worktree; nothing waits on it",
    ),
    ("conflict", MERGE_STEP),
    ("conflict_check_failed", CONFLICT_NOTE),
    ("conflict_resolved", CONFLICT_NOTE),
    ("conflict_skipped", CONFLICT_NOTE),
    ("default_moved_by_other_actor", UNIT_STEP),
    ("gate", UNIT_STEP),
    ("gate_fault", UNIT_STEP),
    ("human_review", MERGE_STEP),
    ("ledger_not_saved", UNIT_STEP),
    ("ledger_resumed", UNIT_STEP),
    ("lock_wait", UNIT_STEP),
    ("lock_wait_timeout", UNIT_STEP),
    ("ls_files_failed", UNIT_STEP),
    ("memory_low", WATCH_STEP),
    ("merge_base_moved", MERGE_STEP),
    ("merge_gate_failed", MERGE_STEP),
    ("merge_held_main_red", MERGE_STEP),
    ("merge_lock_held_through_ci", MERGE_STEP),
    ("merge_update_needed", MERGE_STEP),
    ("merge_update_skipped", MERGE_STEP),
    ("overlap_forced", WATCH_STEP),
    ("overlap_held", WATCH_STEP),
    (
        "quality_record",
        "ns quality reads the records it writes, not the event",
    ),
    ("rebase_after_base_moved", UNIT_STEP),
    ("refill", WATCH_STEP),
    ("requeued", WATCH_STEP),
    ("reship_cap", UNIT_STEP),
    (
        "self_update_built",
        "the self_update event that follows it is the one the supervisor counts",
    ),
    ("ship_after_base_moved", UNIT_STEP),
    ("skills_drain", WATCH_STEP),
    ("skills_pinned", UNIT_STEP),
    ("skills_stale", WATCH_STEP),
    ("skills_synced", WATCH_STEP),
    ("skills_unpinned", UNIT_STEP),
    ("state_rederived", UNIT_STEP),
    ("tmp_not_isolated", UNIT_STEP),
    ("tmpdir_not_removed", UNIT_STEP),
    ("update_branch", MERGE_STEP),
    ("update_branch_failed", MERGE_STEP),
    (
        "worker_end",
        "the unit's own end event carries the same outcome",
    ),
    (
        "worker_start",
        "the unit's own start event marks the same start",
    ),
];

/// The event checks for the repo at `repo`, when it holds the CLI and `ns-supervise`.
pub(super) fn check_event_consumers(repo: &Path) -> Vec<LintError> {
    check_with(repo, ALLOWED)
}

fn check_with(repo: &Path, allowed: &[(&str, &str)]) -> Vec<LintError> {
    let src = repo.join("cli/src");
    let status = src.join("status.rs");
    let supervise = repo.join("skills/ns-supervise");
    if !status.is_file() || !supervise.is_dir() {
        return Vec::new();
    }
    let emitted = emitted(&src);
    let mut read = supervise_spans(&supervise);
    read.extend(status_literals(&status));
    let error = |file: String, message: String| LintError {
        skill: "events".into(),
        file,
        message,
    };
    let allowed: BTreeMap<&str, &str> = allowed.iter().copied().collect();
    let unread = emitted
        .iter()
        .filter(|(name, _)| !read.contains(*name) && !allowed.contains_key(name.as_str()))
        .map(|(name, file)| {
            error(
                file.to_string_lossy().into_owned(),
                format!(
                    "run-log event `{name}` has no reader: name it in skills/ns-supervise/ or cli/src/status.rs, or add it with a reason to ALLOWED in cli/src/lint/events.rs"
                ),
            )
        });
    let stale = allowed.keys().filter_map(|name| {
        let why = if !emitted.contains_key(*name) {
            "ns emits no such event"
        } else if read.contains(*name) {
            "a reader names it"
        } else {
            return None;
        };
        Some(error(
            "cli/src/lint/events.rs".into(),
            format!("ALLOWED lists run-log event `{name}`, but {why}; drop it"),
        ))
    });
    unread.chain(stale).collect()
}

/// The `.rs` files under `dir`, without unit-test files (`tests.rs`, `tests/`).
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            if !p.ends_with("tests") {
                source_files(&p, out);
            }
        } else if p.extension().is_some_and(|e| e == "rs") && !p.ends_with("tests.rs") {
            out.push(p);
        }
    }
}

/// `text` up to its inline unit-test module.
fn without_tests(text: &str) -> &str {
    text.split("#[cfg(test)]\nmod tests {")
        .next()
        .unwrap_or(text)
}

/// Each event name the sources under `src` emit, with the first file that does: a
/// `"event": "<name>"` in a `json!` or a `["event"] = json!("<name>")`.
fn emitted(src: &Path) -> BTreeMap<String, PathBuf> {
    let re = Regex::new(r#""event"\s*(?::|\]\s*=\s*json!\()\s*"([A-Za-z0-9_]+)""#).unwrap();
    let mut files = Vec::new();
    source_files(src, &mut files);
    let mut names = BTreeMap::new();
    for file in files {
        let Ok(text) = fs::read_to_string(&file) else {
            continue;
        };
        for c in re.captures_iter(without_tests(&text)) {
            names.entry(c[1].to_owned()).or_insert_with(|| file.clone());
        }
    }
    names
}

/// The inline code spans of `ns-supervise`'s markdown, its eval cases left out.
fn supervise_spans(dir: &Path) -> BTreeSet<String> {
    let mut files = Vec::new();
    markdown_files(dir, &mut files);
    files
        .iter()
        .filter(|f| !f.starts_with(dir.join("evals")))
        .filter_map(|f| fs::read_to_string(f).ok())
        .flat_map(|text| {
            text.lines()
                .flat_map(|l| code_spans(l).1)
                .map(|s| s.trim().to_owned())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The string literals of `status.rs` outside its unit tests.
fn status_literals(path: &Path) -> BTreeSet<String> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let re = Regex::new(r#""([A-Za-z0-9_]+)""#).unwrap();
    re.captures_iter(without_tests(&text))
        .map(|c| c[1].to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A repo whose CLI emits `start`, `noisy` and `quiet`, with `status.rs` reading `start`.
    fn repo(supervise: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("cli/src");
        fs::create_dir_all(src.join("run")).unwrap();
        fs::create_dir_all(src.join("lint")).unwrap();
        fs::write(
            src.join("status.rs"),
            "match kind { Some(\"start\") => {} }\n#[cfg(test)]\nmod tests {\n    \"noisy\"\n}\n",
        )
        .unwrap();
        fs::write(
            src.join("run.rs"),
            "log(json!({\"event\": \"start\"}));\nev[\"event\"] = json!(\"noisy\");\n",
        )
        .unwrap();
        fs::write(
            src.join("run/drive.rs"),
            "log(json!({\n    \"event\":\"quiet\", \"x\": 1}));\n",
        )
        .unwrap();
        fs::create_dir_all(tmp.path().join("skills/ns-supervise/evals/x")).unwrap();
        fs::write(tmp.path().join("skills/ns-supervise/SKILL.md"), supervise).unwrap();
        fs::write(
            tmp.path().join("skills/ns-supervise/evals/x/case.md"),
            "`quiet` `noisy`\n",
        )
        .unwrap();
        tmp
    }

    fn messages(repo: &Path, allowed: &[(&str, &str)]) -> Vec<String> {
        check_with(repo, allowed)
            .into_iter()
            .map(|e| {
                format!(
                    "{}: {}",
                    e.file.rsplit("cli/src/").next().unwrap(),
                    e.message
                )
            })
            .collect()
    }

    #[test]
    fn an_event_no_reader_names_fails_until_the_supervisor_names_it_or_it_is_allowed() {
        let tmp = repo("Watch for `noisy` events. quiet is a word here, not a span.\n");
        let m = messages(tmp.path(), &[]);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(
            m[0].starts_with("run/drive.rs: run-log event `quiet` has no reader"),
            "{m:?}"
        );

        let tmp = repo("Watch for `noisy` and ` quiet ` events.\n");
        assert_eq!(messages(tmp.path(), &[]), Vec::<String>::new());

        let tmp = repo("Watch for `noisy` events.\n");
        assert_eq!(
            messages(tmp.path(), &[("quiet", "why")]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn eval_cases_and_status_rs_unit_tests_are_not_readers() {
        let tmp = repo("Nothing here.\n");
        let m = messages(tmp.path(), &[]);
        assert_eq!(m.len(), 2, "{m:?}");
        assert!(
            m[0].contains("`noisy`") && m[1].contains("`quiet`"),
            "{m:?}"
        );
    }

    #[test]
    fn events_emitted_only_by_unit_tests_need_no_reader() {
        let tmp = repo("`noisy` `quiet`\n");
        let src = tmp.path().join("cli/src");
        fs::write(src.join("lint/tests.rs"), "json!({\"event\": \"a\"})\n").unwrap();
        fs::create_dir_all(src.join("run/tests")).unwrap();
        fs::write(src.join("run/tests/x.rs"), "json!({\"event\": \"b\"})\n").unwrap();
        fs::write(
            src.join("clean.rs"),
            "fn f() {}\n#[cfg(test)]\nmod tests {\n    json!({\"event\": \"c\"});\n}\n",
        )
        .unwrap();
        assert_eq!(messages(tmp.path(), &[]), Vec::<String>::new());
    }

    #[test]
    fn an_allowlist_entry_that_is_not_emitted_or_has_a_reader_is_stale() {
        let tmp = repo("`noisy` `quiet`\n");
        let m = messages(tmp.path(), &[("gone", "x"), ("noisy", "x")]);
        assert_eq!(
            m,
            [
                "lint/events.rs: ALLOWED lists run-log event `gone`, but ns emits no such event; drop it",
                "lint/events.rs: ALLOWED lists run-log event `noisy`, but a reader names it; drop it",
            ]
        );
    }

    #[test]
    fn a_repo_without_the_cli_or_the_supervisor_is_not_checked() {
        let tmp = repo("Nothing here.\n");
        fs::remove_dir_all(tmp.path().join("skills/ns-supervise")).unwrap();
        assert_eq!(messages(tmp.path(), &[]), Vec::<String>::new());
        let tmp = repo("Nothing here.\n");
        fs::remove_file(tmp.path().join("cli/src/status.rs")).unwrap();
        assert_eq!(messages(tmp.path(), &[]), Vec::<String>::new());
    }
}
