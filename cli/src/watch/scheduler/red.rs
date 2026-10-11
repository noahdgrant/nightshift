//! Fix main first, in the scheduler's fill: which issues red main lets start and what the run log
//! says of it (docs/FACTORY.md, "Main's CI").

use std::collections::BTreeSet;

use anyhow::Result;
use serde_json::json;

use super::Scheduler;
use crate::run;
use crate::watch::{
    ci,
    fix_main::{self, Fix},
    Issue,
};

/// What red main allows a fill to start.
pub(super) enum Gate {
    /// Main is not red: the whole queue.
    Open,
    /// Only the fix issue.
    Only(Fix),
    /// Nothing: the fix issue for this head was closed and main is still red.
    Shut,
}

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
        if !main.holds_queue() {
            self.log_green(main);
            return Ok(Gate::Open);
        }
        let (repo, forge, loaded) = (self.plan.repo, self.plan.forge, self.plan.loaded);
        let startable =
            |i: &Issue| taken.contains(&i.number) || ready.iter().any(|r| r.number == i.number);
        let Some(fix) = fix_main::find_or_file(
            repo,
            forge,
            &loaded.fac,
            main,
            self.fix.as_ref(),
            &self.filed,
        )?
        else {
            if self.shut_at != main.sha {
                let n = self.fix.as_ref().map(|i| i.number);
                eprintln!(
                    "ns watch: warning: main is still red at {}; the fix issue{} already filed \
                     for it is closed, so none is filed again and the queue stays held",
                    main.sha.as_deref().unwrap_or("?"),
                    n.map_or(String::new(), |n| format!(" #{n}")),
                );
                self.shut_at = main.sha.clone();
            }
            return Ok(Gate::Shut);
        };
        if let Some(warning) = fix.warning(&startable) {
            eprintln!("{warning}");
        }
        if let (Fix::JustFiled(_), Some(sha)) = (&fix, &main.sha) {
            self.filed.insert(sha.clone());
        }
        self.fix = Some(fix.issue().clone());
        self.log_red(main, fix.issue());
        Ok(Gate::Only(fix))
    }

    /// Say, once per red head, that main is red and which issue fixes it.
    fn log_red(&mut self, main: &ci::MainCi, fix: &Issue) {
        if self.red_at == main.sha {
            return;
        }
        let failing: Vec<&str> = main.failing.iter().map(|c| c.name.as_str()).collect();
        eprintln!(
            "ns watch: main is red ({}); only #{} {} starts",
            failing.join(", "),
            fix.number,
            fix.title
        );
        run::log_event(
            &self.plan.repo.common_dir,
            json!({"event": "main_red", "sha": main.sha, "failing": failing, "issue": fix.number}),
        );
        self.red_at = main.sha.clone();
    }

    /// Say, when main was red, that it is not any more.
    fn log_green(&mut self, main: &ci::MainCi) {
        if self.red_at.take().is_none() {
            return;
        }
        eprintln!("ns watch: main is not red any more; the queue resumes");
        run::log_event(
            &self.plan.repo.common_dir,
            json!({"event": "main_green", "sha": main.sha, "state": main.state}),
        );
    }
}
