use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};
use serde_json::{json, Value};

use crate::os::{Os, Runner};
use crate::{manifest, os, summary, vcs};

#[derive(ClapArgs)]
pub struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start `ns auto` for this repository each night, from its `[runner]` times, and keep a
    /// summary of each night.
    Install,
    /// Stop starting `ns auto` for this repository and delete what `install` wrote.
    Remove,
}

pub fn run(args: Args) -> Result<()> {
    let vcs = vcs::get();
    let root = vcs.root_find(&std::env::current_dir()?)?;
    let os = os::get();
    let out = match args.command {
        Command::Install => install(
            os.as_ref(),
            &root,
            std::env::current_exe().context("cannot find the running ns")?,
            std::env::var("PATH").context("PATH is not set to a text value")?,
            &vcs.state_dir_find(&root)?,
        )?,
        Command::Remove => remove(os.as_ref(), &root)?,
    };
    println!("{out}");
    Ok(())
}

/// The runner for the repository at `root`, from its settings and the running `ns`.
pub fn runner_build(root: &Path, ns: PathBuf, path: String) -> Result<Runner> {
    let times = manifest::runner_read(root)?;
    Ok(Runner {
        root: root.to_path_buf(),
        ns,
        path,
        start: times.start,
        until: times.until,
    })
}

/// Installs the runner. The settings are read first, so a bad one writes nothing.
fn install(os: &dyn Os, root: &Path, ns: PathBuf, path: String, state_dir: &Path) -> Result<Value> {
    let runner = runner_build(root, ns, path)?;
    let done = os.runner_install(&runner)?;
    Ok(json!({
        "installed": done.files,
        "changed": done.changed,
        "start": runner.start.format("%H:%M").to_string(),
        "until": runner.until.format("%H:%M").to_string(),
        "summaries": state_dir.join(summary::NAME_PATTERN),
    }))
}

fn remove(os: &dyn Os, root: &Path) -> Result<Value> {
    Ok(json!({ "removed": os.runner_remove(root)? }))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::process::Command;
    use std::time::Duration;

    use chrono::{DateTime, Local};

    use super::*;
    use crate::os::{Finished, Installed, RunnerState};

    /// An operating system that only writes down the runner it was asked to install or remove.
    #[derive(Default)]
    struct Recording {
        installed: RefCell<Vec<String>>,
        removed: RefCell<Vec<PathBuf>>,
    }

    impl Os for Recording {
        fn process_run(&self, _: Command, _: &[u8], _: Duration) -> Result<Finished> {
            unreachable!()
        }
        fn process_interact(&self, _: Command) -> Result<bool> {
            unreachable!()
        }
        fn process_replace(&self, _: Command) -> anyhow::Error {
            unreachable!()
        }
        fn shell_command(&self, _: &str) -> Command {
            unreachable!()
        }
        fn time_now(&self) -> DateTime<Local> {
            unreachable!()
        }
        fn sleep(&self, _: Duration) {
            unreachable!()
        }
        fn runner_install(&self, r: &Runner) -> Result<Installed> {
            self.installed
                .borrow_mut()
                .push(format!("{} {} {}", r.ns.display(), r.start, r.until));
            Ok(Installed {
                files: vec![PathBuf::from("/units/a.service")],
                changed: true,
            })
        }
        fn runner_remove(&self, root: &Path) -> Result<bool> {
            self.removed.borrow_mut().push(root.to_path_buf());
            Ok(true)
        }
        fn runner_check(&self, _: &Runner) -> Result<RunnerState> {
            unreachable!()
        }
    }

    fn repo(settings: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".nightshift")).unwrap();
        std::fs::write(tmp.path().join(".nightshift/nightshift.toml"), settings).unwrap();
        tmp
    }

    #[test]
    fn install_hands_the_os_the_settings_times_and_says_where_the_summaries_go() {
        let repo = repo("[runner]\nstart = \"21:30\"\n");
        let os = Recording::default();
        let out = install(
            &os,
            repo.path(),
            "/bin/ns".into(),
            "/bin".into(),
            Path::new("/state"),
        )
        .unwrap();
        assert_eq!(*os.installed.borrow(), ["/bin/ns 21:30:00 06:00:00"]);
        assert_eq!(
            out,
            json!({
                "installed": ["/units/a.service"],
                "changed": true,
                "start": "21:30",
                "until": "06:00",
                "summaries": "/state/night-YYYY-MM-DD.md",
            })
        );
    }

    #[test]
    fn a_time_that_is_not_a_time_of_day_writes_nothing() {
        let repo = repo("[runner]\nuntil = \"25:99\"\n");
        let os = Recording::default();
        let err = install(
            &os,
            repo.path(),
            "/bin/ns".into(),
            "/bin".into(),
            Path::new("/state"),
        )
        .unwrap_err();
        assert!(err.to_string().contains("runner.until"), "{err}");
        assert!(os.installed.borrow().is_empty());
    }

    #[test]
    fn remove_asks_the_os_to_remove_this_repositorys_runner() {
        let os = Recording::default();
        let out = remove(&os, Path::new("/work/repo")).unwrap();
        assert_eq!(out, json!({ "removed": true }));
        assert_eq!(*os.removed.borrow(), [PathBuf::from("/work/repo")]);
    }
}
