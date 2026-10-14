//! `ns status`: the night at a glance. The units an `ns run` holds now with their last logged
//! phase, the units whose run ended since `--since` grouped by outcome, and the next ready issues
//! in the `ns watch` queue. Reads only the run-log fields `ns log` prints.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::io;
use std::process::ExitCode;

use anyhow::{Context, Result};
use serde_json::{json, Map, Value};

use crate::clock::{self, Clock};
use crate::factory::{self, Factory};
use crate::git::Repo;
use crate::log;
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

/// The run-log events that say what main's CI did to the night (docs/factory/watch.md,
/// "Main's CI").
const MAIN_EVENTS: [&str; 3] = ["main_red", "main_green", "main_red_unfixed"];

type Event = Map<String, Value>;

/// The night: what runs now, what ended since `since`, and the queue if it could be read.
struct Summary {
    since: i64,
    running: Vec<Event>,
    ended: BTreeMap<&'static str, Vec<Event>>,
    /// Main's red and green events, oldest first.
    main: Vec<Event>,
    next: Option<Vec<Queued>>,
}

struct Queued {
    number: u64,
    title: String,
}

pub fn cli(since: Option<String>, as_json: bool) -> Result<ExitCode> {
    let since = match since {
        Some(s) => quality::instant("status", "since", &s, "00:00:00")?,
        None => Clock::from_env().now() - DEFAULT_SINCE_S,
    };
    let repo = Repo::discover(&std::env::current_dir().context("cannot read current directory")?)?;
    let path = repo.run_log();
    let running: Vec<String> = run::Lock::run_holders(&repo.common_dir)?
        .iter()
        .filter_map(|h| h["unit"].as_str().map(String::from))
        .collect();
    let base = log::pr_base(&repo);
    let bad = Cell::new(0);
    let mut summary = log::open_lines(&path)
        .and_then(|lines| summarise(log::events(lines, base.as_ref(), &bad), since, &running))
        .with_context(|| format!("ns status on {}", path.display()))?;
    log::report_bad(bad.get(), &path);
    let fac = factory::load(&factory::root(None, &repo.root));
    let queue = fac.and_then(|fac| {
        let forge = watch::forge::open(&fac.queue, &repo.root)?;
        next(forge.as_ref(), &fac)
    });
    summary.next = match queue {
        Ok(next) => Some(next),
        Err(e) => {
            eprintln!("ns status: queue unavailable: {e:#}");
            None
        }
    };
    if as_json {
        println!("{}", summary.to_json());
    } else {
        print!("{}", summary.text());
    }
    Ok(ExitCode::SUCCESS)
}

/// The ready issues `ns watch` would take next, in its order.
fn next(forge: &dyn Forge, fac: &Factory) -> Result<Vec<Queued>> {
    let q = watch::queue(forge, fac, &Default::default())?;
    Ok(q.ready
        .iter()
        .map(|i| Queued {
            number: i.number,
            title: i.title.clone(),
        })
        .collect())
}

/// The summary of the run-log `events`: `running` with each unit's last phase event, and each
/// other unit whose last `end` at or after `since` falls in a bucket. `next` is left unread.
fn summarise(
    events: impl IntoIterator<Item = io::Result<Event>>,
    since: i64,
    running: &[String],
) -> io::Result<Summary> {
    let window = Window {
        since: Some(since),
        until: None,
    };
    let mut last_phase: BTreeMap<&str, Event> = BTreeMap::new();
    let mut last_end: BTreeMap<String, Event> = BTreeMap::new();
    let mut main = Vec::new();
    for ev in events {
        let mut ev = ev?;
        let kind = ev.get("event").and_then(Value::as_str);
        if kind.is_some_and(|k| MAIN_EVENTS.contains(&k)) {
            let ts = ev.get("ts").and_then(Value::as_str);
            if window.contains(ts.and_then(clock::parse_iso)) {
                main.push(ev);
            }
            continue;
        }
        let Some(unit) = ev.get("unit").and_then(Value::as_str).map(String::from) else {
            continue;
        };
        let kind = ev.remove("event");
        match kind.as_ref().and_then(Value::as_str) {
            Some("start") => {
                last_phase.remove(unit.as_str());
            }
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
    let running_events = running
        .iter()
        .map(|u| {
            let mut ev = last_phase.remove(u.as_str()).unwrap_or_default();
            ev.insert("unit".into(), json!(u));
            if let Some(n) = issue_of(u).filter(|_| !ev.contains_key("issue")) {
                ev.insert("issue".into(), json!(n));
            }
            ev
        })
        .collect();
    let mut ended: BTreeMap<&'static str, Vec<Event>> = BUCKETS
        .iter()
        .map(|(key, _, _)| *key)
        .chain([OTHER])
        .map(|key| (key, Vec::new()))
        .collect();
    for (unit, ev) in last_end {
        if running.contains(&unit) {
            continue;
        }
        let outcome = ev.get("outcome").and_then(Value::as_str);
        let key = BUCKETS
            .iter()
            .find(|(_, _, outcomes)| outcome.is_some_and(|o| outcomes.contains(&o)))
            .map_or(OTHER, |(key, _, _)| key);
        ended.entry(key).or_default().push(ev);
    }
    Ok(Summary {
        since,
        running: running_events,
        ended,
        main,
        next: None,
    })
}

impl Summary {
    fn to_json(&self) -> Value {
        let ended: Map<String, Value> = self
            .ended
            .iter()
            .map(|(key, evs)| ((*key).into(), json!(evs)))
            .collect();
        let next = self.next.as_ref().map(|next| {
            next.iter()
                .map(|q| json!({"number": q.number, "title": q.title}))
                .collect::<Vec<_>>()
        });
        json!({
            "since": clock::iso(self.since),
            "running": self.running,
            "ended": ended,
            "main": self.main,
            "next": next,
        })
    }

    /// A heading with a count per part, then a line per entry as `ns log` prints it. `other`
    /// shows only when it holds a unit; a `next` that could not be read is "queue unavailable".
    fn text(&self) -> String {
        let mut out = format!("since {}\n", clock::iso(self.since));
        part(&mut out, "running", &self.running);
        for (key, heading, _) in BUCKETS {
            part(&mut out, heading, &self.ended[key]);
        }
        if !self.ended[OTHER].is_empty() {
            part(&mut out, OTHER, &self.ended[OTHER]);
        }
        if !self.main.is_empty() {
            part(&mut out, "main", &self.main);
        }
        match &self.next {
            Some(next) => {
                out.push_str(&format!("next ({})\n", next.len()));
                for q in next {
                    let title: String = q.title.chars().flat_map(log::printable).collect();
                    out.push_str(&format!("  #{} {title}\n", q.number));
                }
            }
            None => out.push_str("queue unavailable\n"),
        }
        out
    }
}

/// `<heading> (<count>)`, then each event indented as `ns log` prints it.
fn part(out: &mut String, heading: &str, events: &[Event]) {
    out.push_str(&format!("{heading} ({})\n", events.len()));
    for e in events {
        out.push_str(&format!("  {}\n", log::text(e)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::PrBase;
    use crate::watch::forge::{ClosingPrs, IssueState, PrState};
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

    fn summary(
        text: &str,
        since: i64,
        running: &[&str],
        base: Option<&PrBase>,
    ) -> (Summary, usize) {
        let lines = text.lines().map(|l| Ok(l.as_bytes().to_vec()));
        let bad = Cell::new(0);
        let running: Vec<String> = running.iter().map(|u| u.to_string()).collect();
        let s = summarise(log::events(lines, base, &bad), since, &running).unwrap();
        (s, bad.get())
    }

    fn fixture() -> (Value, usize) {
        let since = clock::parse_iso(SINCE).unwrap();
        let (s, bad) = summary(FIXTURE, since, &["108-live", "109-new"], Some(&base()));
        (s.to_json(), bad)
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
    fn a_start_drops_the_phase_of_an_earlier_run() {
        let text = concat!(
            r#"{"event":"phase","unit":"7-a","phase":"verify","ts":"2026-10-09T01:00:00Z"}"#,
            "\n",
            r#"{"event":"start","unit":"7-a","ts":"2026-10-10T01:00:00Z"}"#,
            "\n",
        );
        let s = summary(text, 0, &["7-a"], None).0.to_json();
        assert_eq!(s["running"], json!([{"unit":"7-a","issue":7}]));
    }

    #[test]
    fn a_read_error_mid_log_fails_the_summary() {
        let ok = |l: &str| Ok(serde_json::from_str::<Event>(l).unwrap());
        let events = vec![
            ok(r#"{"event":"end","unit":"7-a","outcome":"merged"}"#),
            Err(io::Error::other("disk gone")),
            ok(r#"{"event":"end","unit":"8-b","outcome":"merged"}"#),
        ];
        assert!(summarise(events, 0, &[]).is_err());
    }

    #[test]
    fn a_running_units_issue_is_the_logged_one_else_its_ids() {
        let line = r#"{"event":"phase","unit":"7-a","issue":8,"phase":"build"}"#;
        let s = summary(line, 0, &["7-a", "x-b"], None).0.to_json();
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
            units(&summary(&line, since, &[], None).0.to_json()["ended"]["merged"]).len()
        };
        assert_eq!(at(SINCE), 1);
        assert_eq!(at("2026-10-08T23:59:59Z"), 0);
        let undated = r#"{"event":"end","unit":"7-a","outcome":"merged"}"#;
        assert_eq!(
            summary(undated, 0, &[], None).0.to_json()["ended"]["merged"],
            json!([])
        );
    }

    #[test]
    fn events_without_a_unit_are_ignored() {
        let line = r#"{"event":"end","outcome":"merged","ts":"2026-10-09T01:00:00Z"}"#;
        let (s, bad) = summary(line, 0, &[], None);
        assert_eq!(
            (s.to_json()["ended"]["merged"].clone(), bad),
            (json!([]), 0)
        );
    }

    #[test]
    fn the_text_holds_what_the_json_holds() {
        let since = clock::parse_iso(SINCE).unwrap();
        let (mut s, _) = summary(FIXTURE, since, &["108-live", "109-new"], Some(&base()));
        s.next = Some(vec![Queued {
            number: 120,
            title: "Add\u{1b}[31m x".into(),
        }]);
        assert_eq!(s.to_json()["since"], SINCE);
        assert_eq!(
            s.text(),
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
        let json = s.to_json();
        assert_eq!(
            json["next"],
            json!([{"number": 120, "title": "Add\u{1b}[31m x"}])
        );
        assert_eq!(
            serde_json::from_str::<Value>(&json.to_string()).unwrap(),
            json
        );
        assert!(!json.to_string().contains("secret"));
        assert!(!json.to_string().contains("transcript"));
    }

    #[test]
    fn an_empty_night_shows_each_part_and_an_unavailable_queue() {
        let (s, bad) = summary("", 0, &[], None);
        assert_eq!(bad, 0);
        assert_eq!(
            s.text(),
            "since 1970-01-01T00:00:00Z\nrunning (0)\nmerged (0)\nPR for a human (0)\nstuck (0)\n\
             paused (0)\nsplit (0)\nqueue unavailable\n"
        );
        assert_eq!(s.to_json()["next"], Value::Null);
    }

    #[test]
    fn main_events_since_since_show_in_their_own_part() {
        let text = concat!(
            r#"{"event":"main_red","sha":"0123456789abcdef","failing":["test"],"issue":100,"ts":"2026-10-08T23:00:00Z"}"#,
            "\n",
            r#"{"event":"main_red","sha":"abcdef0123456789","failing":["test"],"issue":100,"ts":"2026-10-09T01:00:00Z"}"#,
            "\n",
            r#"{"event":"main_green","sha":"1111111111111111","state":"green","ts":"2026-10-09T02:00:00Z"}"#,
            "\n",
            r#"{"event":"main_red_unfixed","sha":"abcdef0123456789","issue":100,"outcome":"stuck","ts":"2026-10-09T03:00:00Z"}"#,
            "\n",
            r#"{"event":"worker_start","worker":1,"issue":100,"ts":"2026-10-09T03:00:00Z"}"#,
            "\n",
        );
        let since = clock::parse_iso(SINCE).unwrap();
        let (s, _) = summary(text, since, &[], None);
        assert_eq!(
            s.to_json()["main"],
            json!([
                {"event":"main_red","sha":"abcdef0123456789","issue":100,"ts":"2026-10-09T01:00:00Z"},
                {"event":"main_green","sha":"1111111111111111","ts":"2026-10-09T02:00:00Z"},
                {"event":"main_red_unfixed","sha":"abcdef0123456789","issue":100,"outcome":"stuck",
                 "ts":"2026-10-09T03:00:00Z"},
            ])
        );
        assert!(
            s.text().ends_with(
                "main (3)
  2026-10-09T01:00:00Z main_red abcdef0 #100
  2026-10-09T02:00:00Z main_green 1111111
  2026-10-09T03:00:00Z main_red_unfixed abcdef0 #100 outcome stuck
queue unavailable
"
            ),
            "{}",
            s.text()
        );
        assert_eq!(summary("", 0, &[], None).0.to_json()["main"], json!([]));
    }

    struct Fake(Vec<Issue>);

    impl Forge for Fake {
        fn open_issues(&self, label: Option<&str>) -> Result<Vec<Issue>> {
            match label {
                Some("broken") => anyhow::bail!("gh: not logged in"),
                _ => Ok(self.0.clone()),
            }
        }
        fn labelled_issues(&self, _: &str) -> Result<Vec<crate::watch::forge::Filed>> {
            Ok(Vec::new())
        }
        fn closing_prs(&self, _: PrState) -> Result<ClosingPrs> {
            Ok(ClosingPrs::new())
        }
        fn merged_prs(&self, _: Option<i64>) -> Result<crate::watch::forge::MergedPrs> {
            unreachable!("ns status only reads the queue")
        }
        fn is_closed(&self, _: u64) -> Result<bool> {
            Ok(false)
        }
        fn issue_link(&self, _: u64) -> Result<crate::watch::forge::IssueLink> {
            unreachable!("ns status only reads the queue")
        }
        fn pr(&self, _: u64) -> Result<crate::watch::forge::Pr> {
            unreachable!("ns status only reads the queue")
        }
        fn commit_prs(&self, _: &str) -> Result<Vec<crate::watch::forge::CommitPr>> {
            unreachable!("ns status only reads the queue")
        }
        fn state(&self, _: u64) -> Result<IssueState> {
            unreachable!("ns status only reads the queue")
        }
        fn sub_issues(&self, _: u64) -> Result<Vec<u64>> {
            unreachable!("ns status only reads the queue")
        }
        fn team_comments(&self, _: u64) -> Result<Vec<String>> {
            unreachable!("ns status only reads the queue")
        }
        fn part_of(&self, _: u64) -> Result<Vec<Issue>> {
            unreachable!("ns status only reads the queue")
        }
        fn ensure_label(&self, _: &str, _: &str) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn labels(&self, _: u64) -> Result<Vec<String>> {
            unreachable!("ns status only reads the queue")
        }
        fn edit_labels(&self, _: u64, _: &[String], _: Option<&str>) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn comment(&self, _: u64, _: &crate::watch::forge::Post) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn close(&self, _: u64) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn add_pr_label(&self, _: u64, _: &str) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn remove_pr_label(&self, _: u64, _: &str) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn comment_pr(&self, _: u64, _: &crate::watch::forge::Post) -> Result<()> {
            unreachable!("ns status only reads the queue")
        }
        fn main_checks(&self) -> Result<crate::watch::ci::HeadChecks> {
            unreachable!("ns status only reads the queue")
        }
        fn create_issue(&self, _: &str, _: &crate::watch::forge::Post, _: &[&str]) -> Result<u64> {
            unreachable!("ns status only reads the queue")
        }
        fn check_log(&self, _: &str) -> Result<String> {
            unreachable!("ns status only reads the queue")
        }
        fn last_green(&self, _: &str) -> Result<Option<String>> {
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
        let ready = next(&forge, &fac).unwrap();
        let ready: Vec<(u64, &str)> = ready.iter().map(|q| (q.number, q.title.as_str())).collect();
        assert_eq!(ready, [(6, "Fix 6"), (5, "Fix 5")]);
        fac.queue.ready_label = "broken".into();
        assert!(next(&forge, &fac).is_err());
    }
}
