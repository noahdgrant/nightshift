//! A systemd user service and timer that start `ns auto` for one repository. The unit files live
//! in the user's own directory, so nothing here needs root.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use super::{Installed, Runner, RunnerState};

/// Runs `systemctl --user` with these arguments.
pub type Ctl<'a> = &'a mut dyn FnMut(&[&str]) -> Result<()>;

/// `$XDG_CONFIG_HOME/systemd/user`, or under `~/.config`: where systemd looks for a user's units.
pub fn dir_find() -> Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(config.join("systemd").join("user"))
}

pub fn ctl(args: &[&str]) -> Result<()> {
    let out = Command::new("systemctl")
        .arg("--user")
        .args(args)
        .output()
        .context("cannot start systemctl")?;
    if !out.status.success() {
        bail!(
            "systemctl --user {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// The name both files share: the checkout's directory name and a hash of its whole path, so two
/// checkouts with the same name do not share a runner.
fn name_of(root: &Path) -> String {
    let hash: String = Sha256::digest(root.as_os_str().as_encoded_bytes())
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir: String = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("ns-runner-{dir}-{hash}")
}

/// Escapes what systemd reads inside a value: `%` and `$` start specifiers and variables.
fn escape(text: &str) -> String {
    text.replace('%', "%%").replace('$', "$$")
}

/// A word of an `ExecStart` line, in double quotes so a space does not split it.
fn quote(text: &str) -> String {
    format!(
        "\"{}\"",
        escape(text).replace('\\', "\\\\").replace('"', "\\\"")
    )
}

pub fn service_text(runner: &Runner) -> String {
    let ns = quote(&runner.ns.to_string_lossy());
    format!(
        "[Unit]\n\
         Description=ns auto in {root}\n\
         \n\
         [Service]\n\
         Type=oneshot\n\
         WorkingDirectory={root}\n\
         Environment={path}\n\
         ExecStart=-{ns} auto --until {until}\n\
         ExecStart={ns} improve\n\
         StandardOutput=journal\n\
         StandardError=journal\n",
        root = escape(&runner.root.to_string_lossy()),
        path = quote(&format!("PATH={}", runner.path)),
        until = runner.until.format("%H:%M"),
    )
}

pub fn timer_text(runner: &Runner) -> String {
    format!(
        "[Unit]\n\
         Description=Start ns auto in {root} each night\n\
         \n\
         [Timer]\n\
         OnCalendar=*-*-* {start}:00\n\
         \n\
         [Install]\n\
         WantedBy=timers.target\n",
        root = escape(&runner.root.to_string_lossy()),
        start = runner.start.format("%H:%M"),
    )
}

fn files_of(dir: &Path, root: &Path) -> (String, PathBuf, PathBuf) {
    let name = name_of(root);
    let service = dir.join(format!("{name}.service"));
    let timer = dir.join(format!("{name}.timer"));
    (name, service, timer)
}

/// Writes the service and the timer into `dir` and enables the timer. A file already as it should
/// be is left alone, and enabling a timer that is enabled changes nothing.
pub fn install(dir: &Path, runner: &Runner, ctl: Ctl) -> Result<Installed> {
    let (name, service, timer) = files_of(dir, &runner.root);
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let mut changed = false;
    for (file, text) in [
        (&service, service_text(runner)),
        (&timer, timer_text(runner)),
    ] {
        if std::fs::read_to_string(file).ok().as_deref() == Some(text.as_str()) {
            continue;
        }
        std::fs::write(file, text).with_context(|| format!("cannot write {}", file.display()))?;
        changed = true;
    }
    if changed {
        ctl(&["daemon-reload"])?;
    }
    ctl(&["enable", "--now", &format!("{name}.timer")])?;
    Ok(Installed {
        files: vec![service, timer],
        changed,
    })
}

/// Disables the timer and deletes both files. Returns whether either was there.
pub fn remove(dir: &Path, root: &Path, ctl: Ctl) -> Result<bool> {
    let (name, service, timer) = files_of(dir, root);
    if timer.exists() {
        ctl(&["disable", "--now", &format!("{name}.timer")])?;
    }
    let mut removed = false;
    for file in [&timer, &service] {
        match std::fs::remove_file(file) {
            Ok(()) => removed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("cannot delete {}", file.display())),
        }
    }
    if removed {
        ctl(&["daemon-reload"])?;
    }
    Ok(removed)
}

/// The service without its `PATH`, which differs from one shell to the next and is no reason to
/// call an installed runner out of date.
fn without_path(service: &str) -> String {
    service
        .lines()
        .filter(|l| !l.starts_with("Environment="))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compares the files in `dir` with what `install` would write for `runner`, leaving the `PATH`
/// out of the comparison.
pub fn check(dir: &Path, runner: &Runner) -> Result<RunnerState> {
    let (_, service, timer) = files_of(dir, &runner.root);
    let read = |file: &Path| std::fs::read_to_string(file).ok();
    let (have_service, have_timer) = (read(&service), read(&timer));
    if have_service.is_none() && have_timer.is_none() {
        return Ok(RunnerState::NotInstalled);
    }
    let current = have_service.as_deref().map(without_path)
        == Some(without_path(&service_text(runner)))
        && have_timer.as_deref() == Some(timer_text(runner).as_str());
    Ok(if current {
        RunnerState::Current
    } else {
        RunnerState::Differs
    })
}

#[cfg(test)]
mod tests {
    use chrono::NaiveTime;

    use super::*;

    fn runner(root: &str, start: &str, until: &str) -> Runner {
        Runner {
            root: PathBuf::from(root),
            ns: PathBuf::from("/opt/ns bin/ns"),
            path: "/usr/bin:/home/me/.local/bin".into(),
            start: NaiveTime::parse_from_str(start, "%H:%M").unwrap(),
            until: NaiveTime::parse_from_str(until, "%H:%M").unwrap(),
        }
    }

    /// A `ctl` that writes down what it was told.
    fn recorder(calls: &mut Vec<String>) -> impl FnMut(&[&str]) -> Result<()> + '_ {
        move |args| {
            calls.push(args.join(" "));
            Ok(())
        }
    }

    #[test]
    fn the_service_is_a_oneshot_with_no_restart_that_runs_ns_auto_until_the_given_time() {
        let text = service_text(&runner("/work/repo", "22:00", "06:00"));
        assert_eq!(
            text,
            "[Unit]\n\
             Description=ns auto in /work/repo\n\
             \n\
             [Service]\n\
             Type=oneshot\n\
             WorkingDirectory=/work/repo\n\
             Environment=\"PATH=/usr/bin:/home/me/.local/bin\"\n\
             ExecStart=-\"/opt/ns bin/ns\" auto --until 06:00\n\
             ExecStart=\"/opt/ns bin/ns\" improve\n\
             StandardOutput=journal\n\
             StandardError=journal\n"
        );
        assert!(!text.contains("Restart="));
        let other = service_text(&runner("/work/repo", "22:00", "05:45"));
        assert!(other.contains("auto --until 05:45\n"));
    }

    #[test]
    fn the_timer_fires_at_the_start_time() {
        let text = timer_text(&runner("/work/repo", "23:30", "06:00"));
        assert!(text.contains("OnCalendar=*-*-* 23:30:00\n"), "{text}");
        assert!(text.contains("WantedBy=timers.target\n"));
    }

    #[test]
    fn a_path_with_a_percent_a_dollar_a_quote_or_a_backslash_is_escaped() {
        let mut r = runner("/work/50%/repo", "22:00", "06:00");
        r.ns = PathBuf::from("/a\"b\\c$d/ns");
        let text = service_text(&r);
        assert!(
            text.contains("WorkingDirectory=/work/50%%/repo\n"),
            "{text}"
        );
        assert!(
            text.contains("ExecStart=-\"/a\\\"b\\\\c$$d/ns\" auto"),
            "{text}"
        );
    }

    #[test]
    fn install_writes_both_files_and_enables_the_timer() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("systemd/user");
        let r = runner("/work/repo", "22:00", "06:00");
        let mut calls = Vec::new();
        let done = install(&dir, &r, &mut recorder(&mut calls)).unwrap();
        assert!(done.changed);
        let name = name_of(&r.root);
        assert_eq!(
            done.files,
            [
                dir.join(format!("{name}.service")),
                dir.join(format!("{name}.timer"))
            ]
        );
        assert_eq!(
            std::fs::read_to_string(&done.files[0]).unwrap(),
            service_text(&r)
        );
        assert_eq!(
            std::fs::read_to_string(&done.files[1]).unwrap(),
            timer_text(&r)
        );
        assert_eq!(
            calls,
            [
                "daemon-reload".to_string(),
                format!("enable --now {name}.timer")
            ]
        );
    }

    #[test]
    fn a_second_install_changes_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let r = runner("/work/repo", "22:00", "06:00");
        let mut calls = Vec::new();
        let first = install(tmp.path(), &r, &mut recorder(&mut calls)).unwrap();
        let before: Vec<_> = first
            .files
            .iter()
            .map(|f| std::fs::metadata(f).unwrap().modified().unwrap())
            .collect();
        calls.clear();
        let second = install(tmp.path(), &r, &mut recorder(&mut calls)).unwrap();
        assert!(!second.changed);
        let after: Vec<_> = second
            .files
            .iter()
            .map(|f| std::fs::metadata(f).unwrap().modified().unwrap())
            .collect();
        assert_eq!(before, after);
        assert_eq!(calls, [format!("enable --now {}.timer", name_of(&r.root))]);
    }

    #[test]
    fn install_after_the_settings_changed_rewrites_the_files() {
        let tmp = tempfile::tempdir().unwrap();
        let mut calls = Vec::new();
        install(
            tmp.path(),
            &runner("/work/repo", "22:00", "06:00"),
            &mut recorder(&mut calls),
        )
        .unwrap();
        let later = runner("/work/repo", "23:00", "06:00");
        assert!(
            install(tmp.path(), &later, &mut recorder(&mut calls))
                .unwrap()
                .changed
        );
        assert_eq!(check(tmp.path(), &later).unwrap(), RunnerState::Current);
    }

    #[test]
    fn remove_disables_the_timer_and_deletes_both_files_and_a_second_remove_does_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let r = runner("/work/repo", "22:00", "06:00");
        let mut calls = Vec::new();
        install(tmp.path(), &r, &mut recorder(&mut calls)).unwrap();
        calls.clear();
        assert!(remove(tmp.path(), &r.root, &mut recorder(&mut calls)).unwrap());
        let name = name_of(&r.root);
        assert_eq!(
            calls,
            [
                format!("disable --now {name}.timer"),
                "daemon-reload".to_string()
            ]
        );
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 0);
        calls.clear();
        assert!(!remove(tmp.path(), &r.root, &mut recorder(&mut calls)).unwrap());
        assert!(calls.is_empty());
    }

    #[test]
    fn remove_leaves_another_repositorys_runner_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let mut calls = Vec::new();
        let other = runner("/work/other", "22:00", "06:00");
        install(tmp.path(), &other, &mut recorder(&mut calls)).unwrap();
        assert!(!remove(
            tmp.path(),
            Path::new("/work/repo"),
            &mut recorder(&mut calls)
        )
        .unwrap());
        assert_eq!(check(tmp.path(), &other).unwrap(), RunnerState::Current);
    }

    #[test]
    fn a_failing_enable_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let r = runner("/work/repo", "22:00", "06:00");
        let err = install(tmp.path(), &r, &mut |_| bail!("no bus")).unwrap_err();
        assert_eq!(err.to_string(), "no bus");
    }

    #[test]
    fn check_tells_a_missing_runner_from_a_current_one_and_one_that_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let r = runner("/work/repo", "22:00", "06:00");
        assert_eq!(check(tmp.path(), &r).unwrap(), RunnerState::NotInstalled);
        let mut calls = Vec::new();
        install(tmp.path(), &r, &mut recorder(&mut calls)).unwrap();
        assert_eq!(check(tmp.path(), &r).unwrap(), RunnerState::Current);
        let mut moved = runner("/work/repo", "22:00", "06:00");
        moved.ns = PathBuf::from("/elsewhere/ns");
        assert_eq!(check(tmp.path(), &moved).unwrap(), RunnerState::Differs);
        let later = runner("/work/repo", "21:00", "06:00");
        assert_eq!(check(tmp.path(), &later).unwrap(), RunnerState::Differs);
        let mut other_path = runner("/work/repo", "22:00", "06:00");
        other_path.path = "/somewhere/else".into();
        assert_eq!(
            check(tmp.path(), &other_path).unwrap(),
            RunnerState::Current
        );
    }
}
