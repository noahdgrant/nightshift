//! `ns status`: the night at a glance. The units an `ns run` holds now with their last logged
//! phase, the units whose run ended since `--since` grouped by outcome, and the next ready issues
//! in the `ns watch` queue. Reads only the run-log fields `ns log` prints.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::clock::{self, Clock};
use crate::factory::{self, Factory};
use crate::git::Repo;
use crate::log::{self, PrBase};
use crate::quality::{self, issue_of, Window};
use crate::run;
use crate::watch::{self, forge::Forge};

/// How far back `ns status` looks without `--since`.
const DEFAULT_SINCE_S: i64 = 24 * 60 * 60;

/// Where an ended unit goes by its outcome: (JSON key, heading, outcomes), in print order. An
/// outcome none names, such as `interrupted`, goes in `other`.
const BUCKETS: [(&str, &str, &[&str]); 5] = [
    ("merged", "merged", &["merged"]),
    ("pr_for_human", "PR for a human", &["done"]),
    ("stuck", "stuck", &["stuck", "budget"]),
    ("paused", "paused", &["paused"]),
    ("split", "split", &["split"]),
];
const OTHER: &str = "other";

pub fn cli(since: Option<String>, as_json: bool) -> Result<ExitCode> {
    let since = match since {
        Some(s) => quality::instant("status", "since", &s, "00:00:00")?,
        None => Clock::from_env().now() - DEFAULT_SINCE_S,
    };
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let path = repo.common_dir.join("ns").join("runs.jsonl");
    let text = match fs::read(&path) {
        Ok(b) => String::from_utf8_lossy(&b).into_owned(),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            eprintln!(
                "no run log at {} yet; ns run and ns watch write one",
                path.display()
            );
            String::new()
        }
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", path.display())),
    };
    let running: Vec<String> = run::Lock::run_holders(&repo.common_dir)?
        .iter()
        .filter_map(|h| h["unit"].as_str().map(String::from))
        .collect();
    let (mut summary, bad) = summarise(&text, since, &running, log::pr_base(&repo).as_ref());
    if bad > 0 {
        eprintln!(
            "skipped {bad} line(s) of {} that are not JSON objects",
            path.display()
        );
    }
    let fac = factory::load(&factory::root(None, &repo.root));
    let next = fac.and_then(|fac| {
        let forge = watch::forge::open(&fac.queue, &repo.root)?;
        next(forge.as_ref(), &fac)
    });
    summary["next"] = next.unwrap_or_else(|e| {
        eprintln!("ns status: queue unavailable: {e:#}");
        Value::Null
    });
    if as_json {
        println!("{summary}");
    } else {
        print!("{}", text_of(&summary));
    }
    Ok(ExitCode::SUCCESS)
}

/// The ready issues `ns watch` would take next, in its order, as `{number, title}`.
fn next(forge: &dyn Forge, fac: &Factory) -> Result<Value> {
    let q = watch::queue(forge, fac, &Default::default())?;
    Ok(q.ready
        .iter()
        .map(|i| json!({"number": i.number, "title": i.title}))
        .collect())
}

/// The summary of the run log `text`: `running` with each unit's last phase event, and each
/// other unit whose last `end` at or after `since` falls in a bucket. `next` is left for the
/// caller. Also returns how many non-blank lines are not JSON objects.
fn summarise(
    text: &str,
    since: i64,
    running: &[String],
    pr_base: Option<&PrBase>,
) -> (Value, usize) {
    let window = Window {
        since: Some(since),
        until: None,
    };
    let mut bad = 0;
    let mut last_phase: BTreeMap<&str, Map<String, Value>> = BTreeMap::new();
    let mut last_end: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let Some(mut ev) = log::event(line, pr_base) else {
            bad += 1;
            continue;
        };
        let Some(unit) = ev.get("unit").and_then(Value::as_str).map(String::from) else {
            continue;
        };
        let kind = ev.remove("event");
        match kind.as_ref().and_then(Value::as_str) {
            Some("phase") => {
                if let Some(u) = running.iter().find(|u| **u == unit) {
                    last_phase.insert(u, ev);
                }
            }
            Some("end") => {
                let ts = ev.get("ts").and_then(Value::as_str);
                if window.contains(ts.and_then(clock::parse_iso)) {
                    last_end.insert(unit, ev);
                }
            }
            _ => {}
        }
    }
    let running: Vec<Value> = running
        .iter()
        .map(|u| {
            let mut ev = last_phase.remove(u.as_str()).unwrap_or_default();
            ev.insert("unit".into(), json!(u));
            if let Some(n) = issue_of(u).filter(|_| !ev.contains_key("issue")) {
                ev.insert("issue".into(), json!(n));
            }
            Value::Object(ev)
        })
        .collect();
    let mut ended = Map::new();
    for (key, _, _) in BUCKETS {
        ended.insert(key.into(), json!([]));
    }
    ended.insert(OTHER.into(), json!([]));
    for (unit, ev) in last_end {
        if running.iter().any(|r| r["unit"] == unit.as_str()) {
            continue;
        }
        let outcome = ev.get("outcome").and_then(Value::as_str).unwrap_or("");
        let key = BUCKETS
            .iter()
            .find(|(_, _, outcomes)| outcomes.contains(&outcome))
            .map_or(OTHER, |(key, _, _)| key);
        if let Some(list) = ended[key].as_array_mut() {
            list.push(Value::Object(ev));
        }
    }
    let summary = json!({
        "since": clock::iso(since),
        "running": running,
        "ended": ended,
        "next": null,
    });
    (summary, bad)
}

/// The human form of a summary: a heading with a count per part, then a line per entry as
/// `ns log` prints it. `other` shows only when it holds a unit; a `null` `next` is "queue
/// unavailable".
fn text_of(s: &Value) -> String {
    let mut out = format!("since {}\n", s["since"].as_str().unwrap_or(""));
    let event = |e: &Value| e.as_object().map(log::text).unwrap_or_default();
    let list = |k: &Value| k.as_array().cloned().unwrap_or_default();
    part(&mut out, "running", &list(&s["running"]), event);
    for (key, heading, _) in BUCKETS {
        part(&mut out, heading, &list(&s["ended"][key]), event);
    }
    let other = list(&s["ended"][OTHER]);
    if !other.is_empty() {
        part(&mut out, OTHER, &other, event);
    }
    let issue = |i: &Value| {
        let title: String = i["title"]
            .as_str()
            .unwrap_or("")
            .chars()
            .flat_map(log::printable)
            .collect();
        format!("#{} {title}", i["number"])
    };
    match s["next"].as_array() {
        Some(next) => part(&mut out, "next", next, issue),
        None => out.push_str("queue unavailable\n"),
    }
    out
}

/// `<heading> (<count>)`, then each entry indented as `line` prints it.
fn part(out: &mut String, heading: &str, entries: &[Value], line: impl Fn(&Value) -> String) {
    out.push_str(&format!("{heading} ({})\n", entries.len()));
    for e in entries {
        out.push_str(&format!("  {}\n", line(e)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watch::forge::{ClosingPrs, PrState};
    use crate::watch::Issue;

    const SINCE: &str = "2026-10-09T00:00:00Z";

    /// One unit per bucket, a unit that ended before `SINCE`, a unit that ended stuck and then
    /// merged, and a running unit that paused earlier tonight.
    const FIXTURE: &str = r#"{"event":"end","unit":"100-old","outcome":"merged","ts":"2026-10-08T23:59:59Z"}
{"event":"end","unit":"101-merged","outcome":"stuck","phase":"review","ts":"2026-10-09T01:00:00Z"}
{"event":"end","unit":"101-merged","outcome":"merged","pr":11,"prompt":"secret","ts":"2026-10-09T02:00:00Z"}
{"event":"end","unit":"102-done","outcome":"done","pr":12,"reason":"needs human review","ts":"2026-10-09T02:10:00Z"}
{"event":"end","unit":"103-stuck","outcome":"stuck","phase":"verify","reason":"verify failed twice","ts":"2026-10-09T02:20:00Z"}
{"event":"end","unit":"104-budget","outcome":"budget","phase":"build","ts":"2026-10-09T02:30:00Z"}
{"event":"end","unit":"105-paused","outcome":"paused","phase":"build","ts":"2026-10-09T02:40:00Z"}
{"event":"end","unit":"106-split","outcome":"split","phase":"triage","ts":"2026-10-09T02:50:00Z"}
{"event":"end","unit":"107-stopped","outcome":"interrupted","reason":"stopped by SIGTERM","ts":"2026-10-09T02:55:00Z"}
{"event":"start","unit":"108-live","ts":"2026-10-09T03:00:00Z"}
{"event":"end","unit":"108-live","outcome":"paused","ts":"2026-10-09T03:10:00Z"}
{"event":"phase","unit":"108-live","phase":"build","attempt":1,"status":"pass","transcript":"/tmp/t","ts":"2026-10-09T03:20:00Z"}
{"event":"phase","unit":"108-live","phase":"verify","attempt":1,"status":"fail","ts":"2026-10-09T03:30:00Z"}
{"event":"phase","unit":"102-done","phase":"ship","attempt":1,"status":"pass","ts":"2026-10-09T02:09:00Z"}
not json
"#;

    fn base() -> PrBase {
        PrBase::new("https://github.com/o/r.git", None, None).unwrap()
    }

    fn fixture() -> (Value, usize) {
        let since = clock::parse_iso(SINCE).unwrap();
        let running = ["108-live".to_string(), "109-new".to_string()];
        summarise(FIXTURE, since, &running, Some(&base()))
    }

    fn units(v: &Value) -> Vec<&str> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|e| e["unit"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn each_outcome_lands_in_its_bucket() {
        let (s, bad) = fixture();
        assert_eq!(bad, 1);
        let e = &s["ended"];
        assert_eq!(units(&e["merged"]), ["101-merged"]);
        assert_eq!(units(&e["pr_for_human"]), ["102-done"]);
        assert_eq!(units(&e["stuck"]), ["103-stuck", "104-budget"]);
        assert_eq!(units(&e["paused"]), ["105-paused"]);
        assert_eq!(units(&e["split"]), ["106-split"]);
        assert_eq!(units(&e["other"]), ["107-stopped"]);
        assert_eq!(
            e["merged"][0],
            json!({"unit":"101-merged","issue":101,"outcome":"merged","pr":11,
                   "pr_url":"https://github.com/o/r/pull/11","ts":"2026-10-09T02:00:00Z"})
        );
    }

    #[test]
    fn running_units_carry_their_last_phase_and_leave_the_buckets() {
        let (s, _) = fixture();
        assert_eq!(
            s["running"],
            json!([
                {"unit":"108-live","issue":108,"phase":"verify","attempt":1,"status":"fail",
                 "ts":"2026-10-09T03:30:00Z"},
                {"unit":"109-new","issue":109},
            ])
        );
        assert_eq!(units(&s["ended"]["paused"]), ["105-paused"]);
    }

    #[test]
    fn a_running_units_issue_is_the_logged_one_else_its_ids() {
        let line = r#"{"event":"phase","unit":"7-a","issue":8,"phase":"build"}"#;
        let running = ["7-a".to_string(), "x-b".to_string()];
        let (s, _) = summarise(line, 0, &running, None);
        assert_eq!(
            s["running"],
            json!([{"unit":"7-a","issue":8,"phase":"build"},{"unit":"x-b"}])
        );
    }

    #[test]
    fn an_end_exactly_at_since_counts_and_one_before_does_not() {
        let at = |ts: &str| {
            let line = format!(r#"{{"event":"end","unit":"7-a","outcome":"merged","ts":"{ts}"}}"#);
            let since = clock::parse_iso(SINCE).unwrap();
            units(&summarise(&line, since, &[], None).0["ended"]["merged"]).len()
        };
        assert_eq!(at(SINCE), 1);
        assert_eq!(at("2026-10-08T23:59:59Z"), 0);
        let undated = r#"{"event":"end","unit":"7-a","outcome":"merged"}"#;
        assert_eq!(
            summarise(undated, 0, &[], None).0["ended"]["merged"],
            json!([])
        );
    }

    #[test]
    fn events_without_a_unit_are_ignored() {
        let line = r#"{"event":"end","outcome":"merged","ts":"2026-10-09T01:00:00Z"}"#;
        let (s, bad) = summarise(line, 0, &[], None);
        assert_eq!((s["ended"]["merged"].clone(), bad), (json!([]), 0));
    }

    #[test]
    fn the_text_holds_what_the_json_holds() {
        let (mut s, _) = fixture();
        s["next"] = json!([{"number": 120, "title": "Add\u{1b}[31m x"}]);
        assert_eq!(
            text_of(&s),
            "since 2026-10-09T00:00:00Z
running (2)
  2026-10-09T03:30:00Z #108 verify attempt 1 status fail
  #109
merged (1)
  2026-10-09T02:00:00Z #101 outcome merged https://github.com/o/r/pull/11
PR for a human (1)
  2026-10-09T02:10:00Z #102 outcome done https://github.com/o/r/pull/12 reason: needs human review
stuck (2)
  2026-10-09T02:20:00Z #103 verify outcome stuck reason: verify failed twice
  2026-10-09T02:30:00Z #104 build outcome budget
paused (1)
  2026-10-09T02:40:00Z #105 build outcome paused
split (1)
  2026-10-09T02:50:00Z #106 triage outcome split
other (1)
  2026-10-09T02:55:00Z #107 outcome interrupted reason: stopped by SIGTERM
next (1)
  #120 Add\\u{1b}[31m x
"
        );
        let json: Value = serde_json::from_str(&s.to_string()).unwrap();
        assert_eq!(json, s);
        assert!(!s.to_string().contains("secret"));
        assert!(!s.to_string().contains("transcript"));
    }

    #[test]
    fn an_empty_night_shows_each_part_and_an_unavailable_queue() {
        let (s, bad) = summarise("", 0, &[], None);
        assert_eq!(bad, 0);
        assert_eq!(
            text_of(&s),
            "since 1970-01-01T00:00:00Z\nrunning (0)\nmerged (0)\nPR for a human (0)\nstuck (0)\n\
             paused (0)\nsplit (0)\nqueue unavailable\n"
        );
    }

    struct Fake(Vec<Issue>);

    impl Forge for Fake {
        fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
            match label {
                Some("broken") => anyhow::bail!("gh: not logged in"),
                _ => Ok(self.0.clone()),
            }
        }
        fn closing_prs(&self, _: PrState) -> Result<ClosingPrs> {
            Ok(ClosingPrs::new())
        }
        fn is_closed(&self, _: u64) -> Result<bool> {
            Ok(false)
        }
        fn labels(&self, _: u64) -> Result<Vec<String>> {
            unreachable!("ns status only reads the queue")
        }
        fn edit_labels(&self, _: u64, _: &[String], _: Option<&str>) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn comment(&self, _: u64, _: &str) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn close(&self, _: u64) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
    }

    #[test]
    fn next_is_the_watch_queue_in_its_order() {
        let mut fac = Factory::default();
        let issue = |number, labels: &[&str], body: &str| Issue {
            number,
            title: format!("Fix {number}"),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            body: body.into(),
            team: true,
        };
        let high = fac.queue.priority[0].clone();
        let forge = Fake(vec![
            issue(5, &[], ""),
            issue(6, &[&high], ""),
            issue(7, &[], "Blocked by: #1"),
        ]);
        assert_eq!(
            next(&forge, &fac).unwrap(),
            json!([{"number":6,"title":"Fix 6"},{"number":5,"title":"Fix 5"}])
        );
        fac.queue.ready_label = "broken".into();
        assert!(next(&forge, &fac).is_err());
    }
}
