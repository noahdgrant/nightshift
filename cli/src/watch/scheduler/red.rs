//! Fix main first, in the scheduler's fill: the reads and the run log around `core::red`'s
//! decisions of which issues red main lets start (docs/factory/watch.md, "Main's CI").

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;

use super::Scheduler;
use crate::run;
pub(super) use crate::watch::core::red::Gate;
use crate::watch::core::red::Note;
use crate::watch::{ci, fix_main, Issue};

impl Scheduler<'_> {
    /// Main's CI, read before each triage-only run and again before the units start. A state
    /// that can't be read holds nothing, as at the night's start.
    pub(super) fn main_ci(&self) -> Option<ci::MainCi> {
        ci::read_or_warn(self.plan.forge, &self.plan.loaded.fac)
    }

    /// While main is red, the issue that fixes it, the only one to start. `ready` is what the
    /// queue would start and `taken` what runs or is done tonight: an open fix issue outside both
    /// is still the one, but nothing starts it, so the night warns.
    pub(super) fn main_fix(
        &mut self,
        main: &ci::MainCi,
        ready: &[Issue],
        taken: &BTreeSet<u64>,
    ) -> Result<Gate> {
        let (gate, note) = if main.holds_queue() {
            let (repo, forge, loaded) = (self.plan.repo, self.plan.forge, self.plan.loaded);
            let red = &self.red;
            let found = fix_main::find_or_file(
                repo,
                forge,
                &loaded.fac,
                main,
                red.fix.as_ref(),
                &red.filed,
            )?;
            let startable =
                |i: &Issue| taken.contains(&i.number) || ready.iter().any(|r| r.number == i.number);
            if let Some(warning) = found.as_ref().and_then(|f| f.warning(&startable)) {
                eprintln!("{warning}");
            }
            self.red.red(main, found)
        } else {
            self.red.not_red()
        };
        if let Some(note) = note {
            self.say(main, &gate, note);
        }
        Ok(gate)
    }

    /// Say what `note` says of `main`, on stderr and in the run log.
    fn say(&self, main: &ci::MainCi, gate: &Gate, note: Note) {
        let common = &self.plan.repo.common_dir;
        match (note, gate) {
            (Note::Red, Gate::Only(fix)) => {
                let fix = fix.issue();
                let failing: Vec<&str> = main.failing.iter().map(|c| c.name.as_str()).collect();
                eprintln!(
                    "ns watch: main is red ({}); only #{} {} starts",
                    failing.join(", "),
                    fix.number,
                    fix.title
                );
                run::log_event(
                    common,
                    json!({"event": "main_red", "sha": main.sha, "failing": failing, "issue": fix.number}),
                );
            }
            (Note::Green, _) => {
                eprintln!("ns watch: main is not red any more; the queue resumes");
                run::log_event(
                    common,
                    json!({"event": "main_green", "sha": main.sha, "state": main.state}),
                );
            }
            (Note::Shut, _) => {
                let n = self.red.fix.as_ref().map(|i| i.number);
                eprintln!(
                    "ns watch: warning: main is still red at {}; the fix issue{} already filed \
                     for it is closed, so none is filed again and the queue stays held",
                    main.sha.as_deref().unwrap_or("?"),
                    n.map_or(String::new(), |n| format!(" #{n}")),
                );
            }
            (Note::Red, _) => {}
        }
    }
}
