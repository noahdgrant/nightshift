//! `docs/EVENTS.md` is the contract of what `ns run`, `ns auto` and `ns log --json` print. These
//! tests drive the commands through the fakes of `run.rs` and `auto.rs`, and fail when the output
//! and the document disagree in either direction: a line or field the document does not list, or
//! one it lists that no scenario printed.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::auto::{
    auto, auto_by, auto_env, dry_run, events as auto_events, three_units_on_a_broken_harness, unit,
};
use crate::common::Repo;
use crate::log::{kept, log};
use crate::run::{
    finished_then, forge_mark, lines, manifest_set, open_behind, run, run_args, run_with,
    stuck_with_person_opened_change_request, tools, BuildCommits, Fakes, FAIL, OPEN_BLOCKER,
};

/// A line `ns` printed, and the command that printed it.
type Seen = (&'static str, Value);

/// What the document says of one event: each field and the JSON types it may have.
#[derive(Default)]
struct Documented {
    fields: BTreeMap<String, BTreeSet<String>>,
}

/// The events of `docs/EVENTS.md`: a `###` heading names an event, `ns run: move` for example, and
/// the table under it has one row for each field: `| `field` | type | meaning |`, the type being
/// JSON types joined by ` or `.
fn document_parse(text: &str) -> BTreeMap<String, Documented> {
    let mut events: BTreeMap<String, Documented> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("### ") {
            let name = title.replace('`', "").trim().to_string();
            events.entry(name.clone()).or_default();
            current = Some(name);
        } else if line.starts_with('#') {
            current = None;
        } else if let (Some(name), Some(row)) = (&current, line.strip_prefix('|')) {
            let cells: Vec<&str> = row.split('|').map(str::trim).collect();
            let Some(field) = cells.first().and_then(|c| c.strip_prefix('`')) else {
                continue;
            };
            let types = cells.get(1).copied().unwrap_or_default();
            events.get_mut(name).unwrap().fields.insert(
                field.trim_end_matches('`').to_string(),
                types
                    .replace('`', "")
                    .split(" or ")
                    .map(str::to_string)
                    .collect(),
            );
        }
    }
    events
}

/// The event of the document that a printed line is. `None` for a line that is none of them.
fn event_of(command: &str, line: &Value) -> Option<String> {
    let has = |key: &str| line.get(key).is_some();
    let name = match command {
        "auto" if has("queue") => "plan",
        "auto" if has("ok") => "unit",
        "auto" if has("stopped") && !has("unit") => "stopped",
        "run" | "auto" => match () {
            _ if has("blocked_by") => "blocked_by",
            _ if has("took_over") => "took_over",
            _ if has("waiting") => "waiting",
            _ if has("not_merged") => "not_merged",
            _ if has("stopped") => return Some("ns run: stopped".to_string()),
            _ if has("from") => "move",
            _ if has("state") => "state",
            _ => return None,
        },
        "log" if has("kind") => "message",
        _ => return None,
    };
    let owner = match (command, name) {
        ("log", _) => "ns log --json",
        ("auto", "plan" | "unit" | "stopped") => "ns auto",
        _ => "ns run",
    };
    Some(format!("{owner}: {name}"))
}

fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// What is wrong between the document and the lines `ns` printed, one sentence each. Empty when
/// the document lists exactly what was printed.
fn problems(document: &str, seen: &[Seen]) -> Vec<String> {
    let documented = document_parse(document);
    let mut problems = Vec::new();
    let mut printed: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (command, line) in seen {
        let Some(name) = event_of(command, line) else {
            problems.push(format!("no event of the document is this line: {line}"));
            continue;
        };
        let Some(event) = documented.get(&name) else {
            problems.push(format!("the document has no event {name}"));
            continue;
        };
        let fields = printed.entry(name.clone()).or_default();
        for (key, value) in line.as_object().into_iter().flatten() {
            fields.insert(key.clone());
            match event.fields.get(key) {
                None => problems.push(format!("{name} printed {key}, which the document lacks")),
                Some(types) if !types.contains(type_of(value)) => problems.push(format!(
                    "{name}: {key} is a {}, the document says {types:?}",
                    type_of(value)
                )),
                Some(_) => {}
            }
        }
    }
    for (name, event) in &documented {
        match printed.get(name) {
            None => problems.push(format!("{name} is documented and was never printed")),
            Some(fields) => {
                for field in event.fields.keys().filter(|f| !fields.contains(*f)) {
                    problems.push(format!(
                        "{name}: {field} is documented and was never printed"
                    ));
                }
            }
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

fn tagged(command: &'static str, out: &[u8]) -> Vec<Seen> {
    lines(out).into_iter().map(|l| (command, l)).collect()
}

fn run_out(command: &'static str, out: assert_cmd::assert::Assert) -> Vec<Seen> {
    tagged(command, &out.get_output().stdout)
}

fn auto_seen(out: &assert_cmd::assert::Assert) -> Vec<Seen> {
    auto_events(out).into_iter().map(|l| ("auto", l)).collect()
}

/// `ns run` through a unit: its states, moves with a model, a failed state, then the gates.
fn run_scenarios() -> Vec<Vec<Seen>> {
    let mut all = Vec::new();
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    all.push(run_out("run", run(&repo, &path)));
    let repo = Repo::new();
    let failing = Fakes {
        verdict: FAIL,
        first_verdict: FAIL,
        ..Fakes::passing()
    };
    all.push(run_out(
        "run",
        run_with(&repo, &tools(&repo, &failing), &[]),
    ));
    // Stopped at a gate.
    let repo = Repo::new();
    let path = tools(&repo, &Fakes::passing());
    all.push(run_out(
        "run",
        run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"]),
    ));
    // Each run goes one state further: the gates after build, verify and review.
    for _ in 0..3 {
        all.push(run_out(
            "run",
            run_args(&repo, &path, &["run", "5", "--ci-poll-seconds", "0"]),
        ));
    }
    // Taken over once a person has opened the change request; and once it has merged.
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\".github/**\"]\n");
    let path = stuck_with_person_opened_change_request(&repo);
    all.push(run_out("run", run_with(&repo, &path, &["--merge"])));
    let repo = Repo::new();
    let path = stuck_with_person_opened_change_request(&repo);
    forge_mark(&repo, "merged", "");
    all.push(run_out("run", run(&repo, &path)));
    // Blocked by an open unit.
    let repo = Repo::new();
    let blocked = Fakes {
        blocked_by: Some(OPEN_BLOCKER),
        ..Fakes::passing()
    };
    all.push(run_out(
        "run",
        run_with(&repo, &tools(&repo, &blocked), &[]),
    ));
    // A usage limit stops the run.
    let repo = Repo::new();
    let limited = Fakes {
        build_commits: BuildCommits::TwoResumeLimited,
        ..Fakes::passing()
    };
    all.push(run_out("run", run(&repo, &tools(&repo, &limited))));
    // A change a person must merge.
    let repo = Repo::new();
    manifest_set(&repo, "[merge]\nhuman_review = [\"*.txt\"]\n");
    all.push(run_out(
        "run",
        run_with(&repo, &tools(&repo, &Fakes::passing()), &["--merge"]),
    ));
    // A finished unit whose change request is open: current, refreshed, and sent back with the evidence.
    let repo = Repo::new();
    let path = finished_then(&repo, "opened");
    all.push(run_out("run", run(&repo, &path)));
    let repo = Repo::new();
    let (path, _) = open_behind(&repo, "other.txt");
    all.push(run_out("run", run(&repo, &path)));
    let repo = Repo::new();
    let (path, _) = open_behind(&repo, "thing.txt");
    all.push(run_out("run", run(&repo, &path)));
    all
}

/// `ns auto`: a dry run, a night that runs a unit, one that stops on a harness that went wrong,
/// one that stops on a usage limit, and one that stops at its time.
fn auto_scenarios() -> Vec<Vec<Seen>> {
    let mut all = Vec::new();
    let plan = dry_run(
        &[
            unit(1, &["status:ready-for-agent"], "OWNER"),
            unit(2, &[], "OWNER"),
            unit(3, &["status:ready-for-agent"], "NONE"),
        ]
        .join(","),
    );
    all.push(vec![("auto", plan)]);
    let repo = Repo::new();
    all.push(auto_seen(&auto(&repo, &Fakes::passing(), &["--merge"])));
    let repo = Repo::new();
    all.push(auto_seen(
        &auto(&repo, &three_units_on_a_broken_harness(), &[]).failure(),
    ));
    let repo = Repo::new();
    let limited = Fakes {
        build_commits: BuildCommits::LimitedFarAhead,
        ..Fakes::passing()
    };
    all.push(auto_seen(
        &auto_by(
            &repo,
            &limited,
            &["--until", "06:30"],
            Some(std::time::Duration::from_secs(120)),
        )
        .success(),
    ));
    all.push(until_stop());
    all
}

/// A night whose time to stop comes while it runs. Each read of the clock by `ns` is an hour
/// later than the last, so a deadline three and a half hours ahead passes after the first unit.
fn until_stop() -> Vec<Seen> {
    let deadline = (chrono::Local::now() + chrono::Duration::minutes(210)).format("%H:%M");
    let repo = Repo::new();
    let fakes = Fakes {
        labels_follow_edits: false,
        units: &[1, 2, 3, 4, 5, 6],
        ..Fakes::passing()
    };
    let out = auto_env(
        &repo,
        &fakes,
        &["--until", &deadline.to_string()],
        &[("NS_CLOCK_TICK_SECONDS", "3600")],
    );
    auto_seen(&out)
}

/// `ns log --json` on a unit with three sessions.
fn log_scenario() -> Vec<Seen> {
    let out = log(&kept(), &["5", "--json"]).success();
    tagged("log", &out.get_output().stdout)
}

fn everything_printed() -> Vec<Seen> {
    std::thread::scope(|scope| {
        let runs = scope.spawn(run_scenarios);
        let autos = scope.spawn(auto_scenarios);
        let mut all: Vec<Seen> = runs.join().unwrap().into_iter().flatten().collect();
        all.extend(autos.join().unwrap().into_iter().flatten());
        all.extend(log_scenario());
        all
    })
}

const DOCUMENT: &str = include_str!("../../../docs/EVENTS.md");

#[test]
fn the_document_lists_exactly_what_ns_run_ns_auto_and_ns_log_print() {
    let seen = everything_printed();
    let problems = problems(DOCUMENT, &seen);
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

const SAMPLE: &str = "\
### ns run: state\n\n| Field | Type | Meaning |\n|---|---|---|\n| `unit` | string | the unit |\n| `state` | string | the state |\n";

fn sample() -> Vec<Seen> {
    vec![("run", serde_json::json!({"unit": "5", "state": "build"}))]
}

#[test]
fn the_checker_accepts_a_document_that_lists_what_was_printed() {
    assert_eq!(problems(SAMPLE, &sample()), Vec::<String>::new());
}

#[test]
fn the_checker_fails_when_a_printed_field_is_dropped_from_the_document() {
    let without = SAMPLE.replace("| `state` | string | the state |\n", "");
    assert_eq!(
        problems(&without, &sample()),
        ["ns run: state printed state, which the document lacks"]
    );
}

#[test]
fn the_checker_fails_when_a_printed_event_is_not_in_the_document() {
    let seen = vec![("run", serde_json::json!({"unit": "5", "waiting": "build"}))];
    assert_eq!(
        problems(SAMPLE, &seen),
        [
            "ns run: state is documented and was never printed".to_string(),
            "the document has no event ns run: waiting".to_string(),
        ]
    );
}

#[test]
fn the_checker_fails_when_a_documented_field_or_event_is_never_printed() {
    let more = format!("{SAMPLE}| `extra` | string | never printed |\n\n### ns run: waiting\n\n| `unit` | string | x |\n");
    assert_eq!(
        problems(&more, &sample()),
        [
            "ns run: state: extra is documented and was never printed".to_string(),
            "ns run: waiting is documented and was never printed".to_string(),
        ]
    );
}

#[test]
fn the_checker_fails_when_a_field_has_a_type_the_document_does_not_list() {
    let seen = vec![("run", serde_json::json!({"unit": 5, "state": "build"}))];
    let got = problems(SAMPLE, &seen);
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(
        got[0].starts_with("ns run: state: unit is a number"),
        "{got:?}"
    );
}
