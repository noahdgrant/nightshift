//! Tells a person, with a command of their choosing, when a unit needs them. The command is in the
//! machine's config (see `config::notify`) and is never allowed to change what happens to a unit
//! or a night: whatever goes wrong with it is logged and `ns` goes on.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{bail, Result};
use serde_json::json;

use crate::os::Os;
use crate::runlog::RunLog;

/// How long the command may run before it is killed.
const TIMEOUT: Duration = Duration::from_secs(30);

/// A thing that happens which a person may want to be told of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Stuck,
    WaitingForHuman,
    Merged,
    NightEnded,
}

impl Move {
    pub const ALL: [Move; 4] = [
        Move::Stuck,
        Move::WaitingForHuman,
        Move::Merged,
        Move::NightEnded,
    ];

    /// The word the config, the environment and the run log use for it.
    pub fn name(self) -> &'static str {
        match self {
            Move::Stuck => "stuck",
            Move::WaitingForHuman => "waiting_for_human",
            Move::Merged => "merged",
            Move::NightEnded => "night_ended",
        }
    }

    pub fn parse(name: &str) -> Option<Move> {
        Move::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// `[notify]` in the machine's config.
#[derive(Debug, PartialEq, Eq)]
pub struct Settings {
    /// A command line, run by the shell.
    pub command: String,
    /// The moves it is run for.
    pub on: Vec<Move>,
}

/// What the command is told about a move.
pub enum Facts<'a> {
    /// A unit, with where it is and where a person finds it.
    Unit {
        unit: u64,
        title: &'a str,
        state: &'a str,
        /// Why it is where it is; empty when nothing says.
        reason: &'a str,
        /// The change request when there is one, else the unit.
        url: &'a str,
    },
    /// A night, with its summary file.
    Night { summary: &'a Path },
}

/// Runs the command for `mv` when `settings` names one for it, and the person was not already told
/// this of the unit. Never fails and never waits more than 30 seconds.
pub fn notify(os: &dyn Os, log: &RunLog, settings: Option<&Settings>, mv: Move, facts: &Facts) {
    notify_within(os, log, settings, mv, facts, TIMEOUT);
}

fn notify_within(
    os: &dyn Os,
    log: &RunLog,
    settings: Option<&Settings>,
    mv: Move,
    facts: &Facts,
    timeout: Duration,
) {
    let Some(settings) = settings.filter(|s| s.on.contains(&mv)) else {
        return;
    };
    let unit = match facts {
        Facts::Unit { unit, .. } => Some(unit.to_string()),
        Facts::Night { .. } => None,
    };
    if let Some(unit) = &unit {
        match log.notified(unit, mv.name()) {
            Ok(true) => return,
            Ok(false) => {}
            Err(e) => eprintln!("ns: cannot read the run log to see who was told: {e:#}"),
        }
        // Before the command, so a move is never told twice, even when the command is slow to die.
        if let Err(e) = log.notified_append(unit, mv.name()) {
            eprintln!("ns: cannot note that {} was told: {e:#}", mv.name());
        }
    }
    if let Err(e) = command_run(os, settings, mv, facts, timeout) {
        let mut event =
            json!({ "notify": "failed", "move": mv.name(), "reason": format!("{e:#}") });
        if let Some(unit) = unit {
            event["unit"] = unit.into();
        }
        eprintln!("ns: the notify command failed: {e:#}");
        if let Err(e) = log.append(&event) {
            eprintln!("ns: cannot log it: {e:#}");
        }
    }
}

/// The names of the variables the command is told, with the move.
const FACT_VARS: [&str; 6] = [
    "NS_NOTIFY_UNIT",
    "NS_NOTIFY_TITLE",
    "NS_NOTIFY_STATE",
    "NS_NOTIFY_REASON",
    "NS_NOTIFY_URL",
    "NS_NOTIFY_SUMMARY",
];

fn command_run(
    os: &dyn Os,
    settings: &Settings,
    mv: Move,
    facts: &Facts,
    timeout: Duration,
) -> Result<()> {
    let mut cmd = os.shell_command(&settings.command);
    // Its output would mix into the events `ns` prints.
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for var in FACT_VARS {
        cmd.env_remove(var);
    }
    cmd.env("NS_NOTIFY_MOVE", mv.name());
    match facts {
        Facts::Unit {
            unit,
            title,
            state,
            reason,
            url,
        } => {
            cmd.env("NS_NOTIFY_UNIT", unit.to_string())
                .env("NS_NOTIFY_TITLE", title)
                .env("NS_NOTIFY_STATE", state)
                .env("NS_NOTIFY_REASON", reason)
                .env("NS_NOTIFY_URL", url);
        }
        Facts::Night { summary } => {
            cmd.env("NS_NOTIFY_SUMMARY", summary);
        }
    }
    match os.process_run(cmd, &[], timeout)?.success {
        Some(true) => Ok(()),
        Some(false) => bail!("the command exited with failure"),
        None => bail!(
            "the command ran past {} seconds and was killed",
            timeout.as_secs()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::os;

    struct Fixture {
        tmp: tempfile::TempDir,
        log: RunLog,
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let log = RunLog::at(tmp.path().join("runs.jsonl"));
        Fixture { tmp, log }
    }

    fn settings(command: &str, on: &[Move]) -> Settings {
        Settings {
            command: command.into(),
            on: on.to_vec(),
        }
    }

    fn unit_facts() -> Facts<'static> {
        Facts::Unit {
            unit: 7,
            title: "a title; $(touch pwned)",
            state: "stuck",
            reason: "failed",
            url: "https://example.com/7",
        }
    }

    fn run(f: &Fixture, s: &Settings, mv: Move, facts: &Facts, timeout: Duration) {
        notify_within(os::get().as_ref(), &f.log, Some(s), mv, facts, timeout);
    }

    const LONG: Duration = Duration::from_secs(60);

    fn dump_command(f: &Fixture) -> String {
        let out = f.tmp.path().join("out");
        format!(
            "env | grep '^NS_NOTIFY_' | sort >> {}; echo --- >> {}",
            out.display(),
            out.display()
        )
    }

    fn dumped(f: &Fixture) -> String {
        std::fs::read_to_string(f.tmp.path().join("out")).unwrap_or_default()
    }

    #[test]
    fn a_unit_move_gives_the_command_every_fact_in_its_environment() {
        let f = fixture();
        let s = settings(&dump_command(&f), &Move::ALL);
        run(&f, &s, Move::Stuck, &unit_facts(), LONG);
        assert_eq!(
            dumped(&f),
            "NS_NOTIFY_MOVE=stuck\nNS_NOTIFY_REASON=failed\nNS_NOTIFY_STATE=stuck\n\
             NS_NOTIFY_TITLE=a title; $(touch pwned)\nNS_NOTIFY_UNIT=7\n\
             NS_NOTIFY_URL=https://example.com/7\n---\n"
        );
        assert!(!f.tmp.path().join("pwned").exists());
    }

    #[test]
    fn a_night_gives_the_summary_path_and_no_unit() {
        let f = fixture();
        let s = settings(&dump_command(&f), &Move::ALL);
        let summary = Path::new("/state/night-2026-11-17.md");
        run(&f, &s, Move::NightEnded, &Facts::Night { summary }, LONG);
        assert_eq!(
            dumped(&f),
            "NS_NOTIFY_MOVE=night_ended\nNS_NOTIFY_SUMMARY=/state/night-2026-11-17.md\n---\n"
        );
    }

    #[test]
    fn a_move_not_in_on_and_absent_settings_run_nothing() {
        let f = fixture();
        let s = settings(&dump_command(&f), &[Move::Merged]);
        run(&f, &s, Move::Stuck, &unit_facts(), LONG);
        notify_within(
            os::get().as_ref(),
            &f.log,
            None,
            Move::Stuck,
            &unit_facts(),
            LONG,
        );
        assert_eq!(dumped(&f), "");
    }

    #[test]
    fn a_move_notifies_once_per_unit_but_another_move_still_does() {
        let f = fixture();
        let s = settings(&dump_command(&f), &Move::ALL);
        run(&f, &s, Move::Stuck, &unit_facts(), LONG);
        run(&f, &s, Move::Stuck, &unit_facts(), LONG);
        assert_eq!(dumped(&f).matches("---").count(), 1);
        run(&f, &s, Move::Merged, &unit_facts(), LONG);
        assert_eq!(dumped(&f).matches("---").count(), 2);
        let other = Facts::Unit {
            unit: 8,
            title: "t",
            state: "stuck",
            reason: "",
            url: "u",
        };
        run(&f, &s, Move::Stuck, &other, LONG);
        assert_eq!(dumped(&f).matches("---").count(), 3);
    }

    #[test]
    fn a_command_that_fails_cannot_start_or_is_slow_only_leaves_a_log_line() {
        for (command, timeout) in [
            ("exit 1", LONG),
            ("no-such-program-anywhere", LONG),
            ("sleep 600", Duration::from_millis(200)),
        ] {
            let f = fixture();
            let s = settings(command, &Move::ALL);
            run(&f, &s, Move::Stuck, &unit_facts(), timeout);
            let failed: Vec<_> = f
                .log
                .events()
                .unwrap()
                .into_iter()
                .filter(|e| e["notify"] == "failed")
                .collect();
            assert_eq!(failed.len(), 1, "{command}");
            assert_eq!(failed[0]["move"], "stuck", "{command}");
            assert_eq!(failed[0]["unit"], "7", "{command}");
            assert!(failed[0]["reason"].as_str().unwrap().len() > 3, "{command}");
        }
    }

    #[test]
    fn the_reason_says_why_it_failed() {
        let f = fixture();
        run(
            &f,
            &settings("sleep 600", &Move::ALL),
            Move::Stuck,
            &unit_facts(),
            Duration::from_millis(100),
        );
        let events = f.log.events().unwrap();
        let reason = events.iter().find(|e| e["notify"] == "failed").unwrap()["reason"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(reason.contains("killed"), "{reason}");
    }

    /// Every `NS_NOTIFY_` name in `text`.
    fn variables(text: &str) -> std::collections::BTreeSet<String> {
        let mut names = std::collections::BTreeSet::new();
        for (at, _) in text.match_indices("NS_NOTIFY_") {
            let name: String = text[at..]
                .chars()
                .take_while(|c| c.is_ascii_uppercase() || *c == '_')
                .collect();
            names.insert(name);
        }
        names
    }

    #[test]
    fn the_readme_documents_the_variables_the_command_gets_and_no_others() {
        let code = include_str!("notify.rs");
        let code = &code[..code.find("#[cfg(test)]").unwrap()];
        let readme = include_str!("../../README.md");
        let section = &readme[readme.find("## Notify").unwrap()..];
        let section = &section[..section.find("\n## ").unwrap()];
        assert_eq!(variables(section), variables(code));
        assert_eq!(variables(code).len(), 7);
    }

    #[test]
    fn the_move_names_parse_back() {
        for m in Move::ALL {
            assert_eq!(Move::parse(m.name()), Some(m));
        }
        assert_eq!(Move::parse("asleep"), None);
    }
}
